//! Private HTTP/error diagnostics, not successful SSE/control/state publication.
//! An actual client issues Received only after send; bounded checked text never
//! supplies authority, billing, a logical-request meter or a public receipt.

use super::request::{Checked, Fault, Lease, Source};
use crate::exec::redaction::publication::{self, Output};
use crate::llm::{Received, Sent};
use std::sync::Arc;

const BODY_BYTES: usize = 1024 * 1024;
const DIAGNOSTIC_SCALARS: usize = 400;
const ATTEMPTS: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Http,
    Connection,
    Stream,
    Read,
    Utf8,
    Redaction,
    Limit,
    Stopped,
}

/// ORIGINAL adapter observations, never counts inferred from redacted text.
/// None means no body observation was selected; not a fabricated zero read.
/// No Debug/Serialize or public result/trace/usage conversion exists.
pub(crate) struct Attempt {
    pub(crate) ordinal: usize,
    pub(crate) status: Option<u16>,
    pub(crate) observed_bytes: Option<u64>,
    pub(crate) eof: bool,
    pub(crate) connection_error: bool,
    pub(crate) read_error: bool,
    pub(crate) truncated: bool,
    pub(crate) diagnostic_cut: bool,
}

/// Only a genuine error-body capture can issue checked detail; callers cannot
/// substitute a String, another request's text or a fabricated response label.
pub(crate) struct Detail {
    lease: Lease,
    ordinal: usize,
    text: String,
}

pub(crate) struct Diagnostic {
    lease: Lease,
    attempts: Vec<Attempt>,
    kind: Kind,
    checked_text: String,
}
impl Diagnostic {
    pub(crate) fn kind(&self) -> Kind {
        if self.lease.stopped() {
            Kind::Stopped
        } else {
            self.kind
        }
    }
    pub(crate) fn attempts(&self) -> &[Attempt] {
        &self.attempts
    }
    pub(crate) fn checked_text(&self) -> Result<&str, Fault> {
        self.lease.ready()?;
        Ok(&self.checked_text)
    }
    pub(crate) fn bind(&self, source: &Arc<Source>, request: &Checked) -> Result<(), Fault> {
        source.bind_lease(request, &self.lease)?;
        // Observing a late stop must not replace this object and lose receipts.
        // kind()/checked_text() observe the SAME latch on every access.
        let _ = self.kind();
        if self.attempts().len() > ATTEMPTS
            || self
                .attempts()
                .iter()
                .enumerate()
                .any(|(i, f)| f.ordinal != i + 1)
        {
            return Err(Fault::Collision);
        }
        if let Ok(text) = self.checked_text() {
            publication::verify_text(self.lease.redactor(), text).map_err(|_| Fault::Collision)?;
        }
        Ok(())
    }
}

/// Framework-facing error formatting stays fixed. Its checked detail/facts are
/// private, source-bound products; the app retains one bounded last failure.
pub(crate) struct Failure(pub(crate) Arc<Diagnostic>);
impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}
impl std::fmt::Debug for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}
impl std::error::Error for Failure {}

