use super::*;

fn bsd(pid: u32) -> libc::proc_bsdinfo {
    // SAFETY: this native struct consists only of integers and fixed arrays.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    info.pbi_pid = pid;
    info.pbi_ppid = std::process::id();
    info.pbi_pgid = pid;
    info.pbi_start_tvsec = 12;
    info.pbi_start_tvusec = 34;
    info
}
fn thread() -> libc::proc_threadinfo {
    // SAFETY: this native struct consists only of integers and fixed arrays.
    unsafe { std::mem::zeroed() }
}
#[test]
fn complete_native_struct_return_rejects_errors_short_and_oversized_outputs() {
    let size = std::mem::size_of::<libc::proc_taskallinfo>();
    assert!(complete_return(size as i32, size));
    for returned in [-1, 0, size as i32 - 1, size as i32 + 1] {
        assert!(!complete_return(returned, size));
    }
}
#[test]
fn original_pid_parent_group_and_start_are_required_for_thread_revalidation() {
    let original = bsd(123);
    let identity = Identity::current(&original, 123).unwrap();
    assert!(identity.matches(&original));
    let mut changed = bsd(123);
    changed.pbi_pid += 1;
    assert!(Identity::current(&changed, 123).is_none());
    changed = bsd(123);
    changed.pbi_ppid = 0;
    assert!(Identity::current(&changed, 123).is_none());
    changed = bsd(123);
    changed.pbi_pgid += 1;
    assert!(Identity::current(&changed, 123).is_none());
    changed = bsd(123);
    changed.pbi_start_tvsec += 1;
    assert!(!identity.matches(&changed));
    changed = bsd(123);
    changed.pbi_start_tvusec += 1;
    assert!(!identity.matches(&changed));
}
#[test]
fn native_thread_list_is_fixed_bounded_and_never_a_partial_complete_set() {
    let width = std::mem::size_of::<u64>() as i32;
    for returned in [-1, 0, 1, width - 1, (MAX_THREADS as i32 + 1) * width + 1] {
        assert!(matches!(
            returned_slots(returned),
            Err(ListProblem::Unavailable)
        ));
    }
    for count in 1..=MAX_THREADS {
        assert_eq!(returned_slots(count as i32 * width).ok(), Some(count));
    }
    assert!(matches!(
        returned_slots((MAX_THREADS as i32 + 1) * width),
        Err(ListProblem::Overbound)
    ));
}
#[test]
fn native_pthread_keys_are_not_thread_ids_and_ambiguous_duplicates_are_refused() {
    // Zero can be an actual native pthread key; never synthesize a thread-id.
    assert!(unique_slots(&[0, 91, 7]).is_ok());
    assert!(matches!(unique_slots(&[0, 0]), Err(ListProblem::Ambiguous)));
    assert!(matches!(
        unique_slots(&[7, 91, 7]),
        Err(ListProblem::Ambiguous)
    ));
}
#[test]
fn fixed_reports_preserve_native_integer_facts_without_kernel_names_or_addresses() {
    let mut process = bsd(123);
    for slot in &mut process.pbi_comm {
        *slot = 27;
    }
    assert_eq!(image_class(&process), "other");
    // SAFETY: this native struct consists only of integers and fixed arrays.
    let mut task: libc::proc_taskinfo = unsafe { std::mem::zeroed() };
    task.pti_total_user = u64::MAX;
    task.pti_total_system = 42;
    task.pti_threadnum = 3;
    task.pti_numrunning = 1;
    task.pti_csw = i32::MAX;
    task.pti_syscalls_unix = -1;
    let mut report = String::new();
    append_task(&mut report, &task, 2);
    assert!(report.contains("total_user_native=18446744073709551615"));
    assert!(report.contains("total_system_native=42"));
    assert!(report.contains("thread_count=3 running_threads=1"));
    assert!(report.contains("context_switches=2147483647 unix_syscalls=-1"));
    let mut native = thread();
    for slot in &mut native.pth_name {
        *slot = 27;
    }
    native.pth_user_time = 9;
    native.pth_run_state = 3;
    ThreadFacts::from_info(&native).append(&mut report, 2, 0);
    assert!(report.contains("native_thread=0 user_time_native=9"));
    assert!(report.contains("run_state=3"));
    assert!(
        report
            .bytes()
            .all(|byte| byte == b'\n' || (32..=126).contains(&byte))
    );
    assert!(!report.contains("address"));
    assert!(!report.contains("name"));
}
#[test]
fn later_child_identity_failure_discards_even_successful_thread_subqueries() {
    let mut report = String::new();
    append_threads(
        &mut report,
        8,
        Ok(vec![ThreadFacts::from_info(&thread())]),
        false,
    );
    assert_eq!(report, "\ncall=8 native_threads=changed-or-unavailable");
    assert!(!report.contains("native_thread="));
    report.clear();
    append_threads(&mut report, 8, Err(ListProblem::Overbound), false);
    assert_eq!(report, "\ncall=8 native_threads=changed-or-unavailable");
    report.clear();
    append_threads(&mut report, 8, Err(ListProblem::Ambiguous), true);
    assert_eq!(report, "\ncall=8 native_threads=ambiguous");
}
