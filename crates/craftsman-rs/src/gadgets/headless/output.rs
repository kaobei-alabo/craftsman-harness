//! Private guest output adapters, not a public ToolOutcome/schema producer.
//!
//! The host selects its frozen redactor from the actual runtime scope. Original
//! bytes enter redaction and a delayed literal-verification stream BEFORE the
//! 64 KiB retention cut. No legacy Capture, spill, detector or publication route.

use super::{ErrorKind, GadgetError, InvocationGate, ensure, error};
use crate::exec::redaction::{ByteStream, RedactionError, Redactor};
use axum::body::Bytes; // Existing app Bytes alias for WASI; no new dependency.
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use tokio::io::AsyncWrite;
use wasmtime_wasi::cli::{IsTerminal, StdoutStream};
use wasmtime_wasi::p2::{OutputStream, Pollable, StreamError, StreamResult};

use super::super::runtime::CallBudget;

pub(super) const MAX_LOG_BYTES: usize = 64 * 1024;
const MAX_WRITE: usize = 64 * 1024;
const WASI_PERMIT: usize = 8192;

fn redaction_error(e: RedactionError) -> GadgetError {
    let kind = match e {
        RedactionError::Record | RedactionError::Chunk | RedactionError::Position => {
            ErrorKind::OutputLimit
        }
        _ => ErrorKind::InvalidGadget,
    };
    error(kind, "private guest output cannot be safely redacted")
}
fn rendered_error() -> GadgetError {
    error(
        ErrorKind::InvalidGadget,
        "private guest rendered output conflicts with a captured credential",
    )
}
fn closed_error() -> GadgetError {
    error(ErrorKind::Io, "private guest output capture is closed")
}

/// String values/member names are redacted before JSON escaping. Then reject
/// literal collisions in the encoded view, including numeric/boolean/null or
/// syntax values and replacement/encoding boundaries. Never corrupt JSON types
/// or trusted structural fields just to replace a captured value.
pub(super) fn json(redactor: &Redactor, raw: &str) -> Result<String, GadgetError> {
    if raw.len() > craftsman_gadget::MAX_JSON_BYTES {
        return Err(error(
            ErrorKind::OutputLimit,
            "private guest result exceeds 1 MiB",
        ));
    }
    let value = serde_json::from_str(raw).map_err(|_| {
        error(
            ErrorKind::InvalidGadget,
            "private guest returned invalid JSON",
        )
    })?;
    let value = redactor.json(&value).map_err(redaction_error)?;
    let encoded = serde_json::to_string(&value).map_err(|_| {
        error(
            ErrorKind::InvalidGadget,
            "private guest result encoding failed",
        )
    })?;
    if encoded.len() > craftsman_gadget::MAX_JSON_BYTES {
        return Err(error(
            ErrorKind::OutputLimit,
            "private guest result exceeds 1 MiB",
        ));
    }
    let mut check = redactor.byte_stream();
    for chunk in encoded.as_bytes().chunks(MAX_WRITE) {
        check.push(chunk).map_err(redaction_error)?;
        if check.matched() {
            return Err(rendered_error());
        }
    }
    check.finish().map_err(redaction_error)?;
    Ok(encoded)
}

