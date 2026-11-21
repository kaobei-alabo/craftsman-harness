//! Appended native diagnostic only; original producer and assertions are unchanged.
use super::*;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::process::Stdio;
use tokio::io::AsyncReadExt;

const HELPER: &str = "tui::session_bridge::script_capture_tests::original_windows_probe::marked_owned_capture_observation";
const MARKER: &str = "__craftsman_original_failure_observation__:";
const DONE: &[u8] =
    b"actual owned capture stage bytes, metadata, return and zero-model-call assertions completed";

fn image_hash(path: &std::path::Path) -> String {
    let mut file = std::fs::File::open(path).unwrap();
    let mut hash = Sha256::new();
    let mut bytes = [0; 8192];
    loop {
        let count = file.read(&mut bytes).unwrap();
        if count == 0 {
            break;
        }
        hash.update(&bytes[..count]);
    }
    format!("{:x}", hash.finalize())
}

#[test]
#[ignore = "native diagnostic only; exact dual markers required before environment/config/path work"]
fn marked_owned_capture_observation() {
    let args: Vec<_> = std::env::args().collect();
    if !args.iter().any(|arg| arg == "--exact") || !args.iter().any(|arg| arg == HELPER) {
        return;
    }
    let names: Vec<_> = args
        .iter()
        .filter_map(|arg| arg.strip_prefix(MARKER))
        .collect();
    if names != ["capture-phases"] {
        return;
    }
    let Some(marker) =
        proof::selected_marker().filter(|marker| marker.case() == proof::Case::Races)
    else {
        return;
    };
    tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap()
        .block_on(async {
            crate::script_capture::tests::original_windows_probe::write_packet("helper-start", &json!({
                "actual_pid":std::process::id(),
                "actual_image_sha256":image_hash(&std::env::current_exe().unwrap()),
                "original_fixture_source_sha256":format!("{:x}",Sha256::digest(include_bytes!("../../script_capture/tests.rs"))),
                "native_source_sha256":format!("{:x}",Sha256::digest(include_bytes!("../../script_capture/native.rs"))),
                "fixture_stage":"before_actual_fixture", "actual_usage":null,
                "role":"owned_selected_TUI_capture", "mode_gadgets":cfg!(feature="gadgets"),
            }));
            let fixture = proof::fixture(&marker).await;
            let (tx, _events) = mpsc::unbounded_channel();
            let mut slot = AgentSlot::spawn(&fixture.cfg, Arc::new(Mutex::new(ApprovalState::default())),
                UiBridge { tx }, craftsman_harness::SessionCancellation::default());
            let returned = tokio::time::timeout(Duration::from_secs(30), slot.get()).await.map(|s|s.is_some());
            crate::script_capture::tests::original_windows_probe::write_packet("builder-return", &json!({
                "actual_initialization_returned":returned.is_ok(),
                "actual_session_initialized":returned.as_ref().ok(),
                "deadline_expired":returned.is_err(),
                "actual_usage":slot.peek().map(|s|*s.usage.lock().unwrap()),
            }));
            assert!(returned.unwrap());
            let session = slot.peek().expect("actual marked selected TUI builder");
            crate::script_capture::tests::original_windows_probe::run(session,&fixture).await;
            fixture.originals_preserved();
            slot.shutdown().await;
        });
    eprintln!("{}", std::str::from_utf8(DONE).unwrap());
}

