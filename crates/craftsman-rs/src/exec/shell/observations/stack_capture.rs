//! Explicit user-approved ONE external native trace of an actual owned child.
//! TEST/macOS-only; no unmarked/default/production sampler or caller-supplied PID.
//! Sampling suspends/resumes threads: this is perturbing diagnostic evidence,
//! not an original wait-cause/exec/EOF/usage/authority or OS deadline guarantee.

use super::{Call, lock};
use crate::platform_os::ProcessTree;
use craftsman_harness::ToolErrorKind;
use std::ffi::{OsStr, OsString};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::os::unix::process::ExitStatusExt;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;

const TARGET: &str = "agent::shell_tests::actual_scoped_shell_poll_progress_retains_reader_return_then_wait_and_original_output";
const MARKER: &str = "__craftsman_owned_child_stack_v1=";
const MAX_ARGUMENTS: usize = 64;
const MAX_ARGUMENT_BYTES: usize = 65536;
const MAX_PATH_BYTES: usize = 16384;
const MAX_TRACE_BYTES: usize = 1024 * 1024;
const MAX_TOOL_PIPE_BYTES: u64 = 8192;
const TOOL_WALL: Duration = Duration::from_secs(5);
const TOOL_CLEANUP: Duration = Duration::from_secs(2);
// One budget for this marked native test process; never reset/refund on errors.
static ATTEMPTED: AtomicBool = AtomicBool::new(false);

