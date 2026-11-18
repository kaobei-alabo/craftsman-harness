//! Fallible output for legacy `-m`: a closed pipe must not panic or dispatch
//! tools from a response whose content already failed to reach stdout.
//!
//! This is not the P2g signal/final-result shutdown implementation. The caller
//! drops its active turn on sink failure; provider guards also check after a
//! response, because a single poll can otherwise stream and dispatch a tool
//! before an outer cancellation future gets polled again.
//! Redirected stdout is captured as an owned native file: Rust's shared
//! `Stdout` follows the global descriptor and suppresses invalid-descriptor
//! errors, so it cannot establish this selected output's success or failure.

use async_trait::async_trait;
use craftsman_harness::{AgentError, ModelProvider, ModelRequest, ModelResponse};
use std::io::{ErrorKind, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

pub(crate) type StdoutWriter = Box<dyn Write + Send>;
type Probe = Box<dyn Fn() -> std::io::Result<()> + Send + Sync>;

/// App-owned observation support, not native pipe data or a public receipt.
/// Checked writes stop on actual IO errors; they cannot proactively observe a
/// peer loss after successful output when no subsequent IO reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ObservationSupport {
    CheckedWrites,
    CheckedLocalPipeWrites,
    PeerState,
}

enum Observation {
    CheckedWrites,
    CheckedLocalPipeWrites,
    PeerState(Probe),
}

/// Capture the invocation's actual output before building any agent. A native
/// duplicate keeps redirected output bound to that pipe/file if another owner
/// closes or rebinds the process-global stdout slot. It also reports actual IO
/// errors instead of StdoutRaw's successful no-op for an invalid descriptor.
pub(crate) fn stdout() -> std::io::Result<Arc<Output<StdoutWriter>>> {
    #[cfg(windows)]
    {
        windows_probe::capture()
    }
    #[cfg(not(windows))]
    {
        captured_stdout()
    }
}

#[cfg(not(windows))]
fn captured_stdout() -> std::io::Result<Arc<Output<StdoutWriter>>> {
    let stdout = std::io::stdout();
    #[cfg(unix)]
    let writer = {
        use std::os::fd::AsFd;
        std::fs::File::from(stdout.as_fd().try_clone_to_owned()?)
    };
    #[cfg(not(any(unix, windows)))]
    let writer = stdout;
    #[cfg(unix)]
    let probe = {
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::FileTypeExt;
        let kind = writer.metadata()?.file_type();
        let descriptor = writer.as_raw_fd();
        if kind.is_socket() {
            let mut socket_type = 0i32;
            let mut length = std::mem::size_of::<i32>() as libc::socklen_t;
            // SAFETY: the captured File owns this live socket descriptor;
            // SO_TYPE reads exactly the supplied integer/length buffers and
            // neither consumes bytes nor changes socket or descriptor flags.
            if unsafe {
                libc::getsockopt(
                    descriptor,
                    libc::SOL_SOCKET,
                    libc::SO_TYPE,
                    (&mut socket_type as *mut i32).cast(),
                    &mut length,
                )
            } < 0
            {
                return Err(std::io::Error::last_os_error());
            }
            if length as usize != std::mem::size_of::<i32>() || socket_type != libc::SOCK_STREAM {
                return Err(std::io::Error::new(
                    ErrorKind::Unsupported,
                    "unsupported one-shot output socket type",
                ));
            }
        }
        (kind.is_fifo() || kind.is_socket()).then(|| {
            Box::new(move || {
                let mut poll = libc::pollfd {
                    fd: descriptor,
                    events: libc::POLLOUT,
                    revents: 0,
                };
                // SAFETY: Output retains the captured File while invoking
                // this zero-time, read-only readiness query. No pipe bytes,
                // descriptor flags or process ownership are changed.
                if unsafe { libc::poll(&mut poll, 1, 0) } < 0 {
                    Err(std::io::Error::last_os_error())
                } else if poll.revents & libc::POLLNVAL != 0 {
                    Err(std::io::Error::from_raw_os_error(libc::EBADF))
                } else if poll.revents & (libc::POLLERR | libc::POLLHUP) != 0 {
                    Err(std::io::Error::from(ErrorKind::BrokenPipe))
                } else {
                    Ok(())
                }
            }) as Probe
        })
    };
    #[cfg(not(any(unix, windows)))]
    let probe = None;
    let mut output = Output::new(Box::new(writer) as StdoutWriter);
    output.observation = match probe {
        Some(check) => Observation::PeerState(check),
        None => Observation::CheckedWrites,
    };
    Ok(Arc::new(output))
}

