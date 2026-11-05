//! Private per-pipe original-byte redaction before bounded head/tail retention.
//! No raw accessor, Debug/Serialize, lossy decoding or spill-file authority.

use crate::exec::redaction::{ByteStream, RedactionError, Redactor};
use std::collections::VecDeque;

const MAX_CHUNK: usize = 64 * 1024;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Fault {
    Redaction,
    Rendered,
    Utf8,
    Read,
}
impl From<RedactionError> for Fault {
    fn from(_: RedactionError) -> Self {
        Self::Redaction
    }
}

pub(super) struct Capture {
    original: ByteStream,
    rendered: ByteStream,
    utf8: Vec<u8>,
    original_utf8: Vec<u8>,
    head: Vec<u8>,
    tail: VecDeque<u8>,
    head_cap: usize,
    tail_cap: usize,
    raw_bytes: u64,
    rendered_bytes: u64,
    eof: bool,
    closed: bool,
    omitted: bool,
    fault: Option<Fault>,
}
impl Capture {
    pub(super) fn new(redactor: &Redactor, head_cap: usize, tail_cap: usize) -> Self {
        Self {
            original: redactor.byte_stream(),
            rendered: redactor.byte_stream(),
            utf8: Vec::new(),
            original_utf8: Vec::new(),
            head: Vec::new(),
            tail: VecDeque::new(),
            head_cap,
            tail_cap,
            raw_bytes: 0,
            rendered_bytes: 0,
            eof: false,
            closed: false,
            omitted: false,
            fault: None,
        }
    }
    fn fail(&mut self, fault: Fault) -> Fault {
        self.fault = Some(fault);
        self.original.abort();
        self.rendered.abort();
        self.utf8.clear();
        self.original_utf8.clear();
        self.head.clear();
        self.tail.clear();
        self.omitted = true;
        fault
    }
    pub(super) fn push(&mut self, bytes: &[u8]) -> Result<(), Fault> {
        self.raw_bytes = self
            .raw_bytes
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| self.fail(Fault::Redaction))?;
        if self.closed {
            return Err(self.fail(Fault::Redaction));
        }
        if let Some(fault) = self.fault {
            return Err(fault);
        }
        if bytes.len() > MAX_CHUNK {
            return Err(self.fail(Fault::Redaction));
        }
        // Captured production seeds are UTF-8. Reject invalid ORIGINAL pipe
        // bytes immediately even if redaction lookahead would delay emission;
        // otherwise an invalid byte followed by sleep could become a timeout.
        if let Err(fault) = self.validate_original(bytes, false) {
            return Err(self.fail(fault));
        }
        let result = self
            .original
            .push(bytes)
            .map_err(Fault::from)
            .and_then(|safe| self.verify(&safe));
        result.map_err(|f| self.fail(f))
    }
    fn validate_original(&mut self, bytes: &[u8], final_chunk: bool) -> Result<(), Fault> {
        self.original_utf8.extend_from_slice(bytes);
        match std::str::from_utf8(&self.original_utf8) {
            Ok(_) => self.original_utf8.clear(),
            Err(e) if e.error_len().is_none() && !final_chunk => {
                self.original_utf8.drain(..e.valid_up_to());
            }
            Err(_) => return Err(Fault::Utf8),
        }
        Ok(())
    }
    fn verify(&mut self, bytes: &[u8]) -> Result<(), Fault> {
        for chunk in bytes.chunks(MAX_CHUNK) {
            let safe = self.rendered.push(chunk)?;
            if self.rendered.matched() {
                return Err(Fault::Rendered);
            }
            self.decode(&safe, false)?;
        }
        Ok(())
    }
    fn decode(&mut self, bytes: &[u8], final_chunk: bool) -> Result<(), Fault> {
        self.utf8.extend_from_slice(bytes);
        let valid = match std::str::from_utf8(&self.utf8) {
            Ok(_) => self.utf8.len(),
            Err(e) if e.error_len().is_none() && !final_chunk => e.valid_up_to(),
            Err(_) => return Err(Fault::Utf8),
        };
        let ready: Vec<_> = self.utf8.drain(..valid).collect();
        self.rendered_bytes = self
            .rendered_bytes
            .checked_add(ready.len() as u64)
            .ok_or(Fault::Redaction)?;
        let take = ready.len().min(self.head_cap - self.head.len());
        self.head.extend_from_slice(&ready[..take]);
        if self.tail_cap > 0 {
            self.tail.extend(ready);
            let excess = self.tail.len().saturating_sub(self.tail_cap);
            self.tail.drain(..excess);
        }
        Ok(())
    }
    pub(super) fn eof(&mut self) {
        self.eof = true;
    }
    pub(super) fn read_failed(&mut self) {
        self.fail(Fault::Read);
    }
    /// Only actual EOF AND normal command completion can release holdback.
    /// Caps, cancellation, timeout and error discard delayed suffixes instead.
    pub(super) fn finish(&mut self, normal: bool) -> View {
        if !self.closed {
            if normal && self.eof && self.fault.is_none() {
                let result = (|| {
                    self.validate_original(&[], true)?;
                    let tail = self.original.finish()?;
                    self.verify(&tail)?;
                    let tail = self.rendered.finish()?;
                    self.decode(&tail, true)
                })();
                if let Err(f) = result {
                    self.fail(f);
                }
            } else {
                let original = self.original.abort();
                let rendered = self.rendered.abort();
                self.omitted |= original
                    || rendered
                    || !self.utf8.is_empty()
                    || !self.original_utf8.is_empty()
                    || !self.eof;
                self.utf8.clear();
                self.original_utf8.clear();
            }
            self.closed = true;
        }
        View {
            head: self.head.clone(),
            tail: self.tail.iter().copied().collect(),
            raw_bytes: self.raw_bytes,
            rendered_bytes: self.rendered_bytes,
            omitted: self.omitted,
            fault: self.fault,
        }
    }
}

