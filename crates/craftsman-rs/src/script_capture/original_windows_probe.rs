//! New ignored diagnostic data only, inserted into an owned exact-source copy.
use super::*;
use cap_std::fs::{Dir, Metadata};
use std::io::Read;
use std::path::Path;

fn native_time(time: std::io::Result<cap_std::time::SystemTime>) -> Value {
    match time {
        Ok(time) => {
            let (before_epoch, span) = match time.into_std().duration_since(std::time::UNIX_EPOCH) {
                Ok(span) => (false, span),
                Err(error) => (true, error.duration()),
            };
            json!({"before_epoch":before_epoch,"seconds":span.as_secs(),"nanoseconds":span.subsec_nanos()})
        }
        Err(error) => json!({"unavailable":format!("{:?}",error.kind())}),
    }
}

fn metadata(metadata: &Metadata) -> Value {
    let mut value = json!({
        "regular":metadata.is_file(),"directory":metadata.is_dir(),
        "symlink":metadata.file_type().is_symlink(),"size":metadata.len(),
        "readonly":metadata.permissions().readonly(),
        "modified":native_time(metadata.modified()),"accessed":native_time(metadata.accessed()),
        "created":native_time(metadata.created()),
    });
    #[cfg(unix)]
    {
        use cap_std::fs::MetadataExt;
        value["unix"] = json!({"dev":metadata.dev(),"ino":metadata.ino(),"nlink":metadata.nlink(),
            "mode":metadata.mode(),"uid":metadata.uid(),"gid":metadata.gid(),"rdev":metadata.rdev(),
            "size":metadata.size(),"atime":metadata.atime(),"atime_nsec":metadata.atime_nsec(),
            "mtime":metadata.mtime(),"mtime_nsec":metadata.mtime_nsec(),
            "ctime":metadata.ctime(),"ctime_nsec":metadata.ctime_nsec(),
            "blksize":metadata.blksize(),"blocks":metadata.blocks()});
    }
    #[cfg(windows)]
    {
        use cap_std::fs::MetadataExt;
        value["windows"] = json!({"file_attributes":metadata.file_attributes(),
            "creation_time_100ns":metadata.creation_time(),"access_time_100ns":metadata.last_access_time(),
            "write_time_100ns":metadata.last_write_time(),"file_size":metadata.file_size()});
    }
    value
}

fn observed(root: &Path) -> std::io::Result<Value> {
    let directory = Dir::open_ambient_dir(root, cap_std::ambient_authority())?;
    let path = directory.symlink_metadata("project-a/raw.bin")?;
    let file = directory.open_with(
        "project-a/raw.bin",
        &crate::platform_os::confined_file_options(false, false),
    )?;
    let handle = file.metadata()?;
    let mut handle_value = metadata(&handle);
    handle_value["native_dev"] = json!(cap_fs_ext::MetadataExt::dev(&handle));
    handle_value["native_ino"] = json!(cap_fs_ext::MetadataExt::ino(&handle));
    handle_value["nlink"] = json!(cap_fs_ext::MetadataExt::nlink(&handle));
    // This fresh probe handle is not the capture's retained handle. Full
    // capture identities/ChangeTime appear only in real historical_facts.
    let mut bytes = Vec::new();
    file.take(65537).read_to_end(&mut bytes)?;
    if bytes.len() > 65536 {
        return Err(std::io::Error::from(std::io::ErrorKind::InvalidData));
    }
    Ok(json!({"path":metadata(&path),"fresh_handle":handle_value,
        "whole_bytes_hex":hex(&bytes),"whole_bytes_sha256":hex(&Sha256::digest(&bytes)),
        "retained_capture_handle_observation":false}))
}

fn observation(root: &Path) -> Value {
    match observed(root) {
        Ok(value) => value,
        Err(error) => json!({"unavailable":format!("{:?}",error.kind()),
            "raw_os_error":error.raw_os_error(),"retained_capture_handle_observation":false}),
    }
}

pub(crate) fn write_packet(name: &str, packet: &Value) {
    let directory = std::env::current_dir().unwrap().join("owned-observations");
    std::fs::create_dir_all(&directory).unwrap();
    let body = serde_json::to_vec_pretty(packet).unwrap();
    assert!(
        body.len() <= 65536,
        "whole diagnostic packet exceeds its cap"
    );
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(directory.join(format!("{name}.json")))
        .unwrap();
    std::io::Write::write_all(&mut file, &body).unwrap();
}

