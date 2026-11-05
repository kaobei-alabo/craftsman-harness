//! Private ASCII projection and exact-byte whole-message selection.
//! Only verified terminal channels/actual scope enter; no raw JSON/IO constructor.
use super::{Error, Facts, Scope};
use crate::exec::messages::checked_message_layout;
use crate::exec::provider::request::{Fault, terminal_channels, verify_message_frame};
use crate::exec::redaction::Redactor;
const CAP: usize = 1024 * 1024;
// New projection staging only. Existing original/view/UTF-8 frame caps stay 1MiB;
// one source byte can expand to at most six ASCII bytes (DEL -> \u007f).
const PROJECTION: usize = 6 * CAP;
const END: &[u8] = b"],\"messages_omitted\":";
const HEX: &[u8] = b"0123456789abcdef";

/// Immutable ASCII-only checked fragment, bound to the actual terminal lease.
/// No Debug/Serialize/raw constructor, public outcome mapping or IO interface.
pub(crate) struct Ascii<'a> {
    scope: Scope<'a>,
    facts: Facts,
    count: usize,
    omitted: usize,
    bytes: Vec<u8>,
}
impl Ascii<'_> {
    pub(crate) fn bytes(&self) -> Result<&[u8], Error> {
        self.scope.ready(self.facts.approval_required)?;
        Ok(&self.bytes)
    }
    pub(crate) fn facts(&self) -> Result<Facts, Error> {
        self.scope.ready(self.facts.approval_required)?;
        Ok(self.facts)
    }
    pub(crate) fn message_count(&self) -> Result<usize, Error> {
        self.scope.ready(self.facts.approval_required)?;
        Ok(self.count)
    }
    pub(crate) fn messages_omitted(&self) -> Result<usize, Error> {
        self.scope.ready(self.facts.approval_required)?;
        Ok(self.omitted)
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
) -> Result<Ascii<'_>, Error> {
    let facts = Facts {
        returned: scope.runtime.returned(),
        approval_required: scope.runtime.approval_required(),
    };
    scope.ready(facts.approval_required)?;
    let result = (|| {
        if max_bytes > CAP {
            return Err(Error::Codec(Fault::Limit));
        }
        let state = scope
            .runtime
            .state()
            .lock()
            .map_err(|_| Error::Codec(Fault::Collision))?;
        // Complete original admission/copies and UTF-8 literal checks remain
        // unchanged. ASCII projection of BOTH full channels precedes selection.
        let (prefix, messages) = terminal_channels(
            scope.runtime.redactor(),
            &state.messages,
            scope.runtime.output(),
            facts.returned.tag(),
            facts.approval_required,
        )?;
        let total = state.messages.len();
        let (starts, body) = checked_message_layout(&messages, total)?;
        let mut marks = Vec::with_capacity(starts.len() + 2);
        marks.push(body.start);
        marks.extend(starts);
        marks.push(body.end);
        let prefix = project(scope.runtime.redactor(), &prefix, &[])?.bytes;
        let projected = project(scope.runtime.redactor(), &messages, &marks)?;
        let body_start = projected.offsets[0];
        let body_end = *projected.offsets.last().ok_or(Fault::Collision)?;
        let starts = &projected.offsets[1..projected.offsets.len() - 1];
        if prefix.last() != Some(&b'}') || body_start == 0 {
            return Err(Error::Codec(Fault::Collision));
        }
        for omitted in 0..=total {
            let start = starts.get(omitted).copied().unwrap_or(body_end);
            let digits = omitted.to_string();
            let size = prefix
                .len()
                .checked_add(body_start - 1)
                .and_then(|n| n.checked_add(body_end - start))
                .and_then(|n| n.checked_add(END.len() + digits.len() + 1))
                .ok_or(Fault::Limit)?;
            if size > max_bytes {
                continue;
            }
            let mut bytes = Vec::with_capacity(size);
            bytes.extend_from_slice(&prefix[..prefix.len() - 1]);
            bytes.push(b',');
            bytes.extend_from_slice(&projected.bytes[1..body_start]);
            bytes.extend_from_slice(&projected.bytes[start..body_end]);
            bytes.extend_from_slice(END);
            bytes.extend_from_slice(digits.as_bytes());
            bytes.push(b'}');
            if bytes.len() != size
                || bytes.len() > max_bytes
                || !bytes.iter().all(|b| (0x20..=0x7e).contains(b))
            {
                return Err(Error::Codec(Fault::Limit));
            }
            // Exact post-escape framing/omission joins. No alternate layout
            // may hide a newly synthesized literal or change an original fact.
            verify_message_frame(scope.runtime.redactor(), &bytes)?;
            return Ok((total - omitted, omitted, bytes));
        }
        Err(Error::Codec(Fault::Limit))
    })();
    after();
    scope.ready(facts.approval_required)?;
    let (count, omitted, bytes) = result?;
    Ok(Ascii {
        scope,
        facts,
        count,
        omitted,
        bytes,
    })
}