#[cfg(windows)]
#[path = "oneshot_io/windows_probe.rs"]
mod windows_probe;

pub(crate) struct Output<W> {
    writer: Mutex<W>,
    failure: Mutex<Option<ErrorKind>>,
    closed: Arc<AtomicBool>,
    stopped: Notify,
    observation: Observation,
    model_seen: AtomicBool,
}

impl<W: Write + Send + 'static> Output<W> {
    pub(crate) fn new(writer: W) -> Self {
        Self {
            writer: Mutex::new(writer),
            failure: Mutex::new(None),
            closed: Arc::new(AtomicBool::new(false)),
            stopped: Notify::new(),
            observation: Observation::CheckedWrites,
            model_seen: AtomicBool::new(false),
        }
    }

    pub(crate) fn closed_flag(&self) -> Arc<AtomicBool> {
        self.closed.clone()
    }

    pub(crate) fn failure_kind(&self) -> Option<ErrorKind> {
        *self.failure.lock().unwrap()
    }

    pub(crate) fn observation_support(&self) -> ObservationSupport {
        match &self.observation {
            Observation::CheckedWrites => ObservationSupport::CheckedWrites,
            Observation::CheckedLocalPipeWrites => ObservationSupport::CheckedLocalPipeWrites,
            Observation::PeerState(_) => ObservationSupport::PeerState,
        }
    }

    pub(crate) fn sink(self: &Arc<Self>) -> crate::agent::TokenSink {
        let output = self.clone();
        Arc::new(move |text| output.write(text.as_bytes()))
    }

    pub(crate) fn guard(self: &Arc<Self>) -> OutputGuard {
        let output = self.clone();
        let monitor_peer = self.observation_support() == ObservationSupport::PeerState;
        OutputGuard {
            check: Arc::new(move |after, dispatching| {
                let probe = if after {
                    output.model_seen.store(true, Ordering::Release);
                    dispatching
                } else {
                    output.model_seen.load(Ordering::Acquire)
                };
                if !output.closed.load(Ordering::Acquire) && probe && monitor_peer {
                    if let Observation::PeerState(check) = &output.observation {
                        let _writer = output.writer.lock().unwrap();
                        if let Err(error) = check() {
                            output.latch(error.kind());
                        }
                    }
                }
                if output.closed.load(Ordering::Acquire) {
                    Err(AgentError::State("output sink is closed".into()))
                } else {
                    Ok(())
                }
            }),
        }
    }

    fn latch(&self, kind: ErrorKind) {
        let mut failure = self.failure.lock().unwrap();
        if failure.is_none() {
            *failure = Some(kind);
            self.closed.store(true, Ordering::Release);
            // notify_one caches a permit if the turn has not started waiting.
            self.stopped.notify_one();
        }
    }

    pub(crate) fn write(&self, bytes: &[u8]) {
        if self.closed.load(Ordering::Acquire) {
            return;
        }
        let mut writer = self.writer.lock().unwrap();
        if self.closed.load(Ordering::Acquire) {
            return;
        }
        if let Err(error) = writer.write_all(bytes).and_then(|()| writer.flush()) {
            // Record only the kind: an IO error's arbitrary text is not a safe
            // diagnostic and must not carry the model's text or credentials.
            self.latch(error.kind());
        }
    }

    pub(crate) async fn failed(&self) {
        if !self.closed.load(Ordering::Acquire) {
            self.stopped.notified().await;
        }
    }
}

/// A source-bound check retains the actual output, not just a caller's boolean.
/// The initial request remains admitted; all roles check the same actual-error
/// latch before/after responses. Selected peer-state observers also check the
/// captured connection before later requests and tool dispatch. Limited local
/// pipe observation supplies no state query or post-success peer guarantee.
/// A no-tool final response still reaches the real final-newline write.
#[derive(Clone)]
pub(crate) struct OutputGuard {
    check: Arc<dyn Fn(bool, bool) -> Result<(), AgentError> + Send + Sync>,
}

