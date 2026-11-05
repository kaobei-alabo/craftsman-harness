//! Fallible output for legacy `-m`: a closed pipe must not panic or dispatch
//! tools from a response whose content already failed to reach stdout.
//!
//! This is not the P2g signal/final-result shutdown implementation. The caller
//! drops its active turn on sink failure; provider guards also check after a
//! response, because a single poll can otherwise stream and dispatch a tool
//! before an outer cancellation future gets polled again.

use async_trait::async_trait;
use craftsman_harness::{AgentError, ModelProvider, ModelRequest, ModelResponse};
use std::io::{ErrorKind, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

pub(crate) struct Output<W> {
    writer: Mutex<W>,
    failure: Mutex<Option<ErrorKind>>,
    closed: Arc<AtomicBool>,
    stopped: Notify,
}

impl<W: Write + Send + 'static> Output<W> {
    pub(crate) fn new(writer: W) -> Self {
        Self {
            writer: Mutex::new(writer),
            failure: Mutex::new(None),
            closed: Arc::new(AtomicBool::new(false)),
            stopped: Notify::new(),
        }
    }

    pub(crate) fn closed_flag(&self) -> Arc<AtomicBool> {
        self.closed.clone()
    }

    pub(crate) fn failure_kind(&self) -> Option<ErrorKind> {
        *self.failure.lock().unwrap()
    }

    pub(crate) fn sink(self: &Arc<Self>) -> crate::agent::TokenSink {
        let output = self.clone();
        Arc::new(move |text| output.write(text.as_bytes()))
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
            *self.failure.lock().unwrap() = Some(error.kind());
            self.closed.store(true, Ordering::Release);
            // notify_one caches a permit if the turn has not started waiting.
            self.stopped.notify_one();
        }
    }

    pub(crate) async fn failed(&self) {
        if !self.closed.load(Ordering::Acquire) {
            self.stopped.notified().await;
        }
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