fn select(arguments: &[OsString]) -> Result<Option<PathBuf>, ()> {
    if arguments.len() > MAX_ARGUMENTS {
        return Err(());
    }
    let mut total = 0usize;
    let mut selected = None;
    for argument in arguments {
        total = total
            .checked_add(argument.as_encoded_bytes().len())
            .ok_or(())?;
        if total > MAX_ARGUMENT_BYTES {
            return Err(());
        }
        let text = argument.to_str().ok_or(())?;
        let Some(root) = text.strip_prefix(MARKER) else {
            continue;
        };
        if selected.is_some() || root.is_empty() || root.len() > MAX_PATH_BYTES {
            return Err(());
        }
        if root.bytes().any(|v| v < 32 || v == 127) {
            return Err(());
        }
        let root = PathBuf::from(root);
        if !root.is_absolute()
            || root
                .components()
                .any(|part| !matches!(part, Component::RootDir | Component::Normal(_)))
            || root.file_name() != Some(OsStr::new("capture"))
            || !root
                .parent()
                .and_then(Path::file_name)
                .and_then(OsStr::to_str)
                .is_some_and(|name| name.starts_with("owned-child-stack-capture-"))
        {
            return Err(());
        }
        selected = Some(root);
    }
    Ok(selected)
}
fn current_target(name: Option<&str>) -> bool {
    name == Some(TARGET)
}
fn directory(root: &Path) -> bool {
    std::fs::symlink_metadata(root).is_ok_and(|info| {
        info.is_dir() && info.mode() & 0o777 == 0o700
            // SAFETY: read-only current effective user identity, not target IO.
            && info.uid() == unsafe { libc::geteuid() }
    })
}
fn identity(info: &std::fs::Metadata) -> Option<(u64, u64)> {
    (info.is_file() && info.nlink() == 1 && info.mode() & 0o777 == 0o600)
        .then_some((info.dev(), info.ino()))
}
fn owned_empty(root: &Path, name: &str) -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .write(true)
        .read(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(root.join(name))
}
fn digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}
fn read_owned(file: &mut std::fs::File, expected: (u64, u64)) -> Option<Vec<u8>> {
    let before = file.metadata().ok()?;
    if identity(&before) != Some(expected) || before.len() > MAX_TRACE_BYTES as u64 {
        return None;
    }
    file.seek(SeekFrom::Start(0)).ok()?;
    let mut bytes = Vec::new();
    Read::take(&mut *file, MAX_TRACE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > MAX_TRACE_BYTES || identity(&file.metadata().ok()?) != Some(expected) {
        return None;
    }
    Some(bytes)
}

pub(super) async fn capture_if_marked(
    call: &Call,
    tree: &ProcessTree,
    stop: Option<ToolErrorKind>,
) {
    if stop != Some(ToolErrorKind::Timeout) || !current_target(std::thread::current().name()) {
        return;
    }
    let arguments: Vec<_> = std::env::args_os().take(MAX_ARGUMENTS + 1).collect();
    let root = match select(&arguments) {
        Ok(Some(root)) => root,
        Ok(None) => return,
        Err(()) => {
            eprintln!("owned-stack-capture selection=refused");
            return;
        }
    };
    // ONLY this still-borrowed, unreaped local tree mints the target. No saved
    // PID lookup, name-based process search, Task port/API/native-bound change.
    let Some(pid) = tree.id().filter(|pid| *pid > 1) else {
        return;
    };
    let sequence = {
        let state = lock(&call.state);
        state
            .entries
            .iter()
            .find(|entry| Some(entry.sequence) == call.sequence)
            .filter(|entry| {
                entry.scope_end_us.is_none()
                    && !entry.launch_failed
                    && entry.pid == Some(pid)
                    && entry.before_stop.is_some()
                    && entry.pipes.iter().all(|pipe| {
                        pipe.first_poll_us.is_some()
                            && pipe.bytes == 0
                            && !pipe.eof
                            && !pipe.read_failed
                    })
            })
            .map(|entry| entry.sequence)
    };
    let Some(sequence) = sequence else { return };
    if ATTEMPTED
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        eprintln!("owned-stack-capture budget=already-consumed");
        return;
    }
    if !directory(&root) {
        eprintln!("owned-stack-capture destination=refused budget=consumed");
        return;
    }
    let Ok(mut trace) = owned_empty(&root, "owned-child-stack.sample.txt") else {
        eprintln!("owned-stack-capture trace-create=refused budget=consumed");
        return;
    };
    let Some(trace_identity) = trace.metadata().ok().as_ref().and_then(identity) else {
        return;
    };
    let Ok(mut record) = owned_empty(&root, "owned-stack-capture.json") else {
        return;
    };
    let started = Instant::now();
    let entered_us = u64::try_from(call.started.elapsed().as_micros()).ok();
    let mut command = tokio::process::Command::new("/usr/bin/sample");
    command
        .arg(pid.to_string())
        .args(["1", "1000", "-mayDie", "-file"])
        .arg(root.join("owned-child-stack.sample.txt"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    // SAFETY: child-only single syscall before exec; never parent/target limits,
    // env, clocks, signals, launcher settings or real SDK/user config changes.
    unsafe {
        command.pre_exec(|| {
            let limit = libc::rlimit {
                rlim_cur: MAX_TRACE_BYTES as libc::rlim_t,
                rlim_max: MAX_TRACE_BYTES as libc::rlim_t,
            };
            if libc::setrlimit(libc::RLIMIT_FSIZE, &limit) == 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error())
            }
        });
    }
    let mut tool_returned = false;
    let mut tool_signal = None;
    let mut tool_exit = None;
    let mut tool_stdout = None;
    let mut tool_stderr = None;
    let mut cleanup_returned = false;
    let mut cleanup_signal = None;
    let mut cleanup_exit = None;
    let status = match command.spawn() {
        Err(_) => "spawn-error",
        Ok(mut child) => {
            let stdout = child.stdout.take().expect("fixed diagnostic stdout pipe");
            let stderr = child.stderr.take().expect("fixed diagnostic stderr pipe");
            let mut stdout = stdout.take(MAX_TOOL_PIPE_BYTES + 1);
            let mut stderr = stderr.take(MAX_TOOL_PIPE_BYTES + 1);
            let mut out = Vec::new();
            let mut err = Vec::new();
            let operation = async {
                tokio::try_join!(
                    child.wait(),
                    stdout.read_to_end(&mut out),
                    stderr.read_to_end(&mut err)
                )
            };
            match tokio::time::timeout(TOOL_WALL, operation).await {
                Ok(Ok((exit, _, _))) => {
                    tool_returned = true;
                    tool_signal = exit.signal();
                    tool_exit = exit.code();
                    if out.len() <= MAX_TOOL_PIPE_BYTES as usize
                        && err.len() <= MAX_TOOL_PIPE_BYTES as usize
                    {
                        tool_stdout = Some(out);
                        tool_stderr = Some(err);
                        if exit.success() {
                            "tool-success"
                        } else {
                            "tool-error"
                        }
                    } else {
                        "tool-pipe-bound"
                    }
                }
                other => {
                    let timed_out = other.is_err();
                    let _ = child.start_kill();
                    if let Ok(Ok(exit)) = tokio::time::timeout(TOOL_CLEANUP, child.wait()).await {
                        cleanup_returned = true;
                        cleanup_signal = exit.signal();
                        cleanup_exit = exit.code();
                    }
                    if timed_out {
                        "execution-timeout"
                    } else {
                        "tool-io-error"
                    }
                }
            }
        }
    };
    // The owning call/tree STILL lives across tool completion/owned cleanup.
    // A target may exit/exec; no original-script/immutable-state/cause promise.
    let target_id_still_held = tree.id() == Some(pid);
    let bytes = read_owned(&mut trace, trace_identity);
    let trace_path_still_owned =
        std::fs::symlink_metadata(root.join("owned-child-stack.sample.txt"))
            .ok()
            .as_ref()
            .and_then(identity)
            == Some(trace_identity);
    for (name, bytes) in [
        ("sample-stdout.txt", &tool_stdout),
        ("sample-stderr.txt", &tool_stderr),
    ] {
        if let Some(bytes) = bytes {
            if let Ok(mut file) = owned_empty(&root, name) {
                let _ = file.write_all(bytes);
            }
        }
    }
    let facts = serde_json::json!({
        "capture_attempt":1,"scope_call_sequence":sequence,"actual_owned_target_pid":pid,
        "only_exact_marked_native_test":TARGET,"stop_kind":"Timeout",
        "original_call_elapsed_enter_us":entered_us,
        "original_call_elapsed_return_us":u64::try_from(call.started.elapsed().as_micros()).ok(),
        "capture_elapsed_us":u64::try_from(started.elapsed().as_micros()).ok(),
        "tool":"sample","duration_argument_seconds":1,"interval_argument_milliseconds":1000,
        "tool_wall_seconds":5,"owned_tool_cleanup_seconds":2,"tool_status":status,
        "tool_returned":tool_returned,"tool_exit_code":tool_exit,"tool_exit_signal":tool_signal,
        "cleanup_returned":cleanup_returned,"cleanup_exit_code":cleanup_exit,"cleanup_exit_signal":cleanup_signal,
        "tool_stdout_bytes":tool_stdout.as_ref().map(Vec::len),
        "tool_stderr_bytes":tool_stderr.as_ref().map(Vec::len),
        "target_id_still_held_before_original_kill":target_id_still_held,
        "trace_path_still_owned":trace_path_still_owned,
        "trace_bytes":bytes.as_ref().map(Vec::len),"trace_sha256":bytes.as_ref().map(|v|digest(v)),
        "not_passive_sampling_suspends_and_resumes_threads":true,
        "original_native13_query_8_thread_4096_byte_shared_ticket_unchanged":true,
        "not_original_wait_cause_exec_EOF_usage_authority_or_OS_deadline_proof":true,
        "no_retry_budget_consumed_even_on_error":true
    });
    if serde_json::to_writer_pretty(&mut record, &facts).is_ok() {
        let _ = record.write_all(b"\n");
    }
    eprintln!(
        "owned-stack-capture call={sequence} actual_owned_pid={pid} status={status} target_id_still_held={target_id_still_held} trace_path_still_owned={trace_path_still_owned}"
    );
}

#[cfg(test)]
mod tests;