pub(crate) fn checked_owned_provider(
    inner: Arc<dyn ModelProvider>,
    guard: Option<OutputGuard>,
) -> Arc<dyn ModelProvider> {
    match guard {
        Some(guard) => Arc::new(OwnedOutputCheckedProvider { inner, guard }),
        None => inner,
    }
}

struct OwnedOutputCheckedProvider {
    inner: Arc<dyn ModelProvider>,
    guard: OutputGuard,
}

#[async_trait]
impl ModelProvider for OwnedOutputCheckedProvider {
    async fn complete(&self, req: ModelRequest) -> Result<ModelResponse, AgentError> {
        (self.guard.check)(false, false)?;
        let response = self.inner.complete(req).await;
        let dispatching = matches!(&response, Ok(ModelResponse {
            message: craftsman_harness::Msg::Assistant { tool_calls, .. }, ..
        }) if !tool_calls.is_empty());
        (self.guard.check)(true, dispatching)?;
        response
    }
}

/// Defaults return the original provider untouched (TUI/frozen transports).
pub(crate) fn checked_provider(
    inner: Arc<dyn ModelProvider>,
    closed: Option<Arc<AtomicBool>>,
) -> Arc<dyn ModelProvider> {
    match closed {
        Some(closed) => Arc::new(OutputCheckedProvider { inner, closed }),
        None => inner,
    }
}

struct OutputCheckedProvider {
    inner: Arc<dyn ModelProvider>,
    closed: Arc<AtomicBool>,
}

impl OutputCheckedProvider {
    fn check(&self) -> Result<(), AgentError> {
        if self.closed.load(Ordering::Acquire) {
            Err(AgentError::State("output sink is closed".into()))
        } else {
            Ok(())
        }
    }
}

