//! macOS-only TEST diagnostics of a currently active actual-shell child.
//! One bounded failure snapshot, not a sampler, authority or progress proof.

use std::fmt::Write;

mod execution;
mod pipes;
mod scheduling;

#[derive(Clone, Copy)]
pub(super) struct WorkspaceIdentity {
    device: u32,
    inode: u64,
}
impl WorkspaceIdentity {
    pub(super) fn from_scope(scope: &crate::agent::ShellScope<'_>) -> Option<Self> {
        use cap_fs_ext::MetadataExt;
        // SAME retained invocation directory. No path reopen/canonicalization,
        // caller root/identity, policy mutation or handle retained in the view.
        let directory = scope.prepared().filesystem().discovery_directory().ok()?;
        let metadata = directory.dir_metadata().ok()?;
        Self::from_fields(metadata.dev(), metadata.ino(), metadata.is_dir())
    }
    fn from_fields(device: u64, inode: u64, is_directory: bool) -> Option<Self> {
        if !is_directory || inode == 0 {
            return None;
        }
        Some(Self {
            device: u32::try_from(device).ok()?,
            inode,
        })
    }
}

const MAX_THREADS: usize = 8;
// Apple's public sys/proc_info.h: list pthread addresses, paired with the
// existing libc PROC_PIDTHREADINFO (NOT the distinct thread-id64 flavor).
// The pinned libc does not expose this constant; no dependency change needed.
const PROC_PIDLISTTHREADS: i32 = 6;

struct Identity {
    pid: u32,
    start: (u64, u64),
}
impl Identity {
    fn current(info: &libc::proc_bsdinfo, pid: u32) -> Option<Self> {
        if info.pbi_pid != pid || info.pbi_ppid != std::process::id() || info.pbi_pgid != pid {
            return None;
        }
        Some(Self {
            pid,
            start: (info.pbi_start_tvsec, info.pbi_start_tvusec),
        })
    }
    fn matches(&self, info: &libc::proc_bsdinfo) -> bool {
        Self::current(info, self.pid).is_some_and(|other| other.start == self.start)
    }
}

