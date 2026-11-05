//! Private whole-message suffix selection of an already checked finite View.
//! No raw source, final outcome, executable argument or publication constructor.
use super::View;
use crate::exec::provider::request::{Fault, verify_message_frame};
use crate::exec::runtime::MessageScope;

const CAP: usize = 1024 * 1024;
const MESSAGES: usize = 256;
// The private snapshot encoder's exact compact framing, not a public schema.
const PREFIX: &[u8] = b"{\"message_scope\":\"retained_state\",\"messages\":[";
const ORIGINAL_END: &[u8] = b"],\"messages_omitted\":0}";
const END: &[u8] = b"],\"messages_omitted\":";
// Original argument depth 64 plus fixed message/call/frame containers.
const STACK: usize = 70;

/// Immutable checked selected bytes and actual retained-snapshot counts only.
/// No Debug/Serialize/raw constructor, status, usage, final answer or writer.
pub(crate) struct Selection<'a> {
    scope: MessageScope<'a>,
    count: usize,
    omitted: usize,
    bytes: Vec<u8>,
}
impl Selection<'_> {
    pub(crate) fn bytes(&self) -> Result<&[u8], Fault> {
        self.scope.ready()?;
        Ok(&self.bytes)
    }
    pub(crate) fn message_count(&self) -> Result<usize, Fault> {
        self.scope.ready()?;
        Ok(self.count)
    }
    pub(crate) fn messages_omitted(&self) -> Result<usize, Fault> {
        self.scope.ready()?;
        Ok(self.omitted)
    }
    pub(crate) fn bind(&self, scope: &MessageScope<'_>) -> Result<(), Fault> {
        if !self.scope.same(scope) {
            return Err(Fault::Collision);
        }
        self.scope.ready()?;
        scope.ready()
    }
}

pub(super) fn select(view: View<'_>, max_bytes: usize) -> Result<Selection<'_>, Fault> {
    select_after(view, max_bytes, || {})
}

// Only the test-only View method can supply a callback; production uses no hook.
pub(super) fn select_after(
    view: View<'_>,
    max_bytes: usize,
    after: impl FnOnce(),
) -> Result<Selection<'_>, Fault> {
    view.scope.ready()?;
    let result = choose(&view, max_bytes);
    after();
    // Typed late approval dominates success AND every codec/bound failure.
    view.scope.ready()?;
    let (omitted, replacement) = result?;
    Ok(Selection {
        scope: view.scope,
        count: view.count - omitted,
        omitted,
        bytes: replacement.unwrap_or(view.bytes),
    })
}

fn choose(view: &View<'_>, max_bytes: usize) -> Result<(usize, Option<Vec<u8>>), Fault> {
    if max_bytes > CAP {
        return Err(Fault::Limit);
    }
    let (starts, end) = layout(&view.bytes, view.count)?;
    // Oldest-first omission; the first fitting choice is the largest contiguous
    // newest suffix. At most 257 arithmetic candidates, ONE final literal check.
    for omitted in 0..=view.count {
        let start = starts.get(omitted).copied().unwrap_or(end);
        let digits = omitted.to_string();
        let size = PREFIX
            .len()
            .checked_add(end - start)
            .and_then(|n| n.checked_add(END.len() + digits.len() + 1))
            .ok_or(Fault::Limit)?;
        if size > max_bytes {
            continue;
        }
        if omitted == 0 {
            verify_message_frame(view.scope.redactor(), &view.bytes)?;
            return Ok((0, None));
        }
        let mut bytes = Vec::with_capacity(size);
        bytes.extend_from_slice(PREFIX);
        bytes.extend_from_slice(&view.bytes[start..end]);
        bytes.extend_from_slice(END);
        bytes.extend_from_slice(digits.as_bytes());
        bytes.push(b'}');
        if bytes.len() != size || bytes.len() > max_bytes || bytes.len() > CAP {
            return Err(Fault::Limit);
        }
        // New framing/omission integers can collide even when all source data
        // views were checked. Refuse fixed, never try another omission or alias.
        verify_message_frame(view.scope.redactor(), &bytes)?;
        return Ok((omitted, Some(bytes)));
    }
    Err(Fault::Limit)
}

/// Span walk of immutable, already-admitted/encoded JSON only. No JSON decoding,
/// data copies, recursive parser, repair or arbitrary-source admission is added.
fn layout(bytes: &[u8], count: usize) -> Result<(Vec<usize>, usize), Fault> {
    if bytes.len() > CAP || count > MESSAGES {
        return Err(Fault::Limit);
    }
    if !bytes.starts_with(PREFIX) || !bytes.ends_with(ORIGINAL_END) {
        return Err(Fault::Collision);
    }
    let end = bytes.len() - ORIGINAL_END.len();
    if end < PREFIX.len() {
        return Err(Fault::Collision);
    }
    let mut starts = Vec::new();
    let mut i = PREFIX.len();
    while i < end {
        if starts.len() >= MESSAGES || bytes[i] != b'{' {
            return Err(Fault::Collision);
        }
        starts.push(i);
        let mut stack = [0u8; STACK];
        let mut depth = 0;
        let mut string = false;
        let mut escaped = false;
        loop {
            let b = *bytes.get(i).filter(|_| i < end).ok_or(Fault::Collision)?;
            i += 1;
            if string {
                if escaped {
                    escaped = false;
                } else if b == b'\\' {
                    escaped = true;
                } else if b == b'"' {
                    string = false;
                }
                continue;
            }
            match b {
                b'"' => string = true,
                b'{' | b'[' => {
                    if depth == STACK {
                        return Err(Fault::Limit);
                    }
                    stack[depth] = if b == b'{' { b'}' } else { b']' };
                    depth += 1;
                }
                b'}' | b']' => {
                    if depth == 0 || stack[depth - 1] != b {
                        return Err(Fault::Collision);
                    }
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
        }
        if i < end {
            if bytes[i] != b',' {
                return Err(Fault::Collision);
            }
            i += 1;
            if i == end {
                return Err(Fault::Collision);
            }
        }
    }
    if starts.len() != count {
        return Err(Fault::Collision);
    }
    Ok((starts, end))
}

/// Pure span projection of an ALREADY checked complete private message frame.
/// It cannot mint any bound view or admit raw input. Original layout is unchanged.
pub(in crate::exec) fn checked_message_layout(
    bytes: &[u8],
    count: usize,
) -> Result<(Vec<usize>, std::ops::Range<usize>), Fault> {
    let (starts, end) = layout(bytes, count)?;
    Ok((starts, PREFIX.len()..end))
}
