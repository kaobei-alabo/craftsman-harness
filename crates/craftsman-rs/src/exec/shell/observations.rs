//! Bounded TEST-ONLY observations of the actual scoped shell and delegated reads.
//! Not execution authority, an OS-exec witness, usage, public results or tracing.

use std::collections::VecDeque;
use std::fmt::Write;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll};
use std::time::Instant;
use tokio::io::{AsyncRead, ReadBuf};

mod progress;

const RETAINED_CALLS: usize = 8;
const RETAINED_SHELLS: usize = 8;

#[derive(Clone, Copy, Default)]
struct PipeFacts {
    first_poll_us: Option<u64>,
    first_chunk_us: Option<u64>,
    bytes: u64,
    eof: bool,
    read_failed: bool,
}
struct Entry {
    sequence: u64,
    launch_enter_us: Option<u64>,
    launch_return_us: Option<u64>,
    launch_failed: bool,
    pid: Option<u32>,
    pipes: [PipeFacts; 2],
    scope_end_us: Option<u64>,
    progress: progress::CallProgress,
    #[cfg(target_os = "macos")]
    native_snapshot: Option<snapshots::Cached>,
    #[cfg(target_os = "macos")]
    before_stop: Option<snapshots::BeforeStop>,
}
#[derive(Default)]
struct State {
    next: u64,
    omitted: u64,
    loss: bool,
    entries: VecDeque<Entry>,
}
fn lock(state: &Mutex<State>) -> MutexGuard<'_, State> {
    match state.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            let mut guard = poisoned.into_inner();
            guard.loss = true;
            guard
        }
    }
}

#[derive(Default)]
pub(super) struct Recorder(
    Arc<Mutex<State>>,
    #[cfg(target_os = "macos")] Option<native::WorkspaceIdentity>,
);
impl Recorder {
    pub(super) fn from_scope(scope: &crate::agent::ShellScope<'_>) -> Self {
        #[cfg(target_os = "macos")]
        {
            Self(
                Default::default(),
                native::WorkspaceIdentity::from_scope(scope),
            )
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = scope;
            Self::default()
        }
    }
    pub(super) fn begin(&self) -> Call {
        let mut state = lock(&self.0);
        let sequence = state.next.checked_add(1);
        if let Some(sequence) = sequence {
            state.next = sequence;
            if state.entries.len() == RETAINED_CALLS {
                state.entries.pop_front();
                state.omitted = state.omitted.saturating_add(1);
            }
            state.entries.push_back(Entry {
                sequence,
                launch_enter_us: None,
                launch_return_us: None,
                launch_failed: false,
                pid: None,
                pipes: [PipeFacts::default(); 2],
                scope_end_us: None,
                progress: Default::default(),
                #[cfg(target_os = "macos")]
                native_snapshot: None,
                #[cfg(target_os = "macos")]
                before_stop: None,
            });
        } else {
            state.loss = true;
        }
        Call {
            state: self.0.clone(),
            sequence,
            started: Instant::now(),
            #[cfg(target_os = "macos")]
            workspace: self.1,
        }
    }
    pub(super) fn view(&self) -> ObservationView {
        #[cfg(target_os = "macos")]
        {
            ObservationView(self.0.clone(), self.1)
        }
        #[cfg(not(target_os = "macos"))]
        {
            ObservationView(self.0.clone())
        }
    }
}

pub(super) struct Call {
    state: Arc<Mutex<State>>,
    sequence: Option<u64>,
    started: Instant,
    #[cfg(target_os = "macos")]
    workspace: Option<native::WorkspaceIdentity>,
}
impl Call {
    fn update(&self, update: impl FnOnce(&mut Entry, u64)) {
        let mut state = lock(&self.state);
        let elapsed = match u64::try_from(self.started.elapsed().as_micros()) {
            Ok(value) => value,
            Err(_) => {
                state.loss = true;
                u64::MAX
            }
        };
        if let Some(entry) = state
            .entries
            .iter_mut()
            .find(|e| Some(e.sequence) == self.sequence)
        {
            update(entry, elapsed);
        }
    }
    pub(super) fn launch_enter(&self) {
        self.update(|entry, elapsed| entry.launch_enter_us = Some(elapsed));
    }
    pub(super) fn launch_returned(&self, pid: Option<u32>) {
        self.update(|entry, elapsed| {
            entry.launch_return_us = Some(elapsed);
            entry.pid = pid;
        });
    }
    pub(super) fn launch_failed(&self) {
        self.update(|entry, elapsed| {
            entry.launch_return_us = Some(elapsed);
            entry.launch_failed = true;
        });
    }
    pub(super) fn reader<R: AsyncRead + Unpin>(
        &self,
        inner: R,
        stderr: bool,
    ) -> ObservedRead<'_, R> {
        ObservedRead {
            inner,
            call: self,
            index: usize::from(stderr),
        }
    }
}
impl Drop for Call {
    fn drop(&mut self) {
        // Scope release is NOT a normal-return/cancellation/completion fact.
        self.update(|entry, elapsed| entry.scope_end_us = Some(elapsed));
    }
}

