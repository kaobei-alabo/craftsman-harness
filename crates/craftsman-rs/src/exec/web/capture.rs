//! Original-byte web capture before any publication/read/parser cut. No raw
//! accessor/Debug/Serialize/spill. Only genuine EOF finishes delayed state.

use super::html::Html;
use crate::exec::redaction::publication::{Output, View};
use crate::exec::redaction::{ByteStream, Redactor};
const CHUNK: usize = 8192;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Fault {
    Redaction,
    Rendered,
    Utf8,
    Syntax,
    Limit,
}
impl From<crate::exec::redaction::publication::Fault> for Fault {
    fn from(value: crate::exec::redaction::publication::Fault) -> Self {
        use crate::exec::redaction::publication::Fault as F;
        match value {
            F::Redaction => Self::Redaction,
            F::Rendered => Self::Rendered,
            F::Utf8 => Self::Utf8,
            F::Limit => Self::Limit,
        }
    }
}
pub(super) struct Capture {
    original: ByteStream,
    rendered: ByteStream,
    original_utf8: Vec<u8>,
    decoded_utf8: Vec<u8>,
    html: Option<Html>,
    output: Output,
    admitted: usize,
    limit: usize,
    observed: u64,
    cut: bool,
}
impl Capture {
    pub(super) fn new(redactor: &Redactor, html: bool, limit: usize) -> Self {
        Self {
            original: redactor.byte_stream(),
            rendered: redactor.byte_stream(),
            original_utf8: Vec::new(),
            decoded_utf8: Vec::new(),
            html: html.then(Html::new),
            output: Output::new(redactor),
            admitted: 0,
            limit,
            observed: 0,
            cut: false,
        }
    }
    pub(super) fn observed(&self) -> u64 {
        self.observed
    }
    pub(super) fn push(&mut self, bytes: &[u8]) -> Result<bool, Fault> {
        self.observed = self
            .observed
            .checked_add(bytes.len() as u64)
            .ok_or(Fault::Limit)?;
        if self.cut {
            return Err(Fault::Limit);
        }
        let admit = bytes.len().min(self.limit - self.admitted);
        self.admitted += admit;
        for chunk in bytes[..admit].chunks(CHUNK) {
            validate(&mut self.original_utf8, chunk, false)?;
            let safe = self.original.push(chunk).map_err(|_| Fault::Redaction)?;
            if !self.verify(&safe)? {
                self.abort_pending();
                return Ok(false);
            }
        }
        if admit < bytes.len() || self.admitted == self.limit {
            self.abort_pending();
            return Ok(false);
        }
        Ok(true)
    }
    fn verify(&mut self, bytes: &[u8]) -> Result<bool, Fault> {
        for chunk in bytes.chunks(64 * 1024) {
            let safe = self.rendered.push(chunk).map_err(|_| Fault::Redaction)?;
            if self.rendered.matched() {
                return Err(Fault::Rendered);
            }
            if !self.decode(&safe, false)? {
                return Ok(false);
            }
        }
        Ok(true)
    }
    fn decode(&mut self, bytes: &[u8], final_chunk: bool) -> Result<bool, Fault> {
        self.decoded_utf8.extend_from_slice(bytes);
        let valid = valid_prefix(&self.decoded_utf8, final_chunk)?;
        let text = String::from_utf8(self.decoded_utf8.drain(..valid).collect())
            .map_err(|_| Fault::Utf8)?;
        match &mut self.html {
            Some(html) => html.push(&text, &mut self.output),
            None => self.output.frame(&text).map_err(Into::into),
        }
    }
    fn abort_pending(&mut self) {
        self.original.abort();
        self.rendered.abort();
        self.original_utf8.clear();
        self.decoded_utf8.clear();
        if let Some(html) = &mut self.html {
            html.abort();
        }
        self.cut = true;
    }
    pub(super) fn finish(mut self, eof: bool) -> Result<View, Fault> {
        let mut complete = eof && !self.cut;
        if complete {
            validate(&mut self.original_utf8, &[], true)?;
            let safe = self.original.finish().map_err(|_| Fault::Redaction)?;
            complete = self.verify(&safe)?;
            if complete {
                let safe = self.rendered.finish().map_err(|_| Fault::Redaction)?;
                complete = self.decode(&safe, true)?;
            }
            if complete {
                if let Some(html) = &mut self.html {
                    complete = html.finish(&mut self.output)?;
                }
            }
        }
        if !complete {
            self.abort_pending();
        }
        self.output.finish(complete).map_err(Into::into)
    }
}
pub(super) fn valid_prefix(bytes: &[u8], final_chunk: bool) -> Result<usize, Fault> {
    match std::str::from_utf8(bytes) {
        Ok(_) => Ok(bytes.len()),
        Err(error) if error.error_len().is_none() && !final_chunk => Ok(error.valid_up_to()),
        Err(_) => Err(Fault::Utf8),
    }
}
pub(super) fn validate(
    pending: &mut Vec<u8>,
    bytes: &[u8],
    final_chunk: bool,
) -> Result<(), Fault> {
    pending.extend_from_slice(bytes);
    let valid = valid_prefix(pending, final_chunk)?;
    pending.drain(..valid);
    Ok(())
}

/// JSON input is a bounded ORIGINAL complete record, not a truncated publication.
/// Literal matching runs before admission; partial/oversize JSON is rejected,
/// never published/repaired. Structured redaction follows complete strict parsing.
pub(super) struct JsonInput {
    original: ByteStream,
    utf8: Vec<u8>,
    bytes: Vec<u8>,
    limit: usize,
    observed: u64,
}
impl JsonInput {
    pub(super) fn new(redactor: &Redactor, limit: usize) -> Self {
        Self {
            original: redactor.byte_stream(),
            utf8: Vec::new(),
            bytes: Vec::new(),
            limit,
            observed: 0,
        }
    }
    pub(super) fn observed(&self) -> u64 {
        self.observed
    }
    pub(super) fn push(&mut self, bytes: &[u8]) -> Result<(), Fault> {
        self.observed = self
            .observed
            .checked_add(bytes.len() as u64)
            .ok_or(Fault::Limit)?;
        let remaining = self.limit - self.bytes.len();
        for chunk in bytes[..bytes.len().min(remaining)].chunks(CHUNK) {
            validate(&mut self.utf8, chunk, false)?;
            self.original.push(chunk).map_err(|_| Fault::Redaction)?; // never publish this syntax-altering view
            self.bytes.extend_from_slice(chunk);
        }
        if bytes.len() > remaining {
            self.abort();
            return Err(Fault::Limit);
        }
        Ok(())
    }
    pub(super) fn finish(mut self) -> Result<serde_json::Value, Fault> {
        validate(&mut self.utf8, &[], true)?;
        self.original.finish().map_err(|_| Fault::Redaction)?;
        serde_json::from_slice(&self.bytes).map_err(|_| Fault::Syntax)
    }
    pub(super) fn abort(&mut self) {
        self.original.abort();
        self.utf8.clear();
        self.bytes.clear();
    }
}

#[cfg(test)]
mod tests;