async fn run_marked(markers: &[&str], selected: bool) {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    let config = workspace.join("actual-settings");
    std::fs::create_dir_all(&config).unwrap();
    let workspace = workspace.canonicalize().unwrap();
    let config = config.canonicalize().unwrap();
    let home = root.path().canonicalize().unwrap();
    let image = std::env::current_exe().unwrap();
    let mut command = tokio::process::Command::new(&image);
    command
        .args(["--ignored", "--exact", HELPER, "--nocapture"])
        .args(markers)
        .current_dir(&workspace)
        .env_clear()
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .env("TMPDIR", &home)
        .env("TEMP", &home)
        .env("TMP", &home)
        .env("CRAFTSMAN_CONFIG_DIR", &config)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(path) = std::env::var_os("PATH") {
        command.env("PATH", path);
    }
    #[cfg(windows)]
    if let Some(root) = std::env::var_os("SystemRoot") {
        command.env("SystemRoot", root);
    }
    let mut child = command.spawn().unwrap();
    let pid = child.id();
    let mut stdout = child.stdout.take().unwrap().take(65537);
    let mut stderr = child.stderr.take().unwrap().take(65537);
    let mut out = Vec::new();
    let mut err = Vec::new();
    let result = tokio::time::timeout(Duration::from_secs(180), async {
        let (status, _, _) = tokio::try_join!(
            child.wait(),
            stdout.read_to_end(&mut out),
            stderr.read_to_end(&mut err)
        )?;
        Ok::<_, std::io::Error>(status)
    })
    .await;
    let cleanup = if !matches!(result, Ok(Ok(_))) {
        Some(
            tokio::time::timeout(Duration::from_secs(5), child.kill())
                .await
                .map(|r| r.map(|()| true).map_err(|e| format!("{:?}", e.kind())))
                .ok(),
        )
    } else {
        None
    };
    let base = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/verification/privacy-original-windows-observation/actual/capture");
    std::fs::create_dir_all(&base).unwrap();
    let directory = tempfile::Builder::new()
        .prefix("capture-")
        .tempdir_in(&base)
        .unwrap()
        .keep();
    for (name, bytes) in [("parent.stdout.raw", &out), ("parent.stderr.raw", &err)] {
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(directory.join(name))
            .unwrap();
        std::io::Write::write_all(&mut file, bytes).unwrap();
    }
    if workspace.join("owned-observations").exists() {
        for packet in std::fs::read_dir(workspace.join("owned-observations")).unwrap() {
            let packet = packet.unwrap();
            if packet.file_type().unwrap().is_file() {
                let bytes = std::fs::read(packet.path()).unwrap();
                assert!(bytes.len() <= 65536);
                let mut file = std::fs::OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(directory.join(packet.file_name()))
                    .unwrap();
                std::io::Write::write_all(&mut file, &bytes).unwrap();
            }
        }
    }
    let packet = json!({"actual_child_pid":pid,"actual_image_sha256":image_hash(&image),
        "actual_exit_code":result.as_ref().ok().and_then(|r|r.as_ref().ok()).and_then(|s|s.code()),
        "actual_exit_success":result.as_ref().ok().and_then(|r|r.as_ref().ok()).map(|s|s.success()),
        "deadline_expired":result.is_err(),"cleanup":cleanup,
        "stdout_captured_bytes":out.len(),"stderr_captured_bytes":err.len(),
        "stdout_sha256":format!("{:x}",Sha256::digest(&out)),"stderr_sha256":format!("{:x}",Sha256::digest(&err)),
        "whole_capture_within_cap":out.len()<=65536&&err.len()<=65536,
        "original_outer_bound_seconds":180,"original_cleanup_bound_seconds":5,"new_selected_observation":selected});
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(directory.join("parent.json"))
        .unwrap();
    std::io::Write::write_all(&mut file, &serde_json::to_vec_pretty(&packet).unwrap()).unwrap();
    assert!(out.len() <= 65536 && err.len() <= 65536);
    assert!(
        result.unwrap().unwrap().success(),
        "new actual capture observer failed; full packet retained"
    );
    if selected {
        assert!(err.windows(DONE.len()).any(|w| w == DONE));
        assert_eq!(
            std::fs::read_dir(workspace.join("owned-observations"))
                .unwrap()
                .count(),
            11
        );
    } else {
        assert!(!config.join("config.json").exists());
        assert!(!config.join("threads.db").exists());
        assert!(!workspace.join("project-a").exists());
        assert!(!workspace.join("owned-observations").exists());
    }
}

#[tokio::test]
async fn actual_capture_observer_records_before_original_changed_verdicts() {
    run_marked(
        &[
            "__craftsman_original_failure_observation__:capture-phases",
            "__craftsman_session_capture__:races",
        ],
        true,
    )
    .await;
}

#[tokio::test]
async fn capture_observer_unmarked_unknown_and_duplicate_return_before_owned_work() {
    run_marked(&[], false).await;
    run_marked(
        &[
            "__craftsman_original_failure_observation__:unknown",
            "__craftsman_session_capture__:races",
        ],
        false,
    )
    .await;
    run_marked(
        &[
            "__craftsman_original_failure_observation__:capture-phases",
            "__craftsman_original_failure_observation__:capture-phases",
            "__craftsman_session_capture__:races",
        ],
        false,
    )
    .await;
    run_marked(
        &[
            "__craftsman_original_failure_observation__:capture-phases",
            "__craftsman_session_capture__:races",
            "__craftsman_session_capture__:races",
        ],
        false,
    )
    .await;
}
