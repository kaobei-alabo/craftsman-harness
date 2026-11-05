//! Test-only original legacy-shell phase/read facts, owned by one ShellRunner.
//! Never a global hook, alternate launcher, readiness witness or public receipt.

use std::fmt::{self, Write};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, ReadBuf};
use tokio::time::Instant;

const MAX_CALLS: usize = 8;
const MAX_REPORT_BYTES: usize = 4096;
const REFUSED: &str = "legacy-shell-observations refused=unavailable-or-incomplete\n";
const REPORT_BOUND: &str = "legacy-shell-observations refused=whole-report-bound\n";

pub(super) struct Owner(Mutex<History>);
#[derive(Default)]
struct History {
    calls: Vec<Call>,
    refused: bool,
}

#[derive(Clone)]
pub(super) struct Call(Arc<Mutex<Data>>);
struct Data {
    started: Instant,
    limit_us: Option<u64>,
    spawn_return_us: Option<u64>,
    pid: Option<u32>,
    readers: Selection,
    wait: Selection,
    stop_enter_us: Option<u64>,
    stop_return_us: Option<u64>,
    stop_pid: &'static str,
    drain: Selection,
    reads: [Reads; 2],
    before_stop: Option<[Reads; 2]>,
    return_branch: &'static str,
    loss: bool,
}

#[derive(Clone, Copy, Default)]
struct Selection {
    enter_us: Option<u64>,
    return_us: Option<u64>,
    result: Option<&'static str>,
    loss: bool,
}
impl Selection {
    fn enter(&mut self, now: Option<u64>) {
        if self.enter_us.is_some() || now.is_none() {
            self.loss = true;
        } else {
            self.enter_us = now;
        }
    }
    fn returned(&mut self, now: Option<u64>, result: &'static str) {
        if self.enter_us.is_none() || self.result.is_some() || now.is_none() {
            self.loss = true;
        } else {
            self.return_us = now;
            self.result = Some(result);
        }
    }
}

#[derive(Clone, Copy)]
struct Reads {
    polls: Option<u64>,
    pending: Option<u64>,
    data: Option<u64>,
    eof: Option<u64>,
    empty_capacity: Option<u64>,
    errors: Option<u64>,
    bytes: Option<u64>,
    last_us: Option<u64>,
    last: &'static str,
}
impl Default for Reads {
    fn default() -> Self {
        Self {
            polls: Some(0),
            pending: Some(0),
            data: Some(0),
            eof: Some(0),
            empty_capacity: Some(0),
            errors: Some(0),
            bytes: Some(0),
            last_us: None,
            last: "not-polled",
        }
    }
}
fn increment(counter: &mut Option<u64>, amount: u64) {
    *counter = counter.and_then(|value| value.checked_add(amount));
}

#[derive(Clone, Copy)]
pub(super) enum Stream {
    Stdout,
    Stderr,
}
impl Stream {
    fn index(self) -> usize {
        match self {
            Self::Stdout => 0,
            Self::Stderr => 1,
        }
    }
}

impl Owner {
    pub(super) fn new() -> Self {
        Self(Mutex::new(History::default()))
    }
    pub(super) fn begin(&self, started: Instant, limit: Duration) -> Option<Call> {
        let mut history = self.0.lock().ok()?;
        if history.refused || history.calls.len() >= MAX_CALLS {
            history.refused = true;
            return None;
        }
        let call = Call(Arc::new(Mutex::new(Data {
            started,
            limit_us: limit.as_micros().try_into().ok(),
            spawn_return_us: None,
            pid: None,
            readers: Selection::default(),
            wait: Selection::default(),
            stop_enter_us: None,
            stop_return_us: None,
            stop_pid: "not-observed",
            drain: Selection::default(),
            reads: [Reads::default(); 2],
            before_stop: None,
            return_branch: "not-observed",
            loss: false,
        })));
        history.calls.push(call.clone());
        Some(call)
    }

