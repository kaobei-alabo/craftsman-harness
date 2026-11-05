//! Private complete-answer and fixed-diagnostic human display, never raw IO.
//! Source is the owned Run's immutable output, not JSON decoding or retained state.
use super::{Error, Facts, Scope};
use crate::exec::provider::request::{Fault, verify_message_frame};
use crate::exec::redaction::Redactor;

const CAP: usize = 1024 * 1024;
const HEX: &[u8] = b"0123456789abcdef";

/// Two immutable, LF-terminated checked display channels and actual private facts.
/// No Debug/Serialize/raw constructor, outcome/exit mapping, execution or writer.
pub(crate) struct Human<'a> {
    scope: Scope<'a>,
    facts: Facts,
    answer: Option<Vec<u8>>,
    diagnostic: Vec<u8>,
}
impl Human<'_> {
    pub(crate) fn answer(&self) -> Result<Option<&[u8]>, Error> {
        self.scope.ready(self.facts.approval_required)?;
        Ok(self.answer.as_deref())
    }
    pub(crate) fn diagnostic(&self) -> Result<&[u8], Error> {
        self.scope.ready(self.facts.approval_required)?;
        Ok(&self.diagnostic)
    }
    pub(crate) fn facts(&self) -> Result<Facts, Error> {
        self.scope.ready(self.facts.approval_required)?;
        Ok(self.facts)
    }
    pub(crate) fn bind(&self, other: &Scope<'_>) -> Result<(), Error> {
        if !self.scope.same(other) {
            return Err(Error::Foreign);
        }
        self.scope.ready(self.facts.approval_required)?;
        other.ready(self.facts.approval_required)
    }
}

pub(super) fn encode(
    scope: Scope<'_>,
    max_bytes: usize,
    after: impl FnOnce(),
) -> Result<Human<'_>, Error> {
    let facts = Facts {
        returned: scope.runtime.returned(),
        approval_required: scope.runtime.approval_required(),
    };
    scope.ready(facts.approval_required)?;
    let result = (|| {
        if max_bytes > CAP {
            return Err(Fault::Limit);
        }
        let redactor = scope.runtime.redactor();
        let answer = scope
            .runtime
            .output()
            .map(|original| {
                // SAME conservative terminal-output work admission: twice the
                // original bytes plus the complete redacted view <=1MiB.
                // Reject before copying/redacting oversized original input.
                let original_work = original
                    .len()
                    .checked_mul(2)
                    .filter(|n| *n <= CAP)
                    .ok_or(Fault::Limit)?;
                let safe = redactor.text(original)?;
                if safe.len() > CAP - original_work {
                    return Err(Fault::Limit);
                }
                // Whole decoded text BEFORE any display change, never a cut,
                // raw-error fallback or caller-provided redacted constructor.
                verify_message_frame(redactor, safe.as_bytes())?;
                Ok::<_, Fault>(safe)
            })
            .transpose()?;
        // Typed observations only. These are NOT stdout/stderr eligibility,
        // task correctness, public status/exit/ACP/usage or raw error details.
        let diagnostic = format!(
            "craftsman: private run return={}; approval_required={}; final_answer={}\n",
            facts.returned.tag(),
            facts.approval_required,
            if answer.is_some() {
                "present"
            } else {
                "absent"
            },
        );
        // Fixed text/scalars must remain exact or fail; never redact a fact.
        verify_message_frame(redactor, diagnostic.as_bytes())?;
        let answer_size = answer.as_deref().map(size).transpose()?;
        let diagnostic_size = size(&diagnostic)?;
        let total = diagnostic_size
            .checked_add(answer_size.unwrap_or(0))
            .filter(|n| *n <= max_bytes && *n <= CAP)
            .ok_or(Fault::Limit)?;
        // Exact COMPLETE two-channel budget before either rendered allocation.
        // No truncation, alternate escaping, omission, spill or lossy decoding.
        let answer = answer
            .as_deref()
            .zip(answer_size)
            .map(|(text, size)| render(text, size))
            .transpose()?;
        let diagnostic = render(&diagnostic, diagnostic_size)?;
        if answer.as_ref().map_or(0, Vec::len) + diagnostic.len() != total {
            return Err(Fault::Limit);
        }
        verify_pair(redactor, answer.as_deref(), &diagnostic)?;
        Ok::<_, Fault>((answer, diagnostic))
    })();
    after();
    // Actual lease/typed epoch changes dominate source/render/budget failures.
    scope.ready(facts.approval_required)?;
    let (answer, diagnostic) = result?;
    Ok(Human {
        scope,
        facts,
        answer,
        diagnostic,
    })
}