/// Source::lease alone supplies the original client/request identity. Received
/// adds actual response/attempt provenance, not an arbitrary reqwest response.
pub(crate) struct Call {
    lease: Lease,
    attempts: Vec<Attempt>,
}
impl Call {
    pub(crate) fn new(lease: Lease) -> Self {
        Self {
            lease,
            attempts: Vec::new(),
        }
    }
    pub(crate) fn lease(&self) -> Lease {
        self.lease.clone()
    }
    pub(crate) fn ready(&self) -> Result<(), Fault> {
        self.lease.ready()
    }
    /// Called immediately before the actual awaited send, never before its
    /// source/gate checks. These are transport attempts, NOT logical admissions.
    pub(crate) fn begin(&mut self, sent: Sent) -> Result<(), Fault> {
        let (lease, ordinal) = sent.into_parts();
        if !self.lease.same(&lease) || ordinal != self.attempts.len() + 1 {
            return Err(Fault::Collision);
        }
        if self.attempts.len() == ATTEMPTS {
            return Err(Fault::Limit);
        }
        self.attempts.push(Attempt {
            ordinal: self.attempts.len() + 1,
            status: None,
            observed_bytes: None,
            eof: false,
            connection_error: false,
            read_error: false,
            truncated: false,
            diagnostic_cut: false,
        });
        Ok(())
    }
    pub(crate) fn received(&mut self, received: &Received) -> Result<(), Fault> {
        let (lease, ordinal, status) = received.metadata();
        if !self.lease.same(lease) || self.attempts.last().map(|f| f.ordinal) != Some(ordinal) {
            return Err(Fault::Collision);
        }
        self.attempts.last_mut().unwrap().status = Some(status);
        Ok(())
    }
    pub(crate) fn connection_failed(&mut self) {
        self.attempts
            .last_mut()
            .expect("actual send precedes failure")
            .connection_error = true;
    }
    /// Only existing ORIGINAL incomplete-SSE counters are selected as facts.
    /// SSE bytes/parser/content/arguments remain unselected, not redacted views.
    pub(crate) fn stream_ended(&mut self, bytes: usize, read_error: bool) {
        let fact = self
            .attempts
            .last_mut()
            .expect("actual receipt precedes body");
        fact.observed_bytes = Some(bytes as u64);
        fact.eof = !read_error;
        fact.read_error = read_error;
        fact.truncated = true; // model stream incomplete even at real body EOF
    }
    pub(crate) async fn error_body(&mut self, received: Received) -> Result<Detail, Kind> {
        let (lease, ordinal, mut response) = received.into_parts();
        if !self.lease.same(&lease) || self.attempts.last().map(|f| f.ordinal) != Some(ordinal) {
            return Err(Kind::Redaction);
        }
        let status = response.status();
        if self.attempts.last().and_then(|f| f.status) != Some(status.as_u16())
            || status.is_success()
        {
            return Err(Kind::Redaction);
        }
        let mut capture = Capture::new(&lease);
        loop {
            if lease.ready().is_err() {
                return Err(Kind::Stopped);
            }
            self.attempts
                .last_mut()
                .unwrap()
                .observed_bytes
                .get_or_insert(0);
            let step = tokio::select! { biased;
                _ = lease.wait_stopped() => { return Err(Kind::Stopped); },
                step = response.chunk() => step,
            };
            match step {
                Ok(Some(bytes)) => {
                    let fact = self.attempts.last_mut().unwrap();
                    let observed = fact
                        .observed_bytes
                        .unwrap()
                        .checked_add(bytes.len() as u64)
                        .ok_or(Kind::Limit)?;
                    fact.observed_bytes = Some(observed); // before any work/view cut
                    #[cfg(test)]
                    lease.observed_for_tests(observed);
                    match capture.push(&bytes) {
                        Ok(()) => {}
                        Err(kind) => {
                            fact.truncated = true;
                            return Err(kind);
                        }
                    }
                    if lease.ready().is_err() {
                        return Err(Kind::Stopped);
                    }
                }
                Ok(None) => {
                    self.attempts.last_mut().unwrap().eof = true;
                    let view = match capture.finish() {
                        // ONLY genuine body EOF releases holdback
                        Ok(view) => view,
                        Err(kind) => {
                            self.attempts.last_mut().unwrap().truncated = true;
                            return Err(kind);
                        }
                    };
                    let cut = view.chars().count() > DIAGNOSTIC_SCALARS;
                    self.attempts.last_mut().unwrap().diagnostic_cut = cut;
                    let prefix = prefix(&view)?;
                    // The final cut/framing join is checked again BEFORE release.
                    let text = lease
                        .redactor()
                        .text(&format!("HTTP {status}: {prefix}"))
                        .map_err(|_| Kind::Redaction)?;
                    publication::verify_text(lease.redactor(), &text).map_err(classify)?;
                    lease.ready().map_err(|_| Kind::Stopped)?;
                    return Ok(Detail {
                        lease,
                        ordinal,
                        text,
                    });
                }
                Err(_) => {
                    let fact = self.attempts.last_mut().unwrap();
                    fact.read_error = true;
                    fact.truncated = true;
                    return Err(Kind::Read); // never format the raw library/URL error
                }
            }
        }
        // Dropping Capture after a cut/error/stop never calls finish.
    }
    pub(crate) fn failure(mut self, mut kind: Kind, detail: Option<Detail>) -> Diagnostic {
        let text = detail.and_then(|detail| {
            if self.lease.same(&detail.lease)
                && self
                    .attempts
                    .last()
                    .is_some_and(|f| f.ordinal == detail.ordinal && f.eof)
            {
                Some(detail.text)
            } else {
                kind = Kind::Redaction;
                None
            }
        });
        if self.lease.stopped() {
            kind = Kind::Stopped;
            if let Some(fact) = self.attempts.last_mut() {
                fact.truncated |= !fact.eof;
            }
        }
        Diagnostic {
            lease: self.lease,
            attempts: self.attempts,
            kind,
            checked_text: if kind == Kind::Stopped {
                "[REDACTED]".into()
            } else {
                text.unwrap_or_else(|| "[REDACTED]".into())
            },
        }
    }
}

/// Only a received-response lease selects the frozen source. No raw accessor,
/// arbitrary redactor constructor, charset repair, spill or suffix finalizer.
struct Capture {
    output: Output,
    input_bytes: usize,
}
impl Capture {
    fn new(lease: &Lease) -> Self {
        Self {
            output: Output::new(lease.redactor()),
            input_bytes: 0,
        }
    }
    fn push(&mut self, bytes: &[u8]) -> Result<(), Kind> {
        self.input_bytes = self
            .input_bytes
            .checked_add(bytes.len())
            .ok_or(Kind::Limit)?;
        if self.input_bytes > BODY_BYTES {
            return Err(Kind::Limit); // discard ALL detail, never publish an unadmitted prefix
        }
        if !self.output.push(bytes).map_err(classify)? {
            return Err(Kind::Limit); // retention cut: held source/rendered suffixes are aborted
        }
        Ok(())
    }
    fn finish(self) -> Result<String, Kind> {
        let view = self.output.finish(true).map_err(classify)?;
        if view.truncated {
            Err(Kind::Limit)
        } else {
            Ok(view.text)
        }
    }
}
fn classify(fault: publication::Fault) -> Kind {
    match fault {
        publication::Fault::Utf8 => Kind::Utf8,
        publication::Fault::Redaction | publication::Fault::Rendered => Kind::Redaction,
        publication::Fault::Limit => Kind::Limit,
    }
}
fn prefix(text: &str) -> Result<&str, Kind> {
    let end = text
        .char_indices()
        .nth(DIAGNOSTIC_SCALARS)
        .map_or(text.len(), |(i, _)| i);
    // Never present a clipped replacement marker as a repaired diagnostic view.
    if text
        .match_indices("[REDACTED]")
        .any(|(i, _)| i < end && i + 10 > end)
    {
        return Err(Kind::Redaction);
    }
    Ok(&text[..end])
}