    /// Only fixed labels/actual optional numbers. No new IO or PID lookup.
    pub(super) fn report(&self) -> String {
        let Ok(history) = self.0.lock() else {
            return REFUSED.into();
        };
        if history.refused {
            return REFUSED.into();
        }
        let mut out = Bounded(String::new());
        if writeln!(out, "legacy-shell-observations phase=original-call-history").is_err() {
            return REPORT_BOUND.into();
        }
        for (index, call) in history.calls.iter().enumerate() {
            let Ok(data) = call.0.lock() else {
                return REFUSED.into();
            };
            if data.loss || data.readers.loss || data.wait.loss || data.drain.loss {
                return REFUSED.into();
            }
            if write_call(&mut out, index + 1, &data).is_err() {
                return REPORT_BOUND.into();
            }
        }
        out.0
    }
}
impl Call {
    fn update(&self, action: impl FnOnce(&mut Data, Option<u64>)) {
        if let Ok(mut data) = self.0.lock() {
            let now = data.started.elapsed().as_micros().try_into().ok();
            action(&mut data, now);
        }
    }
    pub(super) fn spawned(&self, pid: Option<u32>) {
        self.update(|data, now| {
            if data.spawn_return_us.is_some() || now.is_none() {
                data.loss = true;
            } else {
                data.spawn_return_us = now;
                data.pid = pid;
            }
        });
    }
    pub(super) fn readers_enter(&self) {
        self.update(|data, now| data.readers.enter(now));
    }
    pub(super) fn readers_return(&self, completed: bool) {
        self.update(|data, now| {
            data.readers
                .returned(now, if completed { "returned" } else { "deadline" })
        });
    }
    pub(super) fn wait_enter(&self) {
        self.update(|data, now| data.wait.enter(now));
    }
    pub(super) fn wait_return(&self, result: &Option<std::io::Result<std::process::ExitStatus>>) {
        let result = match result {
            Some(Ok(_)) => "returned-status",
            Some(Err(_)) => "returned-error",
            None => "deadline",
        };
        self.update(|data, now| data.wait.returned(now, result));
    }
    pub(super) fn before_stop(&self, pid: Option<u32>) {
        self.update(|data, now| {
            if data.before_stop.is_some() || now.is_none() {
                data.loss = true;
            } else {
                data.stop_enter_us = now;
                data.stop_pid = match (data.pid, pid) {
                    (Some(original), Some(current)) if original == current => "same-owned-tree-id",
                    (Some(_), Some(_)) => "different-tree-id",
                    _ => "unavailable",
                };
                data.before_stop = Some(data.reads);
            }
        });
    }
    pub(super) fn stop_return(&self) {
        self.update(|data, now| {
            if data.stop_enter_us.is_none() || data.stop_return_us.is_some() || now.is_none() {
                data.loss = true;
            } else {
                data.stop_return_us = now;
            }
        });
    }
    pub(super) fn drain_enter(&self) {
        self.update(|data, now| data.drain.enter(now));
    }
    pub(super) fn drain_return(&self, completed: bool) {
        self.update(|data, now| {
            data.drain
                .returned(now, if completed { "returned" } else { "deadline" })
        });
    }
    pub(super) fn returned(&self, timeout: bool) {
        self.update(|data, _| {
            if data.return_branch != "not-observed" {
                data.loss = true;
            } else {
                data.return_branch = if timeout { "timeout" } else { "normal" };
            }
        });
    }
    fn read(
        &self,
        stream: Stream,
        had_capacity: bool,
        bytes: Option<usize>,
        result: &Poll<std::io::Result<()>>,
    ) {
        self.update(|data, now| {
            let Some(bytes) = bytes.and_then(|value| u64::try_from(value).ok()) else {
                data.loss = true;
                return;
            };
            let read = &mut data.reads[stream.index()];
            increment(&mut read.polls, 1);
            read.last_us = now;
            read.last = match result {
                Poll::Pending => {
                    increment(&mut read.pending, 1);
                    data.loss |= bytes != 0;
                    "pending"
                }
                Poll::Ready(Err(_)) => {
                    increment(&mut read.errors, 1);
                    data.loss |= bytes != 0;
                    "error"
                }
                Poll::Ready(Ok(())) if bytes > 0 => {
                    increment(&mut read.data, 1);
                    increment(&mut read.bytes, bytes);
                    "data"
                }
                Poll::Ready(Ok(())) if had_capacity => {
                    increment(&mut read.eof, 1);
                    "eof"
                }
                Poll::Ready(Ok(())) => {
                    increment(&mut read.empty_capacity, 1);
                    "empty-capacity"
                }
            };
        });
    }
}

