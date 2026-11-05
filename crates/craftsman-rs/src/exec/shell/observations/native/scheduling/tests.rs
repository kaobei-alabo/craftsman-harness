use super::*;

fn task() -> libc::proc_taskallinfo {
    // SAFETY: this ABI struct is integers/fixed arrays only, all zero-valid.
    unsafe { std::mem::zeroed() }
}
fn thread() -> libc::proc_threadinfo {
    // SAFETY: this ABI struct is integers/fixed arrays only, all zero-valid.
    unsafe { std::mem::zeroed() }
}
#[test]
fn exported_flag_masks_preserve_bits_without_claiming_script_entry() {
    let mut info = task();
    info.pbsd.pbi_flags = EXEC | PPWAIT | TRACED | INEXIT | LP64;
    let mut report = String::new();
    append_process(&mut report, 1, &info, true);
    for key in [
        "proc_exec_flag",
        "proc_parent_wait_flag",
        "proc_traced_flag",
        "proc_exit_flag",
        "proc_64bit_flag",
    ] {
        assert!(report.contains(&format!("{key}=true")), "{report}");
    }
    assert!(!report.contains("script_entered"));
    assert!(!report.contains("ready=true"));
}
#[test]
fn resource_policy_is_one_masked_enum_not_two_policies_when_terminate_is_set() {
    for (bits, expected) in [
        (0, "none-of-known-bits"),
        (0x200, "throttle-policy"),
        (0x400, "suspend-policy"),
        (0x600, "terminate-policy"),
    ] {
        assert_eq!(policy(bits | EXEC | 0x80000000), expected);
    }
    assert_ne!(policy(0x600), "suspend-policy");
    assert_ne!(policy(0x600), "throttle-policy");
}
#[test]
fn resource_action_preserves_unknown_combination_without_repair_or_authority() {
    for (bits, expected) in [
        (0, "none-of-known-bits"),
        (0x800, "throttle-action"),
        (0x1000, "suspend-action"),
        (0x1800, "unrecognized-combination"),
    ] {
        assert_eq!(action(bits | EXEC), expected);
    }
    let mut info = task();
    info.pbsd.pbi_flags = 0x1800;
    let mut report = String::new();
    append_process(&mut report, 2, &info, true);
    assert!(report.contains("proc_resource_action_bits_native=6144"));
}
#[test]
fn unknown_flags_are_retained_numerically_not_named_from_private_headers() {
    let mut info = task();
    info.pbsd.pbi_flags = 20987920;
    let mut report = String::new();
    append_process(&mut report, 3, &info, true);
    assert!(report.contains("proc_exec_flag=true"));
    assert!(report.contains("proc_parent_wait_flag=false"));
    assert!(report.contains("proc_unclassified_bits_native=20971520"));
    assert!(!report.contains("application"));
    assert!(!report.contains("donor"));
    assert!(!report.contains("suppressed=false"));
}
#[test]
fn process_nice_task_priority_and_policy_preserve_signed_native_extremes() {
    let mut info = task();
    info.pbsd.pbi_nice = i32::MIN;
    info.ptinfo.pti_priority = i32::MAX;
    info.ptinfo.pti_policy = -7;
    let mut report = String::new();
    append_process(&mut report, 4, &info, true);
    assert!(report.contains("nice_native=-2147483648"));
    assert!(report.contains("task_thread_policy_native=-7"));
    assert!(report.contains("task_priority_native=2147483647"));
}
#[test]
fn thread_policy_and_priorities_copy_exactly_without_name_or_address_retention() {
    let mut info = thread();
    info.pth_name.fill(27);
    info.pth_policy = -3;
    info.pth_curpri = i32::MIN;
    info.pth_priority = i32::MAX;
    info.pth_maxpriority = -4;
    let value = ThreadScheduling::from_info(&info);
    info.pth_curpri = 0;
    assert_eq!(
        info.pth_curpri, 0,
        "the original input changed, not the saved facts"
    );
    let mut report = String::new();
    value.append(&mut report, 5, 0);
    assert!(report.contains("thread_policy_native=-3 current_priority_native=-2147483648 priority_native=2147483647 max_priority_native=-4"));
    assert!(!report.contains("name"));
    assert!(!report.contains("address"));
    assert!(
        report
            .bytes()
            .all(|c| c == b'\n' || (32..=126).contains(&c))
    );
}
#[test]
fn failed_original_child_revalidation_hides_all_process_scheduler_details() {
    let mut info = task();
    info.pbsd.pbi_flags = u32::MAX;
    info.pbsd.pbi_nice = i32::MAX;
    let mut report = String::new();
    append_process(&mut report, 6, &info, false);
    assert_eq!(report, "\ncall=6 native_scheduling=changed-or-unavailable");
    assert!(!report.contains("nice_native"));
    assert!(!report.contains("proc_exec_flag"));
}
#[test]
fn public_projected_other_and_unclassified_masks_partition_every_original_bit() {
    for flags in [0, u32::MAX, 20987920, 0x1800, 0x600, 0x80000000] {
        let projected = flags & PROJECTED;
        let other_public = flags & PUBLIC_DEFINED & !PROJECTED;
        let unclassified = flags & !PUBLIC_DEFINED;
        assert_eq!(projected | other_public | unclassified, flags);
        assert_eq!(projected & other_public, 0);
        assert_eq!(other_public & unclassified, 0);
    }
    let mut report = String::new();
    append_process(&mut report, u64::MAX, &task(), true);
    ThreadScheduling::from_info(&thread()).append(&mut report, u64::MAX, 7);
    assert!(report.len() < 1024);
    assert!(
        report
            .bytes()
            .all(|c| c == b'\n' || (32..=126).contains(&c))
    );
}