pub(super) struct ObservedRead<'a, R> {
    inner: R,
    call: &'a Call,
    index: usize,
}
impl<R: AsyncRead + Unpin> AsyncRead for ObservedRead<'_, R> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        let before = buffer.filled().len();
        let had_capacity = buffer.remaining() > 0;
        // The exact inner poll/waker/buffer/result is delegated BEFORE locking.
        let result = Pin::new(&mut this.inner).poll_read(cx, buffer);
        this.call.update(|entry, elapsed| {
            let pipe = &mut entry.pipes[this.index];
            pipe.first_poll_us.get_or_insert(elapsed);
            match &result {
                Poll::Ready(Ok(())) => {
                    let received = buffer.filled().len() - before;
                    if received > 0 {
                        pipe.first_chunk_us.get_or_insert(elapsed);
                        pipe.bytes = pipe.bytes.saturating_add(received as u64);
                    } else if had_capacity {
                        pipe.eof = true;
                    }
                }
                Poll::Ready(Err(_)) => pipe.read_failed = true,
                Poll::Pending => {}
            }
            entry.progress.reads[this.index].observe(
                elapsed,
                &result,
                buffer.filled().len() > before,
                had_capacity,
            );
        });
        result
    }
}

/// Read-only actual-shell diagnostic view: no command/path/auth/error text.
pub(crate) struct ObservationView(
    Arc<Mutex<State>>,
    #[cfg(target_os = "macos")] Option<native::WorkspaceIdentity>,
);
impl ObservationView {
    pub(crate) fn report(&self) -> String {
        let state = lock(&self.0);
        let mut text = format!(
            "native-shell-test-observations calls={} omitted={} loss={}",
            state.next, state.omitted, state.loss
        );
        for entry in &state.entries {
            let _ = write!(
                text,
                "\ncall={} launch_enter_us={:?} launch_return_us={:?} launch_failed={} pid={:?} scope_end_us={:?}",
                entry.sequence,
                entry.launch_enter_us,
                entry.launch_return_us,
                entry.launch_failed,
                entry.pid,
                entry.scope_end_us
            );
            for (label, pipe) in ["stdout", "stderr"].into_iter().zip(&entry.pipes) {
                let _ = write!(
                    text,
                    " {label}_first_poll_us={:?} {label}_first_chunk_us={:?} {label}_bytes={} {label}_eof={} {label}_read_failed={}",
                    pipe.first_poll_us, pipe.first_chunk_us, pipe.bytes, pipe.eof, pipe.read_failed
                );
            }
        }
        text
    }
}

#[cfg(target_os = "macos")]
mod native;

#[cfg(target_os = "macos")]
mod snapshots;

impl Call {
    #[cfg(target_os = "macos")]
    pub(super) fn before_stop(
        &self,
        tree: &crate::platform_os::ProcessTree,
        stop: Option<craftsman_harness::ToolErrorKind>,
    ) {
        // The actual local tree remains OWNED before its ORIGINAL kill/reap.
        // A saved PID by itself never mints this hook or an execution grant.
        if let Some(ticket) = snapshots::note_before_stop(self, tree.id(), stop) {
            snapshots::capture(&self.state, ticket, self.workspace);
        }
    }
}

impl ObservationView {
    pub(crate) fn failure_report(&self) -> String {
        #[cfg(target_os = "macos")]
        snapshots::capture_active(&self.0, self.1);
        let report = self.report();
        #[cfg(target_os = "macos")]
        {
            let mut report = report;
            snapshots::append_active(&mut report, &lock(&self.0));
            report
        }
        #[cfg(not(target_os = "macos"))]
        report
    }
    /// Only failure teardown/new explicit TEST views include historical saved
    /// native facts. Existing released-call failure_report semantics stay exact.
    pub(crate) fn teardown_report(&self) -> String {
        #[cfg(target_os = "macos")]
        {
            snapshots::capture_active(&self.0, self.1);
            let mut report = self.report();
            snapshots::append_teardown(&mut report, &lock(&self.0));
            progress::append(&mut report, &lock(&self.0));
            report
        }
        #[cfg(not(target_os = "macos"))]
        {
            let mut report = self.failure_report();
            progress::append(&mut report, &lock(&self.0));
            report
        }
    }
}

/// Fixture-owned handles survive a call so an ORIGINAL assertion failure can
/// report real observations. No global hook, environment switch or raw output.
#[derive(Default)]
pub(crate) struct FailureObservations(Mutex<Vec<ObservationView>>);
impl FailureObservations {
    pub(crate) fn record(&self, runner: &crate::tools::ShellRunner) {
        if let Some(view) = runner.observation_view_for_tests() {
            if let Ok(mut views) = self.0.lock() {
                if views.len() == RETAINED_SHELLS {
                    views.remove(0);
                }
                views.push(view);
            }
        }
    }
}
impl Drop for FailureObservations {
    fn drop(&mut self) {
        if std::thread::panicking() {
            if let Ok(views) = self.0.lock() {
                for view in views.iter() {
                    // Only fixed ASCII labels/bools/integers, never a source view.
                    eprintln!("{}", view.teardown_report());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;

// Explicitly authorized manual diagnostics only. The original native snapshot
// remains independently <=13 queries /8 threads /4096 bytes with its SAME ticket.
#[cfg(target_os = "macos")]
mod stack_capture;

impl Call {
    #[cfg(target_os = "macos")]
    pub(super) async fn capture_owned_stack_if_marked(
        &self,
        tree: &crate::platform_os::ProcessTree,
        stop: Option<craftsman_harness::ToolErrorKind>,
    ) {
        stack_capture::capture_if_marked(self, tree, stop).await;
    }
}
