use super::*;

fn path(value: &[u8]) -> [u8; PATH_BYTES] {
    let mut result = [0; PATH_BYTES];
    result[..value.len()].copy_from_slice(value);
    result
}
#[test]
fn native_path_return_requires_exact_whole_length_and_termination() {
    let good = path(b"/bin/sh");
    assert!(matches!(executable_result(&good, 7), Executable::SystemSh));
    for size in [-1, 0, 1, 6, 8, PATH_BYTES as i32, i32::MAX] {
        assert!(matches!(
            executable_result(&good, size),
            Executable::Unavailable
        ));
    }
    let mut missing = good;
    missing[7] = 1;
    assert!(matches!(
        executable_result(&missing, 7),
        Executable::Unavailable
    ));
    let mut embedded = good;
    embedded[4] = 0;
    assert!(matches!(
        executable_result(&embedded, 7),
        Executable::Unavailable
    ));
    assert!(matches!(
        executable_result(&path(b"bin/sh"), 6),
        Executable::Unavailable
    ));
    let mut edge = [b'x'; PATH_BYTES];
    edge[0] = b'/';
    edge[PATH_BYTES - 1] = 0;
    assert!(matches!(
        executable_result(&edge, (PATH_BYTES - 1) as i32),
        Executable::Other
    ));
}
#[test]
fn executable_classes_never_normalize_aliases_or_render_unknown_path_bytes() {
    for (value, class) in [
        (b"/bin/sh".as_slice(), Executable::SystemSh),
        (b"/bin/bash", Executable::SystemBash),
        (b"/bin/zsh", Executable::SystemZsh),
        (b"/bin/dash", Executable::SystemDash),
        (b"/bin/sleep", Executable::SystemSleep),
    ] {
        assert!(executable_result(&path(value), value.len() as i32) == class);
    }
    for value in [
        b"/bin/sh-extra".as_slice(),
        b"/bin//sh",
        b"/bin/./sh",
        b"/BIN/sh",
        b"/private-synthetic-value/sh",
        b"/bin/\xff\x1bsh",
    ] {
        let class = executable_result(&path(value), value.len() as i32);
        assert!(matches!(class, Executable::Other));
        assert_eq!(class.label(), "other");
    }
}
#[test]
fn cwd_relation_uses_original_device_inode_and_directory_type_not_path_text() {
    let workspace = Some(WorkspaceIdentity {
        device: 7,
        inode: 9,
    });
    // SAFETY: native struct consists only of fixed integer/array fields.
    let mut stat: libc::vinfo_stat = unsafe { std::mem::zeroed() };
    stat.vst_dev = 7;
    stat.vst_ino = 9;
    stat.vst_mode = libc::S_IFDIR | 0o700;
    assert!(matches!(
        cwd_result(&stat, workspace),
        Cwd::MatchesWorkspace
    ));
    stat.vst_dev = 8;
    assert!(matches!(cwd_result(&stat, workspace), Cwd::Different));
    stat.vst_dev = 7;
    stat.vst_ino = 10;
    assert!(matches!(cwd_result(&stat, workspace), Cwd::Different));
    stat.vst_ino = 9;
    stat.vst_mode = libc::S_IFREG;
    assert!(matches!(cwd_result(&stat, workspace), Cwd::Unavailable));
    stat.vst_mode = libc::S_IFDIR;
    stat.vst_ino = 0;
    assert!(matches!(cwd_result(&stat, workspace), Cwd::Unavailable));
    assert!(matches!(cwd_result(&stat, None), Cwd::WorkspaceUnavailable));
}
#[test]
fn retained_workspace_metadata_refuses_unrepresentable_device_or_missing_identity() {
    assert!(WorkspaceIdentity::from_fields(u64::from(u32::MAX), 9, true).is_some());
    assert!(WorkspaceIdentity::from_fields(u64::from(u32::MAX) + 1, 9, true).is_none());
    assert!(WorkspaceIdentity::from_fields(7, 0, true).is_none());
    assert!(WorkspaceIdentity::from_fields(7, 9, false).is_none());
}
#[test]
fn final_child_revalidation_discards_execution_facts_without_path_or_identity_leak() {
    let mut report = String::new();
    append(
        &mut report,
        3,
        Facts {
            executable: Executable::SystemSh,
            cwd: Cwd::MatchesWorkspace,
        },
        false,
    );
    assert_eq!(report, "\ncall=3 native_execution=changed-or-unavailable");
    report.clear();
    append(
        &mut report,
        3,
        Facts {
            executable: Executable::Other,
            cwd: Cwd::Different,
        },
        true,
    );
    assert_eq!(
        report,
        "\ncall=3 native_execution=observed executable_class=other native_cwd=different-from-invocation-workspace"
    );
    assert!(
        report
            .bytes()
            .all(|byte| byte == b'\n' || (32..=126).contains(&byte))
    );
    assert!(!report.contains("/"));
    assert!(!report.contains("device"));
    assert!(!report.contains("inode"));
}