struct Projected {
    bytes: Vec<u8>,
    offsets: Vec<usize>,
}
/// Deterministic representation change of already checked compact JSON only.
/// All structural marks come from the unchanged old span walk; no decoding,
/// normalization, repair or unsafe input admission/parser is introduced.
fn project(redactor: &Redactor, source: &[u8], marks: &[usize]) -> Result<Projected, Fault> {
    if source.len() > CAP || marks.len() > 258 {
        return Err(Fault::Limit);
    }
    let text = std::str::from_utf8(source).map_err(|_| Fault::Collision)?;
    if marks.windows(2).any(|w| w[0] > w[1]) || marks.last().is_some_and(|m| *m > source.len()) {
        return Err(Fault::Collision);
    }
    let mut size = 0usize;
    let mut next = 0;
    let mut offsets = Vec::with_capacity(marks.len());
    for (index, ch) in text.char_indices() {
        if marks.get(next).is_some_and(|m| *m < index) {
            return Err(Fault::Collision);
        }
        while marks.get(next) == Some(&index) {
            offsets.push(size);
            next += 1;
        }
        let width = if ch.is_ascii() && ch != '\u{007f}' {
            1
        } else {
            6 * ch.len_utf16()
        };
        if ch <= '\u{001f}' {
            return Err(Fault::Collision);
        }
        size = size
            .checked_add(width)
            .filter(|n| *n <= PROJECTION)
            .ok_or(Fault::Limit)?;
    }
    while marks.get(next) == Some(&source.len()) {
        offsets.push(size);
        next += 1;
    }
    if next != marks.len() {
        return Err(Fault::Collision);
    }
    let mut bytes = Vec::with_capacity(size);
    let mut string = false;
    let mut escaped = false;
    for ch in text.chars() {
        if ch.is_ascii() && ch != '\u{007f}' {
            let b = ch as u8;
            bytes.push(b);
            if string {
                if escaped {
                    escaped = false;
                } else if b == b'\\' {
                    escaped = true;
                } else if b == b'"' {
                    string = false;
                }
            } else if b == b'"' {
                string = true;
            }
        } else {
            if !string || escaped {
                return Err(Fault::Collision);
            }
            let mut units = [0u16; 2];
            for unit in ch.encode_utf16(&mut units) {
                bytes.extend_from_slice(&[
                    b'\\',
                    b'u',
                    HEX[(((*unit) >> 12) & 15) as usize],
                    HEX[(((*unit) >> 8) & 15) as usize],
                    HEX[(((*unit) >> 4) & 15) as usize],
                    HEX[((*unit) & 15) as usize],
                ]);
            }
        }
    }
    if string || escaped || bytes.len() != size {
        return Err(Fault::Collision);
    }
    // SAME KMP stream, unchanged seed/chunk/position bounds. Complete expanded
    // channel may be >final CAP but remains <=6*CAP staging; never cut/finish it
    // to conceal an unsafe old record. No spill/raw publication is performed.
    let mut verify = redactor.byte_stream();
    for chunk in bytes.chunks(64 * 1024) {
        verify.push(chunk)?;
        if verify.matched() {
            return Err(Fault::Collision);
        }
    }
    verify.abort();
    Ok(Projected { bytes, offsets })
}