fn complete_return(received: i32, expected: usize) -> bool {
    usize::try_from(received).ok() == Some(expected)
}
fn task_info(pid: i32) -> Option<libc::proc_taskallinfo> {
    let mut info = std::mem::MaybeUninit::<libc::proc_taskallinfo>::uninit();
    let size = std::mem::size_of::<libc::proc_taskallinfo>();
    let size_arg = i32::try_from(size).ok()?;
    // SAFETY: correct native output type/flavor, writable size/alignment.
    // No field is read until the complete returned size is verified.
    let received = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTASKALLINFO,
            0,
            info.as_mut_ptr().cast(),
            size_arg,
        )
    };
    if !complete_return(received, size) {
        return None;
    }
    // SAFETY: exact complete kernel-written integer/array struct above.
    Some(unsafe { info.assume_init() })
}
fn bsd_info(pid: i32) -> Option<libc::proc_bsdinfo> {
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::uninit();
    let size = std::mem::size_of::<libc::proc_bsdinfo>();
    let size_arg = i32::try_from(size).ok()?;
    // SAFETY: this flavor writes exactly proc_bsdinfo to the provided storage.
    let received = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size_arg,
        )
    };
    if !complete_return(received, size) {
        return None;
    }
    // SAFETY: exact complete kernel-written integer/array struct above.
    Some(unsafe { info.assume_init() })
}
fn thread_info(pid: i32, address: u64) -> Option<libc::proc_threadinfo> {
    let mut info = std::mem::MaybeUninit::<libc::proc_threadinfo>::uninit();
    let size = std::mem::size_of::<libc::proc_threadinfo>();
    let size_arg = i32::try_from(size).ok()?;
    // SAFETY: the argument is only an original bounded list entry, paired
    // with PROC_PIDTHREADINFO, and the output is the exact native struct.
    let received = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTHREADINFO,
            address,
            info.as_mut_ptr().cast(),
            size_arg,
        )
    };
    if !complete_return(received, size) {
        return None;
    }
    // SAFETY: exact complete kernel-written integer/array struct above.
    Some(unsafe { info.assume_init() })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ListProblem {
    Unavailable,
    Overbound,
    Ambiguous,
}
fn returned_slots(received: i32) -> Result<usize, ListProblem> {
    let bytes = usize::try_from(received).map_err(|_| ListProblem::Unavailable)?;
    let width = std::mem::size_of::<u64>();
    if bytes == 0 || bytes > (MAX_THREADS + 1) * width || bytes % width != 0 {
        return Err(ListProblem::Unavailable);
    }
    let count = bytes / width;
    if count > MAX_THREADS {
        return Err(ListProblem::Overbound);
    }
    Ok(count)
}
fn unique_slots(slots: &[u64]) -> Result<(), ListProblem> {
    for (index, key) in slots.iter().enumerate() {
        if slots[..index].contains(key) {
            return Err(ListProblem::Ambiguous);
        }
    }
    Ok(())
}
fn thread_addresses(pid: i32) -> Result<Vec<u64>, ListProblem> {
    // One extra slot distinguishes a full bounded set from >=9 entries.
    // The array is initialized; no native return can expose uninitialized data.
    let mut slots = [0u64; MAX_THREADS + 1];
    let size = std::mem::size_of_val(&slots);
    // SAFETY: fixed native u64 array with exact bounded writable byte count.
    // No nullptr size query, growing allocation, retry or raw address rendering.
    let received = unsafe {
        libc::proc_pidinfo(
            pid,
            PROC_PIDLISTTHREADS,
            0,
            slots.as_mut_ptr().cast(),
            size as i32,
        )
    };
    let count = returned_slots(received)?;
    unique_slots(&slots[..count])?;
    Ok(slots[..count].to_vec())
}

struct ThreadFacts {
    user_time_native: u64,
    system_time_native: u64,
    cpu_usage_native: i32,
    run_state: i32,
    flags: i32,
    sleep_time_native: i32,
    scheduling: scheduling::ThreadScheduling,
}
impl ThreadFacts {
    fn from_info(info: &libc::proc_threadinfo) -> Self {
        // Do not copy/retain the kernel-supplied name or original pthread address.
        Self {
            user_time_native: info.pth_user_time,
            system_time_native: info.pth_system_time,
            cpu_usage_native: info.pth_cpu_usage,
            run_state: info.pth_run_state,
            flags: info.pth_flags,
            sleep_time_native: info.pth_sleep_time,
            scheduling: scheduling::ThreadScheduling::from_info(info),
        }
    }
    fn append(&self, report: &mut String, sequence: u64, ordinal: usize) {
        let _ = write!(
            report,
            "\ncall={sequence} native_thread={ordinal} user_time_native={} system_time_native={} cpu_usage_native={} run_state={} flags={} sleep_time_native={}",
            self.user_time_native,
            self.system_time_native,
            self.cpu_usage_native,
            self.run_state,
            self.flags,
            self.sleep_time_native
        );
    }
}
fn image_class(info: &libc::proc_bsdinfo) -> &'static str {
    let name: Vec<u8> = info
        .pbi_comm
        .iter()
        .copied()
        .take_while(|c| *c != 0)
        .map(|c| c as u8)
        .collect();
    match name.as_slice() {
        b"sh" | b"bash" | b"zsh" => "shell",
        b"sleep" => "sleep",
        _ => "other",
    }
}
fn append_task(report: &mut String, info: &libc::proc_taskinfo, sequence: u64) {
    // These are original kernel-native values, not a wall-time/CPU-rate
    // calculation, wait-channel explanation, model Usage or completion status.
    let _ = write!(
        report,
        "\ncall={sequence} native_task=observed total_user_native={} total_system_native={} threads_user_native={} threads_system_native={} thread_count={} running_threads={} context_switches={} unix_syscalls={} mach_syscalls={} faults={} pageins={}",
        info.pti_total_user,
        info.pti_total_system,
        info.pti_threads_user,
        info.pti_threads_system,
        info.pti_threadnum,
        info.pti_numrunning,
        info.pti_csw,
        info.pti_syscalls_unix,
        info.pti_syscalls_mach,
        info.pti_faults,
        info.pti_pageins
    );
}
fn append_threads(
    report: &mut String,
    sequence: u64,
    threads: Result<Vec<ThreadFacts>, ListProblem>,
    same_child_after_queries: bool,
) {
    // Subqueries are not atomic. Discard their facts if original PID/start/
    // parent/group cannot be revalidated, even when the subquery itself failed.
    if !same_child_after_queries {
        let _ = write!(
            report,
            "\ncall={sequence} native_threads=changed-or-unavailable"
        );
        return;
    }
    match threads {
        Ok(facts) => {
            let _ = write!(
                report,
                "\ncall={sequence} native_threads=observed count={}",
                facts.len()
            );
            for (index, fact) in facts.iter().enumerate() {
                fact.append(report, sequence, index);
                fact.scheduling.append(report, sequence, index);
            }
        }
        Err(problem) => {
            let reason = match problem {
                ListProblem::Unavailable => "unavailable",
                ListProblem::Overbound => "overbound",
                ListProblem::Ambiguous => "ambiguous",
            };
            let _ = write!(report, "\ncall={sequence} native_threads={reason}");
        }
    }
}

