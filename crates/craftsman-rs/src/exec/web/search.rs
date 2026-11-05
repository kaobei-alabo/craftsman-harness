//! Strict complete-original JSON parsing precedes typed source extraction. Seeded
//! structured redaction/encoding verification runs BEFORE selecting or cutting
//! result views. Original provider keys remain private parsing metadata, not
//! redacted authority aliases; no syntax/scalar corruption or error-body echo.

use super::capture::Fault;
use crate::exec::redaction::publication::{Output, View};
use crate::exec::redaction::{RedactionError, Redactor};
use serde_json::Value;
const RECORD: usize = 1024 * 1024;
const RESULTS: usize = 10;
const SNIPPET: usize = 300;
fn redaction(error: RedactionError) -> Fault {
    match error {
        RedactionError::Record | RedactionError::Chunk | RedactionError::Position => Fault::Limit,
        _ => Fault::Redaction,
    }
}
fn verify(redactor: &Redactor, text: &[u8]) -> Result<(), Fault> {
    if text.len() > RECORD {
        return Err(Fault::Limit);
    }
    let mut stream = redactor.byte_stream();
    for chunk in text.chunks(64 * 1024) {
        stream.push(chunk).map_err(redaction)?;
        if stream.matched() {
            return Err(Fault::Rendered);
        }
    }
    stream.abort();
    Ok(())
}
fn field(redactor: &Redactor, raw: &str) -> Result<String, Fault> {
    let safe = redactor.text(raw).map_err(redaction)?;
    verify(redactor, safe.as_bytes())?;
    Ok(safe)
}
fn snippet(redactor: &Redactor, raw: &str) -> Result<(String, bool), Fault> {
    // Decode/structured field redaction FIRST. Whitespace collapse can then
    // recreate a value, so redact/verify the complete normalized field BEFORE
    // its 300-scalar cut, not after slicing off part of a known credential.
    let safe = field(redactor, raw)?;
    let flat = safe.split_whitespace().collect::<Vec<_>>().join(" ");
    let safe = field(redactor, &flat)?;
    if safe.chars().count() <= SNIPPET {
        return Ok((safe, false));
    }
    let prefix = safe.chars().take(SNIPPET - 1).collect::<String>();
    Ok((format!("{prefix}…"), true))
}
pub(super) fn render(redactor: &Redactor, original: &Value) -> Result<View, Fault> {
    // Validate the ENTIRE untrusted record, including keys and unused results.
    // A numeric/boolean/null/syntax/replacement/escaping collision cannot be
    // fixed by silently rewriting types or selecting only an innocuous row.
    let safe = redactor.json(original).map_err(redaction)?;
    let encoded = serde_json::to_vec(&safe).map_err(|_| Fault::Syntax)?;
    verify(redactor, &encoded)?;
    let rows = original
        .get("web")
        .and_then(|v| v.get("results"))
        .and_then(Value::as_array)
        .ok_or(Fault::Syntax)?;
    let mut output = Output::new(redactor);
    let mut complete = true;
    let mut shortened = rows.len() > RESULTS;
    if rows.is_empty() {
        complete = output.frame("No results.")?;
    }
    for (index, row) in rows.iter().take(RESULTS).enumerate() {
        let object = row.as_object().ok_or(Fault::Syntax)?;
        let title = field(
            redactor,
            object
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("(untitled)"),
        )?;
        let url = field(
            redactor,
            object.get("url").and_then(Value::as_str).unwrap_or(""),
        )?;
        let description = object
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("");
        let (description, cut) = snippet(redactor, description)?;
        shortened |= cut;
        if !output.frame(&format!("{}. ", index + 1))?
            || !output.frame(&title)?
            || !output.frame("\n   ")?
            || !output.frame(&url)?
            || !output.frame("\n")?
        {
            complete = false;
            break;
        }
        if !description.is_empty()
            && (!output.frame("   ")? || !output.frame(&description)? || !output.frame("\n")?)
        {
            complete = false;
            break;
        }
    }
    let mut view = output.finish(complete && rows.len() <= RESULTS)?;
    view.truncated |= shortened;
    Ok(view)
}

#[cfg(test)]
mod tests;
