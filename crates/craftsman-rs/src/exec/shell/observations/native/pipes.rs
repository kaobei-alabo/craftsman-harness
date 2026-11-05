//! Read-only CURRENT child fd 1/2 pipe metadata, never captured/output bytes.
//! Use only the original live-child snapshot ticket and final PID revalidation.

use super::complete_return;
use std::fmt::Write;

const MAX_NATIVE_QUERIES: usize = 13;
// Original task/path/CWD/list/final BSD identity queries remain unchanged.
const FIXED_NATIVE_QUERIES: usize = 5;
const PIPE_QUERIES: usize = 2;
const PROC_PIDFDPIPEINFO: i32 = 6;
const DTYPE_PIPE: i32 = 6;
const FWRITE: u32 = 2;

// Apple's public sys/proc_info.h. The pinned libc exposes proc_pidfdinfo and
// vinfo_stat but not these two wrapper types. No dependency/feature change.
#[repr(C)]
struct FileInfo {
    open_flags: u32,
    status: u32,
    offset: libc::off_t,
    kind: i32,
    guard_flags: u32,
}
#[repr(C)]
struct PipeInfo {
    stat: libc::vinfo_stat,
    handle: u64,
    peer_handle: u64,
    status: i32,
    reserved: i32,
}
#[repr(C)]
struct PipeFdInfo {
    file: FileInfo,
    pipe: PipeInfo,
}

pub(super) fn within_original_query_bound(max_thread_queries: usize) -> bool {
    FIXED_NATIVE_QUERIES
        .checked_add(max_thread_queries)
        .and_then(|count| count.checked_add(PIPE_QUERIES))
        .is_some_and(|count| count <= MAX_NATIVE_QUERIES)
}

pub(super) struct Pipe {
    buffer_bytes_native: u64,
    capacity_native: i32,
    open_flags_native: u32,
    pipe_status_native: i32,
}
fn fields(info: &PipeFdInfo) -> Option<Pipe> {
    // The flavor itself checks DTYPE_PIPE; additionally verify the complete
    // copied type/write grant/FIFO stat. Never turn a rebound non-pipe fd into
    // an original stdout authority or convert negative counts to zero.
    if info.file.kind != DTYPE_PIPE
        || info.file.open_flags & FWRITE == 0
        || info.pipe.stat.vst_mode & libc::S_IFMT != libc::S_IFIFO
        || info.pipe.stat.vst_blksize <= 0
    {
        return None;
    }
    Some(Pipe {
        buffer_bytes_native: u64::try_from(info.pipe.stat.vst_size).ok()?,
        capacity_native: info.pipe.stat.vst_blksize,
        open_flags_native: info.file.open_flags,
        pipe_status_native: info.pipe.status,
    })
}
fn pipe(pid: i32, descriptor: i32) -> Option<Pipe> {
    let mut info = std::mem::MaybeUninit::<PipeFdInfo>::uninit();
    let size = std::mem::size_of::<PipeFdInfo>();
    let size_arg = i32::try_from(size).ok()?;
    // SAFETY: the current owned child's FIXED descriptor and exact public
    // native flavor/output layout; writable size/alignment. No field is read
    // until the WHOLE returned struct size is verified. No handle/byte read.
    let received = unsafe {
        libc::proc_pidfdinfo(
            pid,
            descriptor,
            PROC_PIDFDPIPEINFO,
            info.as_mut_ptr().cast(),
            size_arg,
        )
    };
    if !complete_return(received, size) {
        return None;
    }
    // SAFETY: complete kernel-written integer/array structs above. Raw opaque
    // handles, inode/device/identity/timestamps never enter retained facts.
    fields(&unsafe { info.assume_init() })
}

pub(super) enum Facts {
    QueryBound,
    Queried {
        stdout: Option<Pipe>,
        stderr: Option<Pipe>,
    },
}
pub(super) fn observe(pid: i32, max_thread_queries: usize) -> Facts {
    if !within_original_query_bound(max_thread_queries) {
        return Facts::QueryBound;
    }
    Facts::Queried {
        stdout: pipe(pid, libc::STDOUT_FILENO),
        stderr: pipe(pid, libc::STDERR_FILENO),
    }
}
pub(super) fn append(report: &mut String, sequence: u64, facts: Facts, same_child: bool) {
    if !same_child {
        let _ = write!(
            report,
            "\ncall={sequence} native_child_pipes=changed-or-unavailable"
        );
        return;
    }
    match facts {
        Facts::QueryBound => {
            let _ = write!(
                report,
                "\ncall={sequence} native_child_pipes=original-query-bound"
            );
        }
        Facts::Queried { stdout, stderr } => {
            for (label, descriptor, value) in [
                ("stdout", libc::STDOUT_FILENO, stdout),
                ("stderr", libc::STDERR_FILENO, stderr),
            ] {
                if let Some(value) = value {
                    let _ = write!(
                        report,
                        "\ncall={sequence} native_child_pipe={label} current_child_fd={descriptor} buffer_bytes_native={} capacity_native={} open_flags_native={} pipe_status_native={}",
                        value.buffer_bytes_native,
                        value.capacity_native,
                        value.open_flags_native,
                        value.pipe_status_native
                    );
                } else {
                    let _ = write!(
                        report,
                        "\ncall={sequence} native_child_pipe={label} current_child_fd={descriptor} metadata=unavailable"
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
