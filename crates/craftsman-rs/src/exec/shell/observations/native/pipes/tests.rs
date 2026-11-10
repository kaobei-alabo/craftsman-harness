use super::*;

fn info() -> PipeFdInfo {
    // SAFETY: integer/fixed-integer native structs only, no pointer/enum fields.
    let mut value: PipeFdInfo = unsafe { std::mem::zeroed() };
    value.file.kind = DTYPE_PIPE;
    value.file.open_flags = FWRITE;
    value.pipe.stat.vst_mode = libc::S_IFIFO;
    value.pipe.stat.vst_blksize = 16384;
    value
}

#[test]
fn pipe_wrapper_layout_matches_the_complete_public_darwin_abi() {
    assert_eq!(std::mem::size_of::<FileInfo>(), 24);
    assert_eq!(std::mem::align_of::<FileInfo>(), 8);
    assert_eq!(std::mem::offset_of!(FileInfo, offset), 8);
    assert_eq!(std::mem::offset_of!(FileInfo, kind), 16);
    assert_eq!(std::mem::size_of::<libc::vinfo_stat>(), 136);
    assert_eq!(std::mem::size_of::<PipeInfo>(), 160);
    assert_eq!(std::mem::offset_of!(PipeInfo, handle), 136);
    assert_eq!(std::mem::offset_of!(PipeInfo, peer_handle), 144);
    assert_eq!(std::mem::offset_of!(PipeInfo, status), 152);
    assert_eq!(std::mem::size_of::<PipeFdInfo>(), 184);
    assert_eq!(std::mem::align_of::<PipeFdInfo>(), 8);
    assert_eq!(std::mem::offset_of!(PipeFdInfo, pipe), 24);
    assert_eq!(PROC_PIDFDPIPEINFO, 6);
}
#[test]
fn two_pipe_queries_never_raise_original_thirteen_query_or_eight_thread_bounds() {
    for threads in 0..=6 {
        assert!(within_original_query_bound(threads));
    }
    for threads in [7, 8, 9, usize::MAX] {
        assert!(!within_original_query_bound(threads));
    }
    assert_eq!(FIXED_NATIVE_QUERIES + 6 + PIPE_QUERIES, MAX_NATIVE_QUERIES);
    assert_eq!(FIXED_NATIVE_QUERIES + 8, MAX_NATIVE_QUERIES);
}
#[test]
fn signed_native_buffer_counts_are_exact_and_never_turned_into_usage_or_zero() {
    let mut value = info();
    value.pipe.stat.vst_size = 33;
    value.pipe.status = -7;
    value.file.open_flags = FWRITE | 0x80000000;
    let pipe = fields(&value).unwrap();
    assert_eq!(pipe.buffer_bytes_native, 33);
    assert_eq!(pipe.pipe_status_native, -7);
    assert_eq!(pipe.open_flags_native, FWRITE | 0x80000000);
    value.pipe.stat.vst_size = i64::MAX;
    assert_eq!(fields(&value).unwrap().buffer_bytes_native, i64::MAX as u64);
    value.pipe.stat.vst_size = -1;
    assert!(fields(&value).is_none());
}
#[test]
fn rebound_nonpipe_readonly_invalid_stat_and_short_native_returns_refuse() {
    let mut value = info();
    value.file.kind = 1;
    assert!(fields(&value).is_none());
    value = info();
    value.file.open_flags = 1;
    assert!(fields(&value).is_none());
    value = info();
    value.pipe.stat.vst_mode = libc::S_IFREG;
    assert!(fields(&value).is_none());
    value = info();
    value.pipe.stat.vst_blksize = 0;
    assert!(fields(&value).is_none());
    value = info();
    value.pipe.stat.vst_blksize = -1;
    assert!(fields(&value).is_none());
    let size = std::mem::size_of::<PipeFdInfo>();
    for returned in [-1, 0, size as i32 - 1, size as i32 + 1] {
        assert!(!complete_return(returned, size));
    }
    assert!(complete_return(size as i32, size));
}
#[test]
fn final_original_child_revalidation_gates_even_complete_pipe_metadata() {
    let mut report = String::new();
    append(
        &mut report,
        4,
        Facts::Queried {
            stdout: fields(&info()),
            stderr: fields(&info()),
        },
        false,
    );
    assert_eq!(report, "\ncall=4 native_child_pipes=changed-or-unavailable");
    report.clear();
    append(&mut report, 4, Facts::QueryBound, false);
    assert_eq!(report, "\ncall=4 native_child_pipes=changed-or-unavailable");
    report.clear();
    append(&mut report, 4, Facts::QueryBound, true);
    assert_eq!(report, "\ncall=4 native_child_pipes=original-query-bound");
}
#[test]
fn fixed_current_descriptor_reports_never_keep_opaque_handles_or_name_authority() {
    let mut value = info();
    value.pipe.handle = 123456789;
    value.pipe.peer_handle = 987654321;
    let mut report = String::new();
    append(
        &mut report,
        2,
        Facts::Queried {
            stdout: fields(&value),
            stderr: None,
        },
        true,
    );
    assert!(report.contains("native_child_pipe=stdout current_child_fd=1 buffer_bytes_native=0"));
    assert!(report.contains("native_child_pipe=stderr current_child_fd=2 metadata=unavailable"));
    assert!(
        report
            .bytes()
            .all(|b| b == b'\n' || (32..=126).contains(&b))
    );
    assert!(!report.contains("123456789") && !report.contains("987654321"));
    assert!(
        !report.contains("handle")
            && !report.contains("original_stdout")
            && !report.contains("usage")
    );
}