#[async_trait]
impl ModelProvider for OutputCheckedProvider {
    async fn complete(&self, req: ModelRequest) -> Result<ModelResponse, AgentError> {
        self.check()?;
        let response = self.inner.complete(req).await;
        // Includes a same-poll content delta followed by a tool-call response.
        // Do not give that response to the loop after the sink has failed.
        self.check()?;
        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use craftsman_harness::{Msg, Usage};
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;

    #[cfg(unix)]
    #[test]
    fn checked_write_support_does_not_invent_peer_state_and_latches_actual_io() {
        use std::io::Read;
        let (writer, mut peer) = std::os::unix::net::UnixStream::pair().unwrap();
        let output = Arc::new(Output::new(writer));
        assert_eq!(
            output.observation_support(),
            ObservationSupport::CheckedWrites
        );
        output.write(b"actual accepted output");
        let mut received = vec![0; b"actual accepted output".len()];
        peer.read_exact(&mut received).unwrap();
        assert_eq!(received, b"actual accepted output");
        drop(peer);
        let guard = output.guard();
        assert!((guard.check)(true, true).is_ok());
        assert_eq!(output.failure_kind(), None);
        output.write(b"actual later IO observes closure");
        assert_eq!(output.failure_kind(), Some(ErrorKind::BrokenPipe));
        assert!((guard.check)(false, false).is_err());
    }

    struct Writer {
        bytes: Arc<Mutex<Vec<u8>>>,
        writes: Arc<AtomicUsize>,
        max_write: usize,
        write_error: Option<ErrorKind>,
        flush_error: Option<ErrorKind>,
    }

    impl Write for Writer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.writes.fetch_add(1, Ordering::SeqCst);
            if let Some(kind) = self.write_error {
                return Err(std::io::Error::new(
                    kind,
                    "untrusted error text: secret-value",
                ));
            }
            let size = bytes.len().min(self.max_write);
            self.bytes.lock().unwrap().extend_from_slice(&bytes[..size]);
            Ok(size)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            match self.flush_error {
                Some(kind) => Err(std::io::Error::new(kind, "untrusted flush error")),
                None => Ok(()),
            }
        }
    }

    fn writer() -> Writer {
        Writer {
            bytes: Arc::new(Mutex::new(Vec::new())),
            writes: Arc::new(AtomicUsize::new(0)),
            max_write: usize::MAX,
            write_error: None,
            flush_error: None,
        }
    }

    #[test]
    fn output_handles_partial_writes_and_keeps_utf8_bytes() {
        let mut w = writer();
        w.max_write = 2;
        let seen = w.bytes.clone();
        let output = Arc::new(Output::new(w));
        (output.sink())("a中文🦀");
        output.write(b"\n");
        assert_eq!(*seen.lock().unwrap(), "a中文🦀\n".as_bytes());
        assert_eq!(output.failure_kind(), None);
        assert!(!output.closed_flag().load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn broken_pipe_is_latched_and_a_late_waiter_cannot_miss_it() {
        let mut w = writer();
        w.write_error = Some(ErrorKind::BrokenPipe);
        let writes = w.writes.clone();
        let output = Output::new(w);
        output.write(b"text");
        output.write(b"never retried");
        assert_eq!(output.failure_kind(), Some(ErrorKind::BrokenPipe));
        assert_eq!(writes.load(Ordering::SeqCst), 1);
        tokio::time::timeout(Duration::from_secs(5), output.failed())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn flush_failure_wakes_the_output_waiter() {
        let mut w = writer();
        w.flush_error = Some(ErrorKind::BrokenPipe);
        let output = Arc::new(Output::new(w));
        let waiting = output.clone();
        let waiter = tokio::spawn(async move { waiting.failed().await });
        output.write(b"buffered");
        tokio::time::timeout(Duration::from_secs(5), waiter)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(output.failure_kind(), Some(ErrorKind::BrokenPipe));
    }

    #[test]
    fn other_output_errors_keep_only_their_kind_and_also_seal_the_sink() {
        let mut w = writer();
        w.write_error = Some(ErrorKind::PermissionDenied);
        let output = Output::new(w);
        output.write(b"secret model text");
        assert_eq!(output.failure_kind(), Some(ErrorKind::PermissionDenied));
        assert!(output.closed_flag().load(Ordering::Acquire));
    }

    struct CountingModel {
        calls: Arc<AtomicUsize>,
        close_during_response: Option<Arc<AtomicBool>>,
        fail: bool,
    }

    #[async_trait]
    impl ModelProvider for CountingModel {
        async fn complete(&self, _: ModelRequest) -> Result<ModelResponse, AgentError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if let Some(closed) = &self.close_during_response {
                closed.store(true, Ordering::Release);
            }
            if self.fail {
                return Err(AgentError::Model("provider test failure".into()));
            }
            Ok(ModelResponse {
                message: Msg::assistant("done"),
                usage: Usage::default(),
            })
        }
    }

    fn request() -> ModelRequest {
        ModelRequest {
            system: String::new(),
            messages: vec![],
            tools: vec![],
        }
    }

    #[tokio::test]
    async fn closed_output_prevents_the_next_main_or_child_model_request() {
        let calls = Arc::new(AtomicUsize::new(0));
        let closed = Arc::new(AtomicBool::new(true));
        let provider = checked_provider(
            Arc::new(CountingModel {
                calls: calls.clone(),
                close_during_response: None,
                fail: false,
            }),
            Some(closed),
        );
        assert!(matches!(
            provider.complete(request()).await,
            Err(AgentError::State(_))
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn closure_during_response_blocks_dispatch_even_without_another_await() {
        let calls = Arc::new(AtomicUsize::new(0));
        let closed = Arc::new(AtomicBool::new(false));
        let provider = checked_provider(
            Arc::new(CountingModel {
                calls: calls.clone(),
                close_during_response: Some(closed.clone()),
                fail: false,
            }),
            Some(closed),
        );
        assert!(matches!(
            provider.complete(request()).await,
            Err(AgentError::State(_))
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn unclosed_output_keeps_provider_errors_unchanged() {
        let provider = checked_provider(
            Arc::new(CountingModel {
                calls: Arc::new(AtomicUsize::new(0)),
                close_during_response: None,
                fail: true,
            }),
            Some(Arc::new(AtomicBool::new(false))),
        );
        assert!(matches!(provider.complete(request()).await,
            Err(AgentError::Model(message)) if message == "provider test failure"));
    }

    #[test]
    fn absent_guard_returns_the_same_provider_for_other_frontends() {
        let provider: Arc<dyn ModelProvider> = Arc::new(CountingModel {
            calls: Arc::new(AtomicUsize::new(0)),
            close_during_response: None,
            fail: false,
        });
        let checked = checked_provider(provider.clone(), None);
        assert!(Arc::ptr_eq(&provider, &checked));
    }
}

#[cfg(all(test, unix))]
#[path = "oneshot_io/native_tests.rs"]
mod native_tests;

#[cfg(all(test, windows))]
#[path = "oneshot_io/windows_tests.rs"]
mod windows_tests;