struct Buffer {
    original: ByteStream,
    rendered: ByteStream,
    retained: Vec<u8>,
    budget: Arc<CallBudget>,
    truncated: bool,
    discarding: bool,
    closed: bool,
    failure: Option<ErrorKind>,
}
impl Buffer {
    fn abort_pending(&mut self) {
        // Both calls must run, even if the first reports discarded bytes.
        let original = self.original.abort();
        let rendered = self.rendered.abort();
        self.truncated |= original || rendered;
    }
    fn fail(&mut self, e: &GadgetError) {
        self.abort_pending();
        self.failure = Some(e.kind);
        if !matches!(e.kind, ErrorKind::NeedsApproval | ErrorKind::Timeout) {
            self.retained.clear();
        }
        self.truncated = true;
    }
    fn retain(&mut self, bytes: &[u8]) {
        let take = bytes.len().min(MAX_LOG_BYTES - self.retained.len());
        self.retained.extend_from_slice(&bytes[..take]);
        self.truncated |= take < bytes.len();
        if self.retained.len() == MAX_LOG_BYTES {
            // Already released bytes passed BOTH delayed matchers. Once full,
            // discard suffixes and drain later writes without unbounded work.
            self.abort_pending();
            self.discarding = true;
        }
    }
    fn verified(&mut self, bytes: &[u8]) -> Result<(), GadgetError> {
        for chunk in bytes.chunks(MAX_WRITE) {
            if self.discarding {
                self.truncated = true;
                break;
            }
            let safe = self.rendered.push(chunk).map_err(redaction_error)?;
            if self.rendered.matched() {
                return Err(rendered_error());
            }
            self.retain(&safe);
        }
        Ok(())
    }
    fn text(&mut self) -> Result<String, GadgetError> {
        match std::str::from_utf8(&self.retained) {
            Ok(s) => Ok(s.to_owned()),
            Err(e) if e.error_len().is_none() && self.truncated => {
                // A byte cap may cut one final non-secret UTF-8 scalar. Omit
                // only that incomplete suffix; never lossy-replace any bytes.
                self.retained.truncate(e.valid_up_to());
                Ok(std::str::from_utf8(&self.retained)
                    .map_err(|_| redaction_error(RedactionError::Utf8))?
                    .to_owned())
            }
            Err(_) => Err(redaction_error(RedactionError::Utf8)),
        }
    }
}