pub(super) fn failed_child_state(
    sequence: u64,
    pid: u32,
    workspace: Option<WorkspaceIdentity>,
) -> String {
    let Ok(pid_arg) = i32::try_from(pid) else {
        return format!("\ncall={sequence} native_process=unavailable");
    };
    let Some(info) = task_info(pid_arg) else {
        return format!("\ncall={sequence} native_process=unavailable");
    };
    let Some(identity) = Identity::current(&info.pbsd, pid) else {
        return format!("\ncall={sequence} native_process=not-owned-current-child");
    };
    let mut report = format!(
        "\ncall={sequence} native_process=owned-child image_class={} kernel_status={} kernel_flags={} child_group_is_pid=true",
        image_class(&info.pbsd),
        info.pbsd.pbi_status,
        info.pbsd.pbi_flags
    );
    append_task(&mut report, &info.ptinfo, sequence);
    let execution = execution::observe(pid_arg, workspace);
    let addresses = thread_addresses(pid_arg);
    // Reserve against the WHOLE original bounded list, even if a subsequent
    // thread query fails. Never refund failed work to add another native query.
    let max_thread_queries = addresses.as_ref().map_or(0, Vec::len);
    let threads = addresses.and_then(|addresses| {
        addresses
            .into_iter()
            .map(|address| {
                thread_info(pid_arg, address)
                    .map(|info| ThreadFacts::from_info(&info))
                    .ok_or(ListProblem::Unavailable)
            })
            .collect()
    });
    let child_pipes = pipes::observe(pid_arg, max_thread_queries);
    let same_child_after_queries = bsd_info(pid_arg).is_some_and(|later| identity.matches(&later));
    execution::append(&mut report, sequence, execution, same_child_after_queries);
    scheduling::append_process(&mut report, sequence, &info, same_child_after_queries);
    append_threads(&mut report, sequence, threads, same_child_after_queries);
    pipes::append(&mut report, sequence, child_pipes, same_child_after_queries);
    // SAME <=13 query cap: 5 fixed + <=8 thread calls. Two read-only current
    // child-pipe queries run ONLY when <=6 thread calls fit, otherwise refuse
    // that facet. No retry/refund/signal/poll/handle/clock/process policy change.
    report
}

#[cfg(test)]
mod tests;