/// Escape exactly these display-dangerous scalars, not arbitrary Unicode or
/// normalization. LF alone is a physical separator; CR/TAB/ESC/DEL/C1, bidi
/// marks/embeddings/isolates, Unicode separators and data BOM are visible text.
fn escape(ch: char, previous: Option<char>) -> bool {
    (ch.is_control() && ch != '\n')
        || matches!(
            ch,
            '\u{061c}' | '\u{200e}'..='\u{200f}' | '\u{2028}'..='\u{202e}'
                | '\u{2066}'..='\u{2069}' | '\u{feff}'
        )
        // Break EVERY adjacent original colon/hash, not only line starts or
        // nonoverlapping pairs. Runs such as :::/### cannot leave a delimiter.
        || (matches!(ch, ':' | '#') && previous == Some(ch))
}
fn size(text: &str) -> Result<usize, Fault> {
    let mut size = 0usize;
    let mut previous = None;
    for ch in text.chars() {
        let width = if escape(ch, previous) {
            8 // lowercase visible \u{xxxx}; all selected scalars are BMP
        } else if ch == '\\' {
            2 // distinguish original literal escapes from generated ones
        } else {
            ch.len_utf8()
        };
        size = size
            .checked_add(width)
            .filter(|n| *n <= CAP)
            .ok_or(Fault::Limit)?;
        previous = Some(ch);
    }
    // Terminate every present channel, including a genuinely empty answer.
    // A missing answer stays None; this delimiter is not original model data.
    if previous != Some('\n') {
        size = size
            .checked_add(1)
            .filter(|n| *n <= CAP)
            .ok_or(Fault::Limit)?;
    }
    Ok(size)
}
fn render(text: &str, size: usize) -> Result<Vec<u8>, Fault> {
    if size > CAP {
        return Err(Fault::Limit);
    }
    let mut bytes = Vec::with_capacity(size);
    let mut previous = None;
    for ch in text.chars() {
        if escape(ch, previous) {
            let scalar = ch as u32;
            if scalar > 0xffff {
                return Err(Fault::Collision);
            }
            bytes.extend_from_slice(&[
                b'\\',
                b'u',
                b'{',
                HEX[((scalar >> 12) & 15) as usize],
                HEX[((scalar >> 8) & 15) as usize],
                HEX[((scalar >> 4) & 15) as usize],
                HEX[(scalar & 15) as usize],
                b'}',
            ]);
        } else if ch == '\\' {
            bytes.extend_from_slice(b"\\\\");
        } else {
            let mut utf8 = [0u8; 4];
            bytes.extend_from_slice(ch.encode_utf8(&mut utf8).as_bytes());
        }
        if bytes.len() > size {
            return Err(Fault::Limit);
        }
        previous = Some(ch);
    }
    if previous != Some('\n') {
        bytes.push(b'\n');
    }
    if bytes.len() != size {
        return Err(Fault::Limit);
    }
    Ok(bytes)
}
/// SAME unchanged bounded KMP stream, including new escapes/defanging/terminal
/// LF and complete-channel joins in BOTH fixed orders. No arbitrary concurrent
/// sink interleaving/OS encoding/atomic-send or retraction guarantee follows.
fn verify_pair(redactor: &Redactor, answer: Option<&[u8]>, diagnostic: &[u8]) -> Result<(), Fault> {
    let answer = answer.unwrap_or_default();
    if answer
        .len()
        .checked_add(diagnostic.len())
        .is_none_or(|n| n > CAP)
    {
        return Err(Fault::Limit);
    }
    for channels in [[answer, diagnostic], [diagnostic, answer]] {
        let mut stream = redactor.byte_stream();
        for channel in channels {
            for chunk in channel.chunks(64 * 1024) {
                stream.push(chunk)?;
                if stream.matched() {
                    stream.abort();
                    return Err(Fault::Collision);
                }
            }
        }
        stream.abort();
    }
    Ok(())
}