pub(super) struct Reader<R> {
    inner: R,
    call: Option<Call>,
    stream: Stream,
}
impl<R> Reader<R> {
    pub(super) fn new(inner: R, call: Option<Call>, stream: Stream) -> Self {
        Self {
            inner,
            call,
            stream,
        }
    }
}
impl<R: AsyncRead + Unpin> AsyncRead for Reader<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        let before = buffer.filled().len();
        let had_capacity = buffer.remaining() > 0;
        // Delegate FIRST with the ORIGINAL context/waker/buffer. Never poll
        // under the recorder lock, wrap/replace a waker or retain byte text.
        let result = Pin::new(&mut this.inner).poll_read(cx, buffer);
        if let Some(call) = &this.call {
            call.read(
                this.stream,
                had_capacity,
                buffer.filled().len().checked_sub(before),
                &result,
            );
        }
        result
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
fn write_selection(out: &mut Bounded, label: &'static str, value: &Selection) -> fmt::Result {
    write!(
        out,
        " {label}_enter_us={:?} {label}_return_us={:?} {label}_result={}",
        value.enter_us,
        value.return_us,
        value.result.unwrap_or("not-returned")
    )
}
fn write_reads(out: &mut Bounded, label: &'static str, value: &Reads) -> fmt::Result {
    write!(
        out,
        " {label}_polls={:?} pending={:?} data={:?} eof={:?} empty_capacity={:?} errors={:?} observed_read_bytes={:?} last_poll_us={:?} last_result={}",
        value.polls,
        value.pending,
        value.data,
        value.eof,
        value.empty_capacity,
        value.errors,
        value.bytes,
        value.last_us,
        value.last
    )
}
fn write_call(out: &mut Bounded, sequence: usize, data: &Data) -> fmt::Result {
    writeln!(
        out,
        "call={sequence} original_limit_us={:?} spawn_return_us={:?} actual_spawn_pid={:?} original_return_branch={}",
        data.limit_us, data.spawn_return_us, data.pid, data.return_branch
    )?;
    write!(out, "call={sequence} phase=original-selection")?;
    write_selection(out, "readers", &data.readers)?;
    write_selection(out, "wait", &data.wait)?;
    writeln!(out)?;
    write!(
        out,
        "call={sequence} phase=original-stop stop_enter_us={:?} stop_return_us={:?} stop_pid_relation={}",
        data.stop_enter_us, data.stop_return_us, data.stop_pid
    )?;
    write_selection(out, "drain", &data.drain)?;
    writeln!(out)?;
    if let Some(reads) = &data.before_stop {
        for (label, read) in ["stdout", "stderr"].into_iter().zip(reads) {
            write!(out, "call={sequence} phase=before-stop")?;
            write_reads(out, label, read)?;
            writeln!(out)?;
        }
    }
    for (label, read) in ["stdout", "stderr"].into_iter().zip(&data.reads) {
        write!(out, "call={sequence} phase=current-history")?;
        write_reads(out, label, read)?;
        writeln!(out)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::task::{Wake, Waker};

    struct Noop;
    impl Wake for Noop {
        fn wake(self: Arc<Self>) {}
    }
    fn owner_call() -> (Owner, Call) {
        let owner = Owner::new();
        let call = owner.begin(Instant::now(), Duration::from_secs(2)).unwrap();
        (owner, call)
    }
    struct OriginalReader {
        waker: Waker,
        polls: usize,
    }
    impl AsyncRead for OriginalReader {
        fn poll_read(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buffer: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            assert!(cx.waker().will_wake(&self.waker));
            assert_eq!(buffer.remaining(), 7);
            assert_eq!(buffer.filled(), b"x");
            self.polls += 1;
            if self.polls == 1 {
                Poll::Pending
            } else {
                buffer.put_slice(b"abc");
                Poll::Ready(Ok(()))
            }
        }
    }

    #[test]
    fn original_waker_buffer_pending_and_ready_results_are_preserved() {
        let (owner, call) = owner_call();
        let waker = Waker::from(Arc::new(Noop));
        let mut context = Context::from_waker(&waker);
        let mut reader = Reader::new(
            OriginalReader {
                waker: waker.clone(),
                polls: 0,
            },
            Some(call.clone()),
            Stream::Stdout,
        );
        let mut bytes = [0; 8];
        let mut buffer = ReadBuf::new(&mut bytes);
        buffer.put_slice(b"x");
        assert!(
            Pin::new(&mut reader)
                .poll_read(&mut context, &mut buffer)
                .is_pending()
        );
        assert_eq!(buffer.filled(), b"x");
        assert!(matches!(
            Pin::new(&mut reader).poll_read(&mut context, &mut buffer),
            Poll::Ready(Ok(()))
        ));
        assert_eq!(buffer.filled(), b"xabc");
        let data = call.0.lock().unwrap();
        assert_eq!(data.reads[0].polls, Some(2));
        assert_eq!(data.reads[0].pending, Some(1));
        assert_eq!(data.reads[0].bytes, Some(3));
        assert_eq!(data.reads[1].polls, Some(0));
        drop(data);
        assert!(!owner.report().contains("abc"));
    }

    #[test]
    fn zero_capacity_and_errors_remain_distinct_from_eof() {
        let (owner, call) = owner_call();
        call.read(Stream::Stdout, false, Some(0), &Poll::Ready(Ok(())));
        call.read(Stream::Stdout, true, Some(0), &Poll::Ready(Ok(())));
        struct OriginalError;
        impl AsyncRead for OriginalError {
            fn poll_read(
                self: Pin<&mut Self>,
                _cx: &mut Context<'_>,
                _buffer: &mut ReadBuf<'_>,
            ) -> Poll<std::io::Result<()>> {
                Poll::Ready(Err(std::io::Error::from_raw_os_error(987654)))
            }
        }
        let waker = Waker::from(Arc::new(Noop));
        let mut context = Context::from_waker(&waker);
        let mut reader = Reader::new(OriginalError, Some(call.clone()), Stream::Stderr);
        let mut bytes = [0; 1];
        let mut buffer = ReadBuf::new(&mut bytes);
        let error = match Pin::new(&mut reader).poll_read(&mut context, &mut buffer) {
            Poll::Ready(Err(error)) => error,
            _ => panic!("original read error must be returned unchanged"),
        };
        assert_eq!(error.raw_os_error(), Some(987654));
        assert!(buffer.filled().is_empty());
        let data = call.0.lock().unwrap();
        assert_eq!(data.reads[0].empty_capacity, Some(1));
        assert_eq!(data.reads[0].eof, Some(1));
        assert_eq!(data.reads[1].errors, Some(1));
        assert_eq!(data.reads[1].eof, Some(0));
        drop(data);
        assert!(!owner.report().contains("987654"));
    }

    #[test]
    fn counter_overflow_becomes_unknown_without_reset_refund_or_saturation() {
        let (owner, call) = owner_call();
        {
            let mut data = call.0.lock().unwrap();
            data.reads[0].polls = Some(u64::MAX);
            data.reads[0].bytes = Some(u64::MAX);
        }
        call.read(Stream::Stdout, true, Some(1), &Poll::Ready(Ok(())));
        call.read(Stream::Stdout, true, Some(1), &Poll::Ready(Ok(())));
        let data = call.0.lock().unwrap();
        assert_eq!(data.reads[0].polls, None);
        assert_eq!(data.reads[0].bytes, None);
        assert_eq!(data.reads[0].data, Some(2));
        drop(data);
        assert!(owner.report().contains("observed_read_bytes=None"));
    }

    #[test]
    fn call_bound_refuses_whole_history_without_eviction_or_reset() {
        let owner = Owner::new();
        for _ in 0..MAX_CALLS {
            assert!(
                owner
                    .begin(Instant::now(), Duration::from_secs(2))
                    .is_some()
            );
        }
        assert!(
            owner
                .begin(Instant::now(), Duration::from_secs(2))
                .is_none()
        );
        assert!(
            owner
                .begin(Instant::now(), Duration::from_secs(2))
                .is_none()
        );
        assert_eq!(owner.0.lock().unwrap().calls.len(), MAX_CALLS);
        assert_eq!(owner.report(), REFUSED);
    }

    #[test]
    fn report_bound_refuses_whole_history_not_a_clipped_prefix() {
        let owner = Owner::new();
        for _ in 0..MAX_CALLS {
            let call = owner.begin(Instant::now(), Duration::from_secs(2)).unwrap();
            call.before_stop(Some(123));
        }
        assert_eq!(owner.report(), REPORT_BOUND);
        assert!(owner.report().len() < MAX_REPORT_BYTES);
    }

    #[test]
    fn before_stop_counts_are_not_relabelled_post_kill_or_queried_again() {
        let (owner, call) = owner_call();
        call.spawned(Some(123));
        call.read(Stream::Stdout, true, Some(0), &Poll::Pending);
        call.before_stop(Some(123));
        call.read(Stream::Stdout, true, Some(3), &Poll::Ready(Ok(())));
        call.read(Stream::Stderr, true, Some(0), &Poll::Ready(Ok(())));
        let data = call.0.lock().unwrap();
        assert_eq!(data.before_stop.unwrap()[0].bytes, Some(0));
        assert_eq!(data.before_stop.unwrap()[0].polls, Some(1));
        assert_eq!(data.reads[0].bytes, Some(3));
        assert_eq!(data.reads[0].polls, Some(2));
        assert_eq!(data.stop_pid, "same-owned-tree-id");
        drop(data);
        let first = owner.report();
        let second = owner.report();
        assert_eq!(first, second);
        assert!(first.contains("phase=before-stop stdout_polls=Some(1)"));
        assert!(first.contains("phase=current-history stdout_polls=Some(2)"));
        assert!(
            first
                .bytes()
                .all(|byte| byte == b'\n' || (0x20..=0x7e).contains(&byte))
        );
        assert!(first.len() <= MAX_REPORT_BYTES);
    }

    #[test]
    fn duplicate_phase_and_poison_refuse_without_teardown_panic() {
        let (owner, call) = owner_call();
        call.readers_enter();
        call.readers_enter();
        assert_eq!(owner.report(), REFUSED);
        let (poisoned, _) = owner_call();
        let result = std::panic::catch_unwind(|| {
            let _guard = poisoned.0.lock().unwrap();
            panic!("synthetic recorder poison");
        });
        assert!(result.is_err());
        assert!(
            poisoned
                .begin(Instant::now(), Duration::from_secs(2))
                .is_none()
        );
        assert_eq!(poisoned.report(), REFUSED);
    }

    #[tokio::test]
    async fn actual_legacy_zero_byte_timeout_keeps_typed_facts_and_original_phases() {
        use crate::shell::ShellSettings;
        use crate::tools::ShellRunner;
        use craftsman_harness::ToolErrorKind;
        use serde_json::json;
        use std::path::PathBuf;

        // This fixture deliberately has NO startup/readiness/output claim.
        // Unix uses only a shell builtin, so no ungrouped grandchild can leak.
        #[cfg(unix)]
        let command = "while :; do :; done";
        #[cfg(windows)]
        let command = "[System.Threading.Thread]::Sleep(60000)";
        let runner = ShellRunner::new(
            PathBuf::from("."),
            ShellSettings::from_value(Some(
                &json!({"timeout_secs": 2, "output_format": "head_tail"}),
            )),
        );
        let outcome =
            tokio::time::timeout(Duration::from_secs(30), runner.run_outcome(command, None))
                .await
                .expect("one real call returns within the unchanged outer bound");
        assert!(outcome.is_error);
        assert_eq!(outcome.meta.error_kind, Some(ToolErrorKind::Timeout));
        assert_eq!(outcome.meta.bytes, Some(0));
        assert_eq!(outcome.meta.exit_code, None);
        assert!(outcome.meta.duration.unwrap() >= Duration::from_secs(2));
        assert!(!outcome.meta.truncated);
        assert_eq!(
            outcome.content,
            "Error: command timed out after 2s and was stopped. It printed nothing before the timeout."
        );
        let report = runner.legacy_observations.as_ref().unwrap().report();
        assert!(
            report.contains("original_limit_us=Some(2000000)"),
            "{report}"
        );
        assert!(
            report.contains("original_return_branch=timeout"),
            "{report}"
        );
        assert!(report.contains("readers_result=deadline"), "{report}");
        assert!(report.contains("wait_result=not-returned"), "{report}");
        assert!(report.contains("phase=before-stop"), "{report}");
        assert!(report.contains("phase=current-history"), "{report}");
        assert!(report.contains("observed_read_bytes=Some(0)"), "{report}");
        assert!(
            report
                .bytes()
                .all(|byte| byte == b'\n' || (0x20..=0x7e).contains(&byte))
        );
        assert!(report.len() <= MAX_REPORT_BYTES);
    }
}