pub(super) struct View {
    head: Vec<u8>,
    tail: Vec<u8>,
    pub(super) raw_bytes: u64,
    pub(super) rendered_bytes: u64,
    pub(super) omitted: bool,
    pub(super) fault: Option<Fault>,
}
impl View {
    pub(super) fn complete(&self) -> bool {
        !self.omitted && self.rendered_bytes == self.head.len() as u64
    }
    pub(super) fn head_text(&self) -> Result<String, Fault> {
        prefix(&self.head, self.head.len())
    }
    pub(super) fn body(&self, budget: usize, label: &'static str) -> Result<(String, bool), Fault> {
        if self.complete() && self.head.len() <= budget {
            return Ok((self.head_text()?, false));
        }
        // Reserve room for truthful rendered-byte omission diagnostics. Original
        // process byte counts are separate and never inferred from this text.
        let half = budget.saturating_sub(256) / 2;
        let head = prefix(&self.head, half)?;
        let room = self.rendered_bytes.saturating_sub(head.len() as u64) as usize;
        let tail = suffix(&self.tail, half.min(room))?;
        let omitted = self
            .rendered_bytes
            .saturating_sub((head.len() + tail.len()) as u64);
        Ok((
            format!(
                "{head}\n… [{omitted} rendered bytes of {label} omitted; full output was not retained] …\n{tail}"
            ),
            true,
        ))
    }
}
fn prefix(bytes: &[u8], limit: usize) -> Result<String, Fault> {
    let bytes = &bytes[..bytes.len().min(limit)];
    match std::str::from_utf8(bytes) {
        Ok(s) => Ok(s.to_owned()),
        Err(e) if e.error_len().is_none() => std::str::from_utf8(&bytes[..e.valid_up_to()])
            .map(str::to_owned)
            .map_err(|_| Fault::Utf8),
        Err(_) => Err(Fault::Utf8),
    }
}
fn suffix(bytes: &[u8], limit: usize) -> Result<String, Fault> {
    let mut start = bytes.len().saturating_sub(limit);
    while start < bytes.len() && bytes[start] & 0xc0 == 0x80 {
        start += 1;
    }
    std::str::from_utf8(&bytes[start..])
        .map(str::to_owned)
        .map_err(|_| Fault::Utf8)
}

#[cfg(test)]
mod tests;
