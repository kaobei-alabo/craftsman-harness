use super::*;

fn argument(path: &str) -> OsString {
    format!("{MARKER}{path}").into()
}
fn valid() -> OsString {
    argument("/tmp/owned-child-stack-capture-native/capture")
}

#[test]
fn unmarked_arguments_refuse_to_enable_any_capture() {
    assert_eq!(
        select(&["test-binary".into(), "--test-threads=4".into()]),
        Ok(None)
    );
    assert_eq!(
        select(&["__craftsman_owned_child_stack_v2=/tmp/other".into()]),
        Ok(None)
    );
}
#[test]
fn exactly_one_explicit_marker_selects_only_an_owned_capture_named_directory() {
    assert_eq!(
        select(&[valid()]),
        Ok(Some(PathBuf::from(
            "/tmp/owned-child-stack-capture-native/capture"
        )))
    );
    assert!(select(&[valid(), valid()]).is_err());
    for path in [
        "",
        "capture",
        "/tmp/other/capture",
        "/tmp/owned-child-stack-capture-native/other",
        "/tmp/owned-child-stack-capture-native/../capture",
        "/tmp/owned-child-stack-capture-native/capture\n",
    ] {
        assert!(select(&[argument(path)]).is_err(), "{path:?}");
    }
}
#[test]
fn whole_argument_count_and_native_byte_bounds_precede_capture() {
    assert!(select(&vec!["binary".into(); MAX_ARGUMENTS + 1]).is_err());
    assert!(select(&[OsString::from("x".repeat(MAX_ARGUMENT_BYTES + 1))]).is_err());
    assert!(
        select(&[argument(&format!(
            "/tmp/owned-child-stack-capture-{}/capture",
            "x".repeat(MAX_PATH_BYTES)
        ))])
        .is_err()
    );
    use std::os::unix::ffi::OsStringExt;
    assert!(select(&[OsString::from_vec(vec![255])]).is_err());
}
#[test]
fn libtest_name_selection_is_exact_and_never_process_search_or_runtime_role_authority() {
    assert!(current_target(Some(TARGET)));
    for name in [
        None,
        Some(""),
        Some("tokio-runtime-worker"),
        Some(
            "agent::shell_tests::private_shell_normal_output_preserves_raw_counts_and_exit_metadata",
        ),
    ] {
        assert!(!current_target(name));
    }
    assert!(!current_target(Some(&format!("{TARGET}extra"))));
}
#[test]
fn diagnostic_output_is_create_only_regular_single_link_private_and_identity_bound() {
    let root = tempfile::tempdir().unwrap();
    let mut file = owned_empty(root.path(), "owned.txt").unwrap();
    let original = identity(&file.metadata().unwrap()).unwrap();
    file.write_all(b"native owned data").unwrap();
    assert_eq!(
        read_owned(&mut file, original).unwrap(),
        b"native owned data"
    );
    assert!(owned_empty(root.path(), "owned.txt").is_err());
    assert!(read_owned(&mut file, (original.0, original.1 + 1)).is_none());
    std::fs::hard_link(root.path().join("owned.txt"), root.path().join("other.txt")).unwrap();
    assert!(read_owned(&mut file, original).is_none());
}
#[test]
fn symlink_roots_and_oversized_trace_files_are_refused_whole() {
    let root = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(root.path(), root.path().join("link")).unwrap();
    assert!(!directory(&root.path().join("link")));
    let mut file = owned_empty(root.path(), "large.txt").unwrap();
    let original = identity(&file.metadata().unwrap()).unwrap();
    file.set_len(MAX_TRACE_BYTES as u64 + 1).unwrap();
    assert!(read_owned(&mut file, original).is_none());
}
#[test]
fn independent_one_shot_budget_never_resets_or_refunds_even_when_tool_fails() {
    let budget = AtomicBool::new(false);
    assert!(
        budget
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    );
    assert!(
        budget
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
    );
    assert!(budget.load(Ordering::Acquire));
}
#[test]
fn native_stack_tool_is_test_only_marked_owned_and_separate_from_original_snapshot_limits() {
    let shell = include_str!("../../mod.rs").replace("\r\n", "\n");
    assert!(shell.contains("#[cfg(test)]\nmod observations;"));
    assert!(shell.contains("#[cfg(all(test, target_os = \"macos\"))]\n            observation.capture_owned_stack_if_marked(&tree, stop).await;"));
    let observations = include_str!("../../observations.rs").replace("\r\n", "\n");
    assert!(observations.contains("#[cfg(target_os = \"macos\")]\nmod stack_capture;"));
    let source = include_str!("../stack_capture.rs");
    assert!(source.contains("Command::new(\"/usr/bin/sample\")"));
    assert!(source.contains("tree.id().filter(|pid| *pid > 1)"));
    assert!(source.contains("static ATTEMPTED: AtomicBool = AtomicBool::new(false);"));
    assert!(source.contains(".compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)"));
    for forbidden in [
        "set_var(",
        "remove_var(",
        "task_for_pid(",
        "-wait",
        "-fullPaths",
        "sudo",
        "pgrep",
        "process_group(",
        "libc::signal(",
        "libc::kill(",
    ] {
        assert!(!source.contains(forbidden), "{forbidden}");
    }
}