/// No Debug/Serialize/raw accessor. All stdout/stderr/import handles share the
/// SAME original-byte order, frozen patterns and actual invocation fail latch.
#[derive(Clone)]
pub(super) struct RedactedCapture {
    buffer: Arc<Mutex<Buffer>>,
    gate: Arc<InvocationGate>,
}
impl RedactedCapture {
    pub(super) fn new(
        redactor: &Redactor,
        gate: Arc<InvocationGate>,
        budget: Arc<CallBudget>,
    ) -> Self {
        Self {
            buffer: Arc::new(Mutex::new(Buffer {
                original: redactor.byte_stream(),
                rendered: redactor.byte_stream(),
                retained: Vec::new(),
                budget,
                truncated: false,
                discarding: false,
                closed: false,
                failure: None,
            })),
            gate,
        }
    }
    fn check(&self, buffer: &mut Buffer) -> Result<(), GadgetError> {
        // Typed invocation stop/expiry always wins over formatting faults.
        if let Err(e) = ensure(&self.gate, Some(&buffer.budget)) {
            buffer.fail(&e);
            return Err(e);
        }
        if let Some(kind) = buffer.failure {
            return Err(error(kind, "private guest output capture failed"));
        }
        if buffer.closed {
            return Err(closed_error());
        }
        Ok(())
    }
    pub(super) fn ready(&self) -> Result<(), GadgetError> {
        self.check(&mut self.buffer.lock().unwrap())
    }
    pub(super) fn reject_oversized_record(&self) {
        let mut buffer = self.buffer.lock().unwrap();
        if self.check(&mut buffer).is_ok() {
            buffer.fail(&error(
                ErrorKind::OutputLimit,
                "private guest log record exceeds 64 KiB",
            ));
        }
    }
    pub(super) fn append(&self, bytes: &[u8]) -> Result<(), GadgetError> {
        let mut buffer = self.buffer.lock().unwrap();
        self.check(&mut buffer)?;
        if bytes.len() > MAX_WRITE {
            let e = error(
                ErrorKind::OutputLimit,
                "private guest log write exceeds 64 KiB",
            );
            buffer.fail(&e);
            return Err(e);
        }
        if buffer.discarding {
            buffer.truncated |= !bytes.is_empty();
            return self.check(&mut buffer);
        }
        let result = buffer
            .original
            .push(bytes)
            .map_err(redaction_error)
            .and_then(|bytes| buffer.verified(&bytes));
        if let Err(e) = result {
            buffer.fail(&e);
            return Err(e);
        }
        self.check(&mut buffer)
    }
    /// Natural completed export only. Flush/handle shutdown never releases
    /// match holdback; failures/drop must discard it rather than call finish.
    pub(super) fn finish(&self) -> Result<(String, bool), GadgetError> {
        let mut buffer = self.buffer.lock().unwrap();
        self.check(&mut buffer)?;
        let result = (|| {
            if !buffer.discarding {
                let tail = buffer.original.finish().map_err(redaction_error)?;
                buffer.verified(&tail)?;
                if !buffer.discarding {
                    let tail = buffer.rendered.finish().map_err(redaction_error)?;
                    buffer.retain(&tail);
                }
            }
            buffer.text()
        })();
        let text = match result {
            Ok(text) => text,
            Err(e) => {
                buffer.fail(&e);
                return Err(e);
            }
        };
        self.check(&mut buffer)?;
        buffer.closed = true;
        Ok((text, buffer.truncated))
    }
    /// No release of pending original/rendered bytes on failed/aborted work.
    pub(super) fn abort(&self) {
        let mut buffer = self.buffer.lock().unwrap();
        buffer.abort_pending();
        buffer.closed = true;
    }
    pub(super) fn truncated(&self) -> bool {
        self.buffer.lock().unwrap().truncated
    }
    #[cfg(test)]
    pub(super) fn set_budget_for_tests(&self, budget: Arc<CallBudget>) {
        self.buffer.lock().unwrap().budget = budget;
    }
    #[cfg(test)]
    pub(super) fn snapshot_for_tests(&self) -> (String, bool) {
        let buffer = self.buffer.lock().unwrap();
        let prefix = match std::str::from_utf8(&buffer.retained) {
            Ok(s) => s,
            Err(e) => std::str::from_utf8(&buffer.retained[..e.valid_up_to()]).unwrap(),
        };
        (prefix.to_owned(), buffer.truncated)
    }
}
impl IsTerminal for RedactedCapture {
    fn is_terminal(&self) -> bool {
        false
    }
}
impl StdoutStream for RedactedCapture {
    fn async_stream(&self) -> Box<dyn AsyncWrite + Send + Sync> {
        Box::new(Writer {
            capture: self.clone(),
            closed: false,
        })
    }
    fn p2_stream(&self) -> Box<dyn OutputStream> {
        // No AsyncWriteStream background task/queue. Every WASIp2 write reaches
        // the same gated matcher synchronously BEFORE export finalization.
        Box::new(WasiWriter {
            capture: self.clone(),
            permit: 0,
            closed: false,
        })
    }
}
struct Writer {
    capture: RedactedCapture,
    closed: bool,
}
fn io_error(_: GadgetError) -> std::io::Error {
    std::io::Error::other("private guest output write failed")
}
impl AsyncWrite for Writer {
    fn poll_write(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        if self.closed {
            return Poll::Ready(Err(io_error(closed_error())));
        }
        Poll::Ready(
            self.capture
                .append(bytes)
                .map(|_| bytes.len())
                .map_err(io_error),
        )
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(self.capture.ready().map_err(io_error))
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        // Closing one handle must not finish shared stdout/stderr/log streams.
        self.closed = true;
        Poll::Ready(Ok(()))
    }
}
struct WasiWriter {
    capture: RedactedCapture,
    permit: usize,
    closed: bool,
}
fn stream_error(_: GadgetError) -> StreamError {
    StreamError::LastOperationFailed(wasmtime::Error::msg("private guest output write failed"))
}
#[async_trait::async_trait]
impl Pollable for WasiWriter {
    async fn ready(&mut self) {}
}
#[async_trait::async_trait]
impl OutputStream for WasiWriter {
    fn write(&mut self, bytes: Bytes) -> StreamResult<()> {
        if self.closed {
            return Err(StreamError::Closed);
        }
        self.capture.ready().map_err(stream_error)?;
        if bytes.len() > self.permit {
            return Err(StreamError::trap("private guest write exceeds its permit"));
        }
        self.permit = 0;
        self.capture.append(&bytes).map_err(stream_error)
    }
    fn flush(&mut self) -> StreamResult<()> {
        if self.closed {
            return Err(StreamError::Closed);
        }
        self.capture.ready().map_err(stream_error)
    }
    fn check_write(&mut self) -> StreamResult<usize> {
        if self.closed {
            return Err(StreamError::Closed);
        }
        self.capture.ready().map_err(stream_error)?;
        self.permit = WASI_PERMIT;
        Ok(self.permit)
    }
    async fn cancel(&mut self) {
        self.closed = true;
        self.permit = 0;
    }
}

#[cfg(test)]
#[path = "output_tests.rs"]
mod tests;
