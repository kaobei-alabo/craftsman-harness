//! Filesystem whole-name records; text publication shares the private codec.
//! Original IO authority and the filesystem scope are unchanged.

use crate::exec::redaction::Redactor;
use crate::exec::redaction::publication::verify_text;
pub(super) use crate::exec::redaction::publication::{BUDGET, Fault, Output, View};

/// Name fields are whole, independent original records, NEVER truncated into
/// aliases. The caller keeps original names for IO/ordering and detects view
/// collisions before returning an array. Joins use Output's framing pipeline.
pub(super) fn name(redactor: &Redactor, original: &str) -> Result<String, Fault> {
    let safe = redactor.text(original).map_err(|_| Fault::Redaction)?;
    verify_text(redactor, &safe)?;
    Ok(safe)
}

#[cfg(test)]
mod tests;
