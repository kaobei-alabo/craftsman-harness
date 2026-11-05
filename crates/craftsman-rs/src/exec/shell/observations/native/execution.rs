//! Exact bounded native executable/CWD classifications, never path authority.

use super::{WorkspaceIdentity, complete_return};
use std::fmt::Write;

const PATH_BYTES: usize = libc::PROC_PIDPATHINFO_MAXSIZE as usize;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Executable {
    SystemSh,
    SystemBash,
    SystemZsh,
    SystemDash,
    SystemSleep,
    Other,
    Unavailable,
}
impl Executable {
    fn label(self) -> &'static str {
        match self {
            Self::SystemSh => "system-sh",
            Self::SystemBash => "system-bash",
            Self::SystemZsh => "system-zsh",
            Self::SystemDash => "system-dash",
            Self::SystemSleep => "system-sleep",
            Self::Other => "other",
            Self::Unavailable => "unavailable",
        }
    }
}
fn executable_result(buffer: &[u8; PATH_BYTES], returned: i32) -> Executable {
    // Apple's libproc returns strlen, EXCLUDING NUL, not the raw PIDINFO size.
    // Admit the WHOLE exact returned byte string, never a clipped known prefix.
    let Ok(length) = usize::try_from(returned) else {
        return Executable::Unavailable;
    };
    if length == 0
        || length >= buffer.len()
        || buffer[length] != 0
        || buffer[..length].contains(&0)
        || buffer[0] != b'/'
    {
        return Executable::Unavailable;
    }
    match &buffer[..length] {
        b"/bin/sh" => Executable::SystemSh,
        b"/bin/bash" => Executable::SystemBash,
        b"/bin/zsh" => Executable::SystemZsh,
        b"/bin/dash" => Executable::SystemDash,
        b"/bin/sleep" => Executable::SystemSleep,
        _ => Executable::Other,
    }
}
fn executable(pid: i32) -> Executable {
    // Fixed initialized native scratch; never copied into the report/state.
    let mut buffer = [0u8; PATH_BYTES];
    // SAFETY: correct fixed writable native byte buffer; the public libproc
    // contract terminates the path and returns its length excluding NUL.
    let returned =
        unsafe { libc::proc_pidpath(pid, buffer.as_mut_ptr().cast(), PATH_BYTES as u32) };
    executable_result(&buffer, returned)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Cwd {
    MatchesWorkspace,
    Different,
    Unavailable,
    WorkspaceUnavailable,
}
impl Cwd {
    fn label(self) -> &'static str {
        match self {
            Self::MatchesWorkspace => "matches-invocation-workspace",
            Self::Different => "different-from-invocation-workspace",
            Self::Unavailable => "unavailable",
            Self::WorkspaceUnavailable => "expected-workspace-unavailable",
        }
    }
}
fn cwd_result(stat: &libc::vinfo_stat, workspace: Option<WorkspaceIdentity>) -> Cwd {
    let Some(workspace) = workspace else {
        return Cwd::WorkspaceUnavailable;
    };
    if stat.vst_mode & libc::S_IFMT != libc::S_IFDIR || stat.vst_ino == 0 {
        return Cwd::Unavailable;
    }
    if stat.vst_dev == workspace.device && stat.vst_ino == workspace.inode {
        Cwd::MatchesWorkspace
    } else {
        Cwd::Different
    }
}
fn cwd(pid: i32, workspace: Option<WorkspaceIdentity>) -> Cwd {
    let mut info = std::mem::MaybeUninit::<libc::proc_vnodepathinfo>::uninit();
    let size = std::mem::size_of::<libc::proc_vnodepathinfo>();
    let Ok(size_arg) = i32::try_from(size) else {
        return Cwd::Unavailable;
    };
    // SAFETY: exact native flavor/output type, writable alignment/size. No
    // field access before the COMPLETE exact return. Embedded paths ignored.
    let returned = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDVNODEPATHINFO,
            0,
            info.as_mut_ptr().cast(),
            size_arg,
        )
    };
    if !complete_return(returned, size) {
        return Cwd::Unavailable;
    }
    // SAFETY: complete kernel-written integer/array struct verified above.
    let info = unsafe { info.assume_init() };
    cwd_result(&info.pvi_cdir.vip_vi.vi_stat, workspace)
}

pub(super) struct Facts {
    executable: Executable,
    cwd: Cwd,
}
pub(super) fn observe(pid: i32, workspace: Option<WorkspaceIdentity>) -> Facts {
    Facts {
        executable: executable(pid),
        cwd: cwd(pid, workspace),
    }
}
pub(super) fn append(report: &mut String, sequence: u64, facts: Facts, same_child: bool) {
    if !same_child {
        let _ = write!(
            report,
            "\ncall={sequence} native_execution=changed-or-unavailable"
        );
        return;
    }
    let _ = write!(
        report,
        "\ncall={sequence} native_execution=observed executable_class={} native_cwd={}",
        facts.executable.label(),
        facts.cwd.label()
    );
}

#[cfg(test)]
mod tests;
