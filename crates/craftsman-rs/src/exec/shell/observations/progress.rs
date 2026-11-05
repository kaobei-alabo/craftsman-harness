//! TEST-only facts from ORIGINAL reader polls and existing selection branches.
//! No wrapped waker, native query, byte retention, startup or wait-cause claim.

use super::{Call, State, lock};
use craftsman_harness::ToolErrorKind;
use std::fmt::{self, Write};
use std::task::Poll;

const MAX_REPORT_BYTES: usize = 4096;

#[derive(Clone, Copy)]
enum ReadReturn {
    Pending,
    Data,
    Eof,
    EmptyCapacity,
    Error,
}
impl ReadReturn {
    fn label(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Data => "data",
            Self::Eof => "eof",
            Self::EmptyCapacity => "empty-capacity",
            Self::Error => "error",
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct ReadProgress {
    polls: Option<u64>,
    pending: Option<u64>,
    last_us: Option<u64>,
    last: Option<ReadReturn>,
}
impl Default for ReadProgress {
    fn default() -> Self {
        Self {
            polls: Some(0),
            pending: Some(0),
            last_us: None,
            last: None,
        }
    }
}
impl ReadProgress {
    pub(super) fn observe(
        &mut self,
        now: u64,
        result: &Poll<std::io::Result<()>>,
        has_data: bool,
        had_capacity: bool,
    ) {
        self.polls = self.polls.and_then(|value| value.checked_add(1));
        self.last_us = Some(now);
        self.last = Some(match result {
            Poll::Pending => {
                self.pending = self.pending.and_then(|value| value.checked_add(1));
                ReadReturn::Pending
            }
            Poll::Ready(Err(_)) => ReadReturn::Error,
            Poll::Ready(Ok(())) if has_data => ReadReturn::Data,
            Poll::Ready(Ok(())) if had_capacity => ReadReturn::Eof,
            Poll::Ready(Ok(())) => ReadReturn::EmptyCapacity,
        });
    }
}

#[derive(Clone, Copy)]
enum Selected {
    ReturnedOk,
    ReturnedError,
    ReturnedStatus,
    Timeout,
    Approval,
    OtherStop,
}
impl Selected {
    fn stop(stop: Option<ToolErrorKind>) -> Self {
        match stop {
            Some(ToolErrorKind::Timeout) => Self::Timeout,
            Some(ToolErrorKind::NeedsApproval) => Self::Approval,
            _ => Self::OtherStop,
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::ReturnedOk => "returned-ok",
            Self::ReturnedError => "returned-error",
            Self::ReturnedStatus => "returned-status",
            Self::Timeout => "timeout-branch",
            Self::Approval => "approval-branch",
            Self::OtherStop => "other-stop-branch",
        }
    }
}
#[derive(Clone, Copy, Default)]
struct Selection {
    enter_us: Option<u64>,
    return_us: Option<u64>,
    result: Option<Selected>,
    loss: bool,
}
impl Selection {
    fn enter(&mut self, now: u64) {
        if self.enter_us.is_some() {
            self.loss = true;
        } else {
            self.enter_us = Some(now);
        }
    }
    fn returned(&mut self, now: u64, result: Selected) {
        if self.enter_us.is_none() || self.return_us.is_some() {
            self.loss = true;
            return;
        }
        self.return_us = Some(now);
        self.result = Some(result);
    }
}
#[derive(Clone, Copy, Default)]
pub(super) struct CallProgress {
    pub(super) reads: [ReadProgress; 2],
    readers: Selection,
    wait: Selection,
}
impl Call {
    pub(in crate::exec::shell) fn readers_selected(&self) {
        self.update(|entry, now| entry.progress.readers.enter(now));
    }
    pub(in crate::exec::shell) fn readers_returned(
        &self,
        returned: bool,
        ok: bool,
        stop: Option<ToolErrorKind>,
    ) {
        let result = if returned {
            if ok {
                Selected::ReturnedOk
            } else {
                Selected::ReturnedError
            }
        } else {
            Selected::stop(stop)
        };
        self.update(|entry, now| entry.progress.readers.returned(now, result));
    }
    pub(in crate::exec::shell) fn wait_selected(&self) {
        self.update(|entry, now| entry.progress.wait.enter(now));
    }
    pub(in crate::exec::shell) fn wait_returned(
        &self,
        status: &Option<std::io::Result<std::process::ExitStatus>>,
        stop: Option<ToolErrorKind>,
    ) {
        let result = match status {
            Some(Ok(_)) => Selected::ReturnedStatus,
            Some(Err(_)) => Selected::ReturnedError,
            None => Selected::stop(stop),
        };
        self.update(|entry, now| entry.progress.wait.returned(now, result));
    }
}

struct Bounded(String);
impl Write for Bounded {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        if self
            .0
            .len()
            .checked_add(text.len())
            .is_none_or(|len| len > MAX_REPORT_BYTES)
        {
            return Err(fmt::Error);
        }
        self.0.push_str(text);
        Ok(())
    }
}
enum Phase {
    CurrentHistory,
    #[cfg(target_os = "macos")]
    BeforeStopHistory,
}
impl Phase {
    fn label(self) -> &'static str {
        match self {
            Self::CurrentHistory => "current-history",
            #[cfg(target_os = "macos")]
            Self::BeforeStopHistory => "before-stop-history",
        }
    }
}
fn write_call(
    out: &mut Bounded,
    sequence: u64,
    phase: Phase,
    progress: &CallProgress,
) -> fmt::Result {
    let phase = phase.label();
    let readers = &progress.readers;
    let wait = &progress.wait;
    write!(
        out,
        "\ncall={sequence} poll_progress_phase={phase} readers_enter_us={:?} readers_return_us={:?} readers_result={} readers_phase_loss={} wait_enter_us={:?} wait_return_us={:?} wait_result={} wait_phase_loss={}",
        readers.enter_us,
        readers.return_us,
        readers.result.map_or("not-returned", Selected::label),
        readers.loss,
        wait.enter_us,
        wait.return_us,
        wait.result.map_or("not-returned", Selected::label),
        wait.loss
    )?;
    for (label, read) in ["stdout", "stderr"].into_iter().zip(&progress.reads) {
        write!(
            out,
            "\ncall={sequence} poll_progress_phase={phase} {label}_polls={:?} {label}_pending_polls={:?} {label}_last_poll_us={:?} {label}_last_poll_result={} {label}_count_loss={}",
            read.polls,
            read.pending,
            read.last_us,
            read.last.map_or("not-polled", ReadReturn::label),
            read.polls.is_none() || read.pending.is_none()
        )?;
    }
    Ok(())
}
pub(super) fn report(state: &State) -> String {
    let mut out = Bounded(String::with_capacity(MAX_REPORT_BYTES));
    let result = (|| {
        write!(
            &mut out,
            "native-shell-test-poll-progress calls={} omitted={} recorder_loss={} clock=call",
            state.next, state.omitted, state.loss
        )?;
        for entry in &state.entries {
            write_call(
                &mut out,
                entry.sequence,
                Phase::CurrentHistory,
                &entry.progress,
            )?;
            #[cfg(target_os = "macos")]
            if let Some(stop) = &entry.before_stop {
                write_call(
                    &mut out,
                    entry.sequence,
                    Phase::BeforeStopHistory,
                    &stop.progress,
                )?;
            }
        }
        Ok::<_, fmt::Error>(())
    })();
    if result.is_err()
        || !out
            .0
            .bytes()
            .all(|byte| byte == b'\n' || (32..=126).contains(&byte))
    {
        "native-shell-test-poll-progress=refused".to_owned()
    } else {
        out.0
    }
}
pub(super) fn append(report: &mut String, state: &State) {
    report.push('\n');
    report.push_str(&self::report(state));
}
impl super::ObservationView {
    pub(crate) fn poll_progress_report_for_tests(&self) -> String {
        report(&lock(&self.0))
    }
}

#[cfg(test)]
mod tests;
