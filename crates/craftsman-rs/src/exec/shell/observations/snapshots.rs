//! TEST-only one-shot native query ownership shared by active/pre-stop views.
//! Native calls occur outside recorder locking. No raw paths/error/byte text.

use super::{Call, Entry, PipeFacts, State, lock, native};
use craftsman_harness::ToolErrorKind;
use std::fmt::Write;
use std::sync::Mutex;
use std::time::Instant;

const MAX_SAVED_REPORT_BYTES: usize = 4096;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    ActiveView,
    BeforeStop,
}
impl Phase {
    fn label(self) -> &'static str {
        match self {
            Self::ActiveView => "active-view",
            Self::BeforeStop => "before-stop",
        }
    }
}
#[derive(Clone, Copy)]
enum StopKind {
    Timeout,
    Approval,
    Failed,
    Other,
}
impl StopKind {
    fn from_kind(kind: ToolErrorKind) -> Self {
        match kind {
            ToolErrorKind::Timeout => Self::Timeout,
            ToolErrorKind::NeedsApproval => Self::Approval,
            ToolErrorKind::Failed => Self::Failed,
            _ => Self::Other,
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::Approval => "needs-approval",
            Self::Failed => "failed",
            Self::Other => "other",
        }
    }
}
pub(super) struct BeforeStop {
    enter_us: u64,
    kind: StopKind,
    tree_pid_present: bool,
    tree_pid_matches_recorded: bool,
    pipes: [PipeFacts; 2],
    pub(super) progress: super::progress::CallProgress,
}
pub(super) struct Cached {
    phase: Phase,
    enter_us: u64,
    return_us: Option<u64>,
    report: Option<String>,
}
/// Non-clone claim for one ORIGINAL entry/PID. No caller/public constructor.
pub(super) struct Ticket {
    sequence: u64,
    pid: u32,
    started: Instant,
}
fn elapsed(started: Instant, state: &mut State) -> u64 {
    match u64::try_from(started.elapsed().as_micros()) {
        Ok(value) => value,
        Err(_) => {
            state.loss = true;
            u64::MAX
        }
    }
}
fn claim(entry: &mut Entry, phase: Phase, now: u64, started: Instant) -> Option<Ticket> {
    if entry.scope_end_us.is_some() || entry.native_snapshot.is_some() {
        return None;
    }
    let pid = entry.pid?;
    entry.native_snapshot = Some(Cached {
        phase,
        enter_us: now,
        return_us: None,
        report: None,
    });
    Some(Ticket {
        sequence: entry.sequence,
        pid,
        started,
    })
}
fn note(
    state: &mut State,
    sequence: Option<u64>,
    pid: Option<u32>,
    stop: Option<ToolErrorKind>,
    now: u64,
    started: Instant,
) -> Option<Ticket> {
    let kind = StopKind::from_kind(stop?);
    let entry = state
        .entries
        .iter_mut()
        .find(|entry| Some(entry.sequence) == sequence)?;
    if entry.scope_end_us.is_some() || entry.before_stop.is_some() {
        return None;
    }
    let same_pid = pid.is_some() && pid == entry.pid;
    entry.before_stop = Some(BeforeStop {
        enter_us: now,
        kind,
        tree_pid_present: pid.is_some(),
        tree_pid_matches_recorded: same_pid,
        pipes: entry.pipes,
        progress: entry.progress,
    });
    // None/mismatched current tree ID is NOT permission to query a saved PID.
    if !same_pid {
        return None;
    }
    claim(entry, Phase::BeforeStop, now, started)
}
pub(super) fn note_before_stop(
    call: &Call,
    pid: Option<u32>,
    stop: Option<ToolErrorKind>,
) -> Option<Ticket> {
    let mut state = lock(&call.state);
    let now = elapsed(call.started, &mut state);
    note(&mut state, call.sequence, pid, stop, now, call.started)
}
fn admitted_report(sequence: u64, report: String) -> String {
    if report.len() <= MAX_SAVED_REPORT_BYTES
        && report
            .bytes()
            .all(|c| c == b'\n' || (32..=126).contains(&c))
    {
        report
    } else {
        // Whole fixed refusal, never clip/echo a prefix to rescue it.
        format!("\ncall={sequence} native_snapshot_report=refused")
    }
}
fn finish(state: &mut State, ticket: &Ticket, report: String, returned: u64) {
    if let Some(entry) = state
        .entries
        .iter_mut()
        .find(|entry| entry.sequence == ticket.sequence)
    {
        if let Some(snapshot) = entry.native_snapshot.as_mut() {
            if snapshot.return_us.is_none() {
                snapshot.report = Some(admitted_report(ticket.sequence, report));
                snapshot.return_us = Some(returned);
            }
        }
    }
}
pub(super) fn capture(
    state: &Mutex<State>,
    ticket: Ticket,
    workspace: Option<native::WorkspaceIdentity>,
) {
    // FIRST reserve under lock, then perform the single <=13-call native query
    // outside it. Never inner-poll, native-call, await or sleep while locking.
    let report = native::failed_child_state(ticket.sequence, ticket.pid, workspace);
    let mut state = lock(state);
    let returned = elapsed(ticket.started, &mut state);
    finish(&mut state, &ticket, report, returned);
}
pub(super) fn capture_active(state: &Mutex<State>, workspace: Option<native::WorkspaceIdentity>) {
    let tickets = {
        let mut state = lock(state);
        state
            .entries
            .iter_mut()
            .filter(|entry| entry.before_stop.is_none())
            .filter_map(|entry| {
                // Existing active-view timing has its own monotonic origin; it is
                // labelled active-view, never relabelled fresh before-stop time.
                claim(entry, Phase::ActiveView, 0, Instant::now())
            })
            .collect::<Vec<_>>()
    };
    for ticket in tickets {
        capture(state, ticket, workspace);
    }
}
fn append_native(report: &mut String, entry: &Entry, phase_and_times: bool) {
    let Some(snapshot) = &entry.native_snapshot else {
        return;
    };
    if phase_and_times {
        let clock = match snapshot.phase {
            Phase::ActiveView => "active-query",
            Phase::BeforeStop => "call",
        };
        let _ = write!(
            report,
            "\ncall={} native_snapshot_phase={} native_snapshot_query_count=1 native_snapshot_clock={} native_snapshot_enter_us={} native_snapshot_return_us={:?}",
            entry.sequence,
            snapshot.phase.label(),
            clock,
            snapshot.enter_us,
            snapshot.return_us
        );
    }
    match &snapshot.report {
        Some(saved) => report.push_str(saved),
        None => {
            let _ = write!(report, "\ncall={} native_snapshot=pending", entry.sequence);
        }
    }
}
pub(super) fn append_active(report: &mut String, state: &State) {
    for entry in state
        .entries
        .iter()
        .filter(|entry| entry.scope_end_us.is_none())
    {
        append_native(report, entry, true);
    }
}
pub(super) fn append_teardown(report: &mut String, state: &State) {
    for entry in &state.entries {
        if let Some(stop) = &entry.before_stop {
            let _ = write!(
                report,
                "\ncall={} before_stop_enter_us={} before_stop_decision={} before_stop_tree_pid_present={} before_stop_tree_pid_matches_recorded={}",
                entry.sequence,
                stop.enter_us,
                stop.kind.label(),
                stop.tree_pid_present,
                stop.tree_pid_matches_recorded
            );
            for (label, pipe) in ["stdout", "stderr"].into_iter().zip(&stop.pipes) {
                let _ = write!(
                    report,
                    " before_stop_{label}_first_poll_us={:?} before_stop_{label}_first_chunk_us={:?} before_stop_{label}_bytes={} before_stop_{label}_eof={} before_stop_{label}_read_failed={}",
                    pipe.first_poll_us, pipe.first_chunk_us, pipe.bytes, pipe.eof, pipe.read_failed
                );
            }
            if entry.native_snapshot.is_none() {
                let _ = write!(
                    report,
                    "\ncall={} before_stop_native_query=not-started",
                    entry.sequence
                );
            }
        }
        append_native(report, entry, true);
    }
}

#[cfg(test)]
mod tests;
