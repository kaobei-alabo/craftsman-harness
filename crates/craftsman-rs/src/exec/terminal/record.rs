//! Only the actual owned terminal can mint this finite private file record.
use super::{Error, Facts, Scope, Terminal};
use crate::exec::output::{CAP, Format};
use crate::exec::provider::request::{Fault, verify_message_frame};

pub(in crate::exec) struct Record<'a> {
    scope: Scope<'a>,
    facts: Facts,
    bytes: Vec<u8>,
}
impl Record<'_> {
    pub(in crate::exec) fn ready(&self) -> Result<(), Error> {
        self.scope.ready(self.facts.approval_required)
    }
    pub(in crate::exec) fn bytes(&self) -> Result<&[u8], Error> {
        self.ready()?;
        Ok(&self.bytes)
    }
}
pub(super) fn capture(
    terminal: &Terminal,
    format: Format,
    maximum: usize,
) -> Result<Record<'_>, Error> {
    let scope = terminal.scope()?;
    let approval = scope.runtime.approval_required();
    scope.ready(approval)?;
    let result = (|| {
        if maximum == 0 || maximum > CAP {
            return Err(Error::Codec(Fault::Limit));
        }
        let (facts, bytes) = match format {
            Format::AsciiFragment => {
                // The new physical LF is inside the COMPLETE file budget.
                let view = terminal.ascii(maximum - 1)?;
                view.bind(&scope)?;
                let facts = view.facts()?;
                let mut bytes = view.bytes()?.to_vec();
                bytes.push(b'\n');
                (facts, bytes)
            }
            Format::HumanPreview => {
                let view = terminal.human(maximum)?;
                view.bind(&scope)?;
                let facts = view.facts()?;
                let answer = view.answer()?.unwrap_or_default();
                let diagnostic = view.diagnostic()?;
                let size = answer
                    .len()
                    .checked_add(diagnostic.len())
                    .ok_or(Fault::Limit)?;
                if size > maximum {
                    return Err(Error::Codec(Fault::Limit));
                }
                let mut bytes = Vec::with_capacity(size);
                bytes.extend_from_slice(answer);
                bytes.extend_from_slice(diagnostic);
                (facts, bytes)
            }
        };
        if bytes.len() > maximum || bytes.last() != Some(&b'\n') {
            return Err(Error::Codec(Fault::Limit));
        }
        // SAME complete finite literal check after file framing/joins, before
        // ANY staging effect. No repaired prefix or alternate format fallback.
        verify_message_frame(scope.runtime.redactor(), &bytes)?;
        Ok::<_, Error>((facts, bytes))
    })();
    scope.ready(approval)?;
    let (facts, bytes) = result?;
    scope.ready(facts.approval_required)?;
    Ok(Record {
        scope,
        facts,
        bytes,
    })
}