fn unread_pipe_fixture(pause_before_publication: bool) -> String {
    let pause = if pause_before_publication {
        "while [ ! -e release-ready ]; do sleep 0.01; done; "
    } else {
        ""
    };
    // Creation happens while fd 1 is redirected. Publish only after that
    // command has returned and the shell restored its original pipe writer.
    format!(
        "printf native-pipe-output; printf native-pipe-error >&2; printf ready > .ready-pending; {pause}mv .ready-pending ready.txt; sleep 30"
    )
}

#[tokio::test]
async fn actual_owned_child_pipe_snapshot_preserves_real_unread_bytes_without_reading_them() {
    use crate::platform_os::{ProcessTree, SpawnOptions};
    use std::time::Duration;
    use tokio::io::AsyncReadExt;

    let root = tempfile::tempdir().unwrap();
    let stdout_expected = b"native-pipe-output";
    let stderr_expected = b"native-pipe-error";
    let options = SpawnOptions {
        process_group: true,
        ..Default::default()
    };
    // ORIGINAL real writes precede a genuine separate ready file. Retain the
    // original ProcessTree pipe readers untouched until AFTER the snapshot.
    let mut tree =
        ProcessTree::spawn_shell(&unread_pipe_fixture(false), root.path(), &options).unwrap();
    tokio::time::timeout(Duration::from_secs(20), async {
        while !root.path().join("ready.txt").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("actual writes/ready before the bounded metadata snapshot");
    let pid = tree.id().expect("actual currently owned child");
    let report = super::super::failed_child_state(1, pid, None);
    assert!(report.contains("native_process=owned-child"), "{report}");
    assert!(
        report.contains(&format!(
            "native_child_pipe=stdout current_child_fd=1 buffer_bytes_native={}",
            stdout_expected.len()
        )),
        "{report}"
    );
    assert!(
        report.contains(&format!(
            "native_child_pipe=stderr current_child_fd=2 buffer_bytes_native={}",
            stderr_expected.len()
        )),
        "{report}"
    );
    let stdout = tree.take_stdout().unwrap();
    let stderr = tree.take_stderr().unwrap();
    tree.kill_tree().await;
    let read = async {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut stdout = stdout.take(4096);
        let mut stderr = stderr.take(4096);
        tokio::try_join!(stdout.read_to_end(&mut out), stderr.read_to_end(&mut err)).unwrap();
        (out, err)
    };
    let (out, err) = tokio::time::timeout(Duration::from_secs(2), read)
        .await
        .expect("original bounded post-kill pipe drain");
    assert_eq!(out, stdout_expected);
    assert_eq!(err, stderr_expected);
    assert!(!report.contains("native-pipe-output") && !report.contains("native-pipe-error"));
    assert!(
        report
            .bytes()
            .all(|b| b == b'\n' || (32..=126).contains(&b))
    );
}

#[tokio::test]
async fn readiness_is_published_after_redirection_closes_and_preserves_original_pipe_bytes() {
    use crate::platform_os::{ProcessTree, SpawnOptions};
    use std::time::Duration;
    use tokio::io::AsyncReadExt;

    let root = tempfile::tempdir().unwrap();
    let mut tree = ProcessTree::spawn_shell(
        &unread_pipe_fixture(true),
        root.path(),
        &SpawnOptions {
            process_group: true,
            ..Default::default()
        },
    )
    .unwrap();
    tokio::time::timeout(Duration::from_secs(20), async {
        while !root.path().join(".ready-pending").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("actual pending readiness file created");
    assert!(!root.path().join("ready.txt").exists());
    std::fs::write(
        root.path().join("release-ready"),
        b"publish actual readiness",
    )
    .unwrap();
    tokio::time::timeout(Duration::from_secs(20), async {
        while !root.path().join("ready.txt").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("actual readiness rename completed");
    assert_eq!(
        std::fs::read(root.path().join("ready.txt")).unwrap(),
        b"ready"
    );
    assert!(!root.path().join(".ready-pending").exists());
    let stdout = tree.take_stdout().unwrap();
    let stderr = tree.take_stderr().unwrap();
    tree.kill_tree().await;
    let read = async {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut stdout = stdout.take(4096);
        let mut stderr = stderr.take(4096);
        tokio::try_join!(stdout.read_to_end(&mut out), stderr.read_to_end(&mut err)).unwrap();
        (out, err)
    };
    let (out, err) = tokio::time::timeout(Duration::from_secs(2), read)
        .await
        .unwrap();
    assert_eq!(out, b"native-pipe-output");
    assert_eq!(err, b"native-pipe-error");
}

#[test]
fn current_pipe_metadata_is_cfgtest_scoped_and_never_a_reader_or_public_channel() {
    let shell = include_str!("../../../mod.rs").replace("\r\n", "\n");
    assert!(shell.contains("#[cfg(test)]\nmod observations;"));
    let observations = include_str!("../../../observations.rs").replace("\r\n", "\n");
    assert!(observations.contains("#[cfg(target_os = \"macos\")]\nmod native;"));
    let pipe = include_str!("../pipes.rs");
    for forbidden in [
        "read_to_end",
        "AsyncRead",
        "Context::",
        "Waker",
        "dup(",
        "open(",
        "set_var",
        "remove_var",
        "Serialize",
        "Debug",
        "ToolOutcome",
        "ToolMeta",
        "task_for_pid",
    ] {
        assert!(
            !pipe.contains(forbidden),
            "unexpected pipe metadata interface: {forbidden}"
        );
    }
    assert!(pipe.contains("libc::proc_pidfdinfo("));
    assert!(pipe.contains("const MAX_NATIVE_QUERIES: usize = 13;"));
    assert!(pipe.contains("const FIXED_NATIVE_QUERIES: usize = 5;"));
    assert!(pipe.contains("const PIPE_QUERIES: usize = 2;"));
    assert!(pipe.contains("if !same_child"));
}