pub(crate) async fn run(session: &Session, fixture: &Fixture) {
    let capture = session.script_capture.as_ref().unwrap();
    let target = fixture.root.join("project-a/raw.bin");
    for action in ["original", "same_bytes", "append_control"] {
        for stage in [
            native::Stage::BeforeRead,
            native::Stage::BeforeEof,
            native::Stage::AfterRead,
        ] {
            let reset = std::fs::write(&target, RAW);
            if let Err(error) = &reset {
                write_packet(
                    &format!("reset-{action}-{stage:?}"),
                    &json!({
                        "action":action,"requested_stage":format!("{stage:?}"),
                        "actual_reset_error":format!("{:?}",error.kind()),"raw_os_error":error.raw_os_error(),
                        "actual_stage_operations":null,"actual_returned_bytes_hex":null,
                    }),
                );
            }
            reset.unwrap();
            let baseline = capture.capture(session, &["project-a/raw.bin"]).await;
            if let Err(error) = &baseline {
                write_packet(
                    &format!("baseline-{action}-{stage:?}"),
                    &json!({
                        "action":action,"requested_stage":format!("{stage:?}"),
                        "baseline_capture_error":format!("{error:?}"),
                        "actual_stage_operations":null,"actual_returned_bytes_hex":null,
                        "actual_owned_target":observation(&fixture.root),
                        "actual_usage":*session.usage.lock().unwrap(),
                    }),
                );
            }
            let baseline = baseline.unwrap();
            let baseline_facts = baseline
                .historical_facts(session, "project-a/raw.bin")
                .unwrap();
            assert_eq!(
                baseline
                    .original_bytes(session, "project-a/raw.bin")
                    .unwrap(),
                Some(RAW)
            );
            drop(baseline);
            let rows = Arc::new(Mutex::new(Vec::new()));
            let observed = rows.clone();
            let once = Arc::new(AtomicBool::new(false));
            let mutation = once.clone();
            let root = fixture.root.clone();
            let file = target.clone();
            capture.set_hook(Arc::new(move |seen, name| {
                if seen != stage || name != "project-a/raw.bin" || mutation.swap(true, Ordering::SeqCst) {
                    return;
                }
                let before = observation(&root);
                let operation = if action == "append_control"
                    || (action == "original" && stage == native::Stage::BeforeEof)
                {
                    std::fs::OpenOptions::new().append(true).open(&file)
                        .and_then(|mut writer|std::io::Write::write_all(&mut writer,b"+"))
                } else if action == "same_bytes" {
                    std::fs::write(&file, RAW)
                } else {
                    std::fs::write(&file, [0, 254, 128, b'\r', b'\n', b'Z'])
                };
                let operation_error = operation.as_ref().err().map(|error| {
                    json!({"kind":format!("{:?}",error.kind()),"raw_os_error":error.raw_os_error()})
                });
                let after = observation(&root);
                observed.lock().unwrap().push(json!({"actual_stage":format!("{seen:?}"),
                    "operation_returned_success":operation.is_ok(),"operation_error":operation_error,
                    "before_operation":before,"after_operation":after}));
            }));
            let result = capture.capture(session, &["project-a/raw.bin"]).await;
            capture.clear_hook();
            let (error, returned_bytes, returned_facts) = match result {
                Ok(snapshot) => (
                    None,
                    snapshot
                        .original_bytes(session, "project-a/raw.bin")
                        .unwrap()
                        .map(hex),
                    Some(
                        snapshot
                            .historical_facts(session, "project-a/raw.bin")
                            .unwrap(),
                    ),
                ),
                Err(error) => (Some(format!("{error:?}")), None, None),
            };
            let rows = rows.lock().unwrap().clone();
            let row = json!({"action":action,"requested_stage":format!("{stage:?}"),
                "baseline_actual_native_facts":baseline_facts,"actual_stage_operations":rows,
                "actual_capture_error":error,"actual_returned_bytes_hex":returned_bytes,
                "actual_returned_native_facts":returned_facts,"actual_file_after":observation(&fixture.root),
                "actual_usage":*session.usage.lock().unwrap(),
                "source_scope":"3e actual selected TUI builder/capture; new observations in owned copy; macOS only when run here"});
            write_packet(&format!("{action}-{stage:?}"), &row);
            assert!(
                once.load(Ordering::SeqCst),
                "actual operation must have been reached"
            );
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0]["operation_returned_success"], true);
            if action == "original" || action == "append_control" {
                assert_eq!(row["actual_capture_error"], "Changed");
            }
            std::fs::write(&target, RAW).unwrap();
        }
    }
    fixture.originals_preserved();
    assert_eq!(*session.usage.lock().unwrap(), (0, 0));
    assert!(fixture.server.received_requests().await.unwrap().is_empty());
}
