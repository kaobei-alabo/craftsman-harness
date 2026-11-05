//! Shared bounded private text publication, not a capability constructor.
//! Producers supply their actual scope's frozen redactor BEFORE any own cuts.

use crate::exec::redaction::{ByteStream, Redactor};

pub(crate) const BUDGET: usize = 100_000;
const HEAD: usize = BUDGET - 1024;
const CHUNK: usize = 8192;
const NOTICE: &str = "\n… [output truncated; full result was not retained]";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Fault {
    Redaction,
    Rendered,
    Utf8,
    Limit,
}

pub(crate) struct View {
    pub(crate) text: String,
    pub(crate) truncated: bool,
}

pub(crate) struct Output {
    redactor: Redactor,
    redact: ByteStream,
    verify: ByteStream,
    input_utf8: Vec<u8>,
    output_utf8: Vec<u8>,
    head: Vec<u8>,
    truncated: bool,
}
impl Output {
    pub(crate) fn new(redactor: &Redactor) -> Self {
        Self {
            redactor: redactor.clone(),
            redact: redactor.byte_stream(),
            verify: redactor.byte_stream(),
            input_utf8: Vec::new(),
            output_utf8: Vec::new(),
            head: Vec::new(),
            truncated: false,
        }
    }
    pub(crate) fn frame(&mut self, text: &str) -> Result<bool, Fault> {
        self.push(text.as_bytes())
    }
    /// False means a real retention cut. Callers stop generation and abort,
    /// never finish as though all remaining source/framing had been consumed.
    pub(crate) fn push(&mut self, bytes: &[u8]) -> Result<bool, Fault> {
        if self.truncated {
            return Ok(false);
        }
        for chunk in bytes.chunks(CHUNK) {
            validate(&mut self.input_utf8, chunk, false)?;
            let safe = self.redact.push(chunk).map_err(|_| Fault::Redaction)?;
            self.verify_bytes(&safe)?;
            if self.truncated {
                return Ok(false);
            }
        }
        Ok(true)
    }
    fn verify_bytes(&mut self, bytes: &[u8]) -> Result<(), Fault> {
        for chunk in bytes.chunks(64 * 1024) {
            let safe = self.verify.push(chunk).map_err(|_| Fault::Redaction)?;
            if self.verify.matched() {
                return Err(Fault::Rendered);
            }
            self.retain(&safe, false)?;
        }
        Ok(())
    }
    fn retain(&mut self, bytes: &[u8], final_chunk: bool) -> Result<(), Fault> {
        self.output_utf8.extend_from_slice(bytes);
        let valid = valid_prefix(&self.output_utf8, final_chunk)?;
        let ready = self.output_utf8.drain(..valid);
        let remaining = HEAD - self.head.len();
        for (index, byte) in ready.enumerate() {
            if index < remaining {
                self.head.push(byte);
            } else {
                self.truncated = true;
            }
        }
        Ok(())
    }
    pub(crate) fn finish(mut self, complete: bool) -> Result<View, Fault> {
        if complete {
            validate(&mut self.input_utf8, &[], true)?;
            let safe = self.redact.finish().map_err(|_| Fault::Redaction)?;
            self.verify_bytes(&safe)?;
            let safe = self.verify.finish().map_err(|_| Fault::Redaction)?;
            self.retain(&safe, true)?;
        } else {
            self.redact.abort();
            self.verify.abort();
            self.input_utf8.clear();
            self.output_utf8.clear();
            self.truncated = true;
        }
        let end = valid_prefix(&self.head, false)?;
        self.truncated |= end != self.head.len();
        self.head.truncate(end);
        let mut text = String::from_utf8(self.head).map_err(|_| Fault::Utf8)?;
        if self.truncated {
            text.push_str(NOTICE);
        }
        // The cut/notice join may create a value not present in an earlier
        // record. Redact it before the final bound; verify the exact text too.
        text = self.redactor.text(&text).map_err(|_| Fault::Redaction)?;
        verify_text(&self.redactor, &text)?;
        Ok(View {
            text,
            truncated: self.truncated,
        })
    }
}

fn valid_prefix(bytes: &[u8], final_chunk: bool) -> Result<usize, Fault> {
    match std::str::from_utf8(bytes) {
        Ok(_) => Ok(bytes.len()),
        Err(error) if error.error_len().is_none() && !final_chunk => Ok(error.valid_up_to()),
        Err(_) => Err(Fault::Utf8),
    }
}
fn validate(pending: &mut Vec<u8>, bytes: &[u8], final_chunk: bool) -> Result<(), Fault> {
    pending.extend_from_slice(bytes);
    let valid = valid_prefix(pending, final_chunk)?;
    pending.drain(..valid);
    Ok(())
}

pub(crate) fn verify_text(redactor: &Redactor, text: &str) -> Result<(), Fault> {
    if text.len() > BUDGET {
        return Err(Fault::Limit);
    }
    let mut verify = redactor.byte_stream();
    for chunk in text.as_bytes().chunks(64 * 1024) {
        verify.push(chunk).map_err(|_| Fault::Redaction)?;
        if verify.matched() {
            return Err(Fault::Rendered);
        }
    }
    verify.abort();
    Ok(())
}
