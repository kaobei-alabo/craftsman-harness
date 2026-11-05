//! Whole-message selection after BOTH terminal channels are fully admitted.
//! Preserve original checked output/facts; never use a live-latch exemption.
use super::{Error, Facts, Scope};
use crate::exec::messages::checked_message_layout;
use crate::exec::provider::request::{Fault, terminal_channels, verify_message_frame};
const CAP: usize = 1024 * 1024;
const END: &[u8] = b"],\"messages_omitted\":";

/// Only the actual Terminal's opaque scope mints these immutable checked bytes.
/// No Debug/Serialize/raw constructor, execution aliases or IO interface.
pub(crate) struct Selection<'a> {
    scope: Scope<'a>,
    facts: Facts,
    count: usize,
    omitted: usize,
    bytes: Vec<u8>,
}
impl Selection<'_> {
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
pub(super) fn select(
    scope: Scope<'_>,
    max_bytes: usize,
    after: impl FnOnce(),
) -> Result<Selection<'_>, Error> {
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
        // Each complete ORIGINAL source is admitted before its copies. Both
        // encoded channels are checked before ANY omission, under the real lock.
        let (prefix, messages) = terminal_channels(
            scope.runtime.redactor(),
            &state.messages,
            scope.runtime.output(),
            facts.returned.tag(),
            facts.approval_required,
        )?;
        let total = state.messages.len();
        let (starts, body) = checked_message_layout(&messages, total)?;
        if prefix.last() != Some(&b'}') || body.start == 0 {
            return Err(Error::Codec(Fault::Collision));
        }
        for omitted in 0..=total {
            let start = starts.get(omitted).copied().unwrap_or(body.end);
            let digits = omitted.to_string();
            let size = prefix
                .len()
                .checked_add(body.start - 1)
                .and_then(|n| n.checked_add(body.end - start))
                .and_then(|n| n.checked_add(END.len() + digits.len() + 1))
                .ok_or(Error::Codec(Fault::Limit))?;
            if size > max_bytes {
                continue;
            }
            let mut bytes = Vec::with_capacity(size);
            bytes.extend_from_slice(&prefix[..prefix.len() - 1]);
            bytes.push(b',');
            bytes.extend_from_slice(&messages[1..body.start]);
            bytes.extend_from_slice(&messages[start..body.end]);
            bytes.extend_from_slice(END);
            bytes.extend_from_slice(digits.as_bytes());
            bytes.push(b'}');
            if bytes.len() != size || bytes.len() > max_bytes || bytes.len() > CAP {
                return Err(Error::Codec(Fault::Limit));
            }
            // Changed first-record joins and omission integers can collide even
            // after both channels passed. Never try another layout to hide facts.
            verify_message_frame(scope.runtime.redactor(), &bytes)?;
            return Ok((total - omitted, omitted, bytes));
        }
        Err(Error::Codec(Fault::Limit))
    })();
    after();
    // Actual lease/epoch changes dominate every codec/budget error too.
    scope.ready(facts.approval_required)?;
    let (count, omitted, bytes) = result?;
    Ok(Selection {
        scope,
        facts,
        count,
        omitted,
        bytes,
    })
}
