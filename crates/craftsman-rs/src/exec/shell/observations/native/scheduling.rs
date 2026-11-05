//! TEST-only projection of fields in ALREADY complete native query buffers.
//! No syscall/PID/root/clock/permission authority, stage or wait-cause claim.

use std::fmt::Write;

// Apple's PUBLIC sys/proc_info.h, checked against retained official XNU commit
// f6217f891ac0bb64f3d375211650a4c1ff8ca1ea and the local SDK header. No private
// flag meaning is guessed. Preserve the original full numeric pbi_flags too.
const TRACED: u32 = 0x2;
const INEXIT: u32 = 0x4;
const PPWAIT: u32 = 0x8;
const LP64: u32 = 0x10;
const EXEC: u32 = 0x4000;
const RESOURCE_POLICY: u32 = 0x600;
const RESOURCE_ACTION: u32 = 0x1800;
const PUBLIC_DEFINED: u32 = 0x7fff;
const PROJECTED: u32 = TRACED | INEXIT | PPWAIT | LP64 | EXEC | RESOURCE_POLICY | RESOURCE_ACTION;

fn policy(flags: u32) -> &'static str {
    match flags & RESOURCE_POLICY {
        0 => "none-of-known-bits",
        0x200 => "throttle-policy",
        0x400 => "suspend-policy",
        0x600 => "terminate-policy",
        _ => unreachable!(),
    }
}
fn action(flags: u32) -> &'static str {
    match flags & RESOURCE_ACTION {
        0 => "none-of-known-bits",
        0x800 => "throttle-action",
        0x1000 => "suspend-action",
        _ => "unrecognized-combination",
    }
}
fn flag(flags: u32, mask: u32) -> bool {
    flags & mask == mask
}

pub(super) fn append_process(
    report: &mut String,
    sequence: u64,
    info: &libc::proc_taskallinfo,
    same_child_after_queries: bool,
) {
    if !same_child_after_queries {
        let _ = write!(
            report,
            "\ncall={sequence} native_scheduling=changed-or-unavailable"
        );
        return;
    }
    // Facts from the first full TASKALLINFO, not an atomic last-state sample.
    // EXEC is a kernel bit, not script entry/current executable/readiness proof.
    let flags = info.pbsd.pbi_flags;
    let _ = write!(
        report,
        "\ncall={sequence} native_scheduling=observed native_scheduling_source=initial-task-info nice_native={} task_thread_policy_native={} task_priority_native={} proc_exec_flag={} proc_parent_wait_flag={} proc_traced_flag={} proc_exit_flag={} proc_64bit_flag={} proc_resource_policy={} proc_resource_policy_bits_native={} proc_resource_action={} proc_resource_action_bits_native={} proc_other_public_bits_native={} proc_unclassified_bits_native={}",
        info.pbsd.pbi_nice,
        info.ptinfo.pti_policy,
        info.ptinfo.pti_priority,
        flag(flags, EXEC),
        flag(flags, PPWAIT),
        flag(flags, TRACED),
        flag(flags, INEXIT),
        flag(flags, LP64),
        policy(flags),
        flags & RESOURCE_POLICY,
        action(flags),
        flags & RESOURCE_ACTION,
        flags & PUBLIC_DEFINED & !PROJECTED,
        flags & !PUBLIC_DEFINED
    );
}

pub(super) struct ThreadScheduling {
    policy: i32,
    current_priority: i32,
    priority: i32,
    max_priority: i32,
}
impl ThreadScheduling {
    pub(super) fn from_info(info: &libc::proc_threadinfo) -> Self {
        // No kernel name, pthread key/address or later ambient query retained.
        Self {
            policy: info.pth_policy,
            current_priority: info.pth_curpri,
            priority: info.pth_priority,
            max_priority: info.pth_maxpriority,
        }
    }
    pub(super) fn append(&self, report: &mut String, sequence: u64, ordinal: usize) {
        let _ = write!(
            report,
            "\ncall={sequence} native_thread_scheduling={ordinal} thread_policy_native={} current_priority_native={} priority_native={} max_priority_native={}",
            self.policy, self.current_priority, self.priority, self.max_priority
        );
    }
}

#[cfg(test)]
mod tests;
