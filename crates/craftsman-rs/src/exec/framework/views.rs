//! Complete known framework records before the existing result budget. Original
//! typed todos stay separate from model views; fixed schema/status are not aliases.

use crate::exec::redaction::publication::{Output, View};
use crate::exec::redaction::{RedactionError, Redactor};
use craftsman_harness::state::Todo;
use serde_json::Value;
use std::io::{self, Write};
const RECORD: usize = 1024 * 1024;
const DEPTH: usize = 64;
const NODES: usize = 100_000;
const PREFIX: &str = "Updated todo list to ";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Fault {
    Limit,
    Unsafe,
}
fn redaction(error: RedactionError) -> Fault {
    match error {
        RedactionError::Record | RedactionError::Chunk | RedactionError::Position => Fault::Limit,
        _ => Fault::Unsafe,
    }
}
impl From<crate::exec::redaction::publication::Fault> for Fault {
    fn from(value: crate::exec::redaction::publication::Fault) -> Self {
        match value {
            crate::exec::redaction::publication::Fault::Limit => Self::Limit,
            _ => Self::Unsafe,
        }
    }
}

/// Private input admission only: a memory-only counter, not an output/stdout,
/// persistence or IO writer. No raw JSON buffer/diagnostic is retained. Count the
/// COMPLETE original todo input before cloning/parsing it; never parse a prefix.
pub(super) fn admit_todos(input: &Value) -> Result<(), Fault> {
    // Bound depth/work BEFORE serde's recursive counter or a parser clone.
    // Check pending children before growing this private, bounded work stack.
    let mut pending = vec![(input, 0)];
    let mut visited = 0;
    while let Some((value, depth)) = pending.pop() {
        visited += 1;
        if visited > NODES || depth > DEPTH {
            return Err(Fault::Limit);
        }
        let children = match value {
            Value::Array(rows) => rows.len(),
            Value::Object(fields) => fields.len(),
            _ => 0,
        };
        if children > NODES - visited - pending.len() {
            return Err(Fault::Limit);
        }
        match value {
            Value::Array(rows) => pending.extend(rows.iter().map(|v| (v, depth + 1))),
            Value::Object(fields) => pending.extend(fields.values().map(|v| (v, depth + 1))),
            Value::String(text) if text.len() > RECORD => return Err(Fault::Limit),
            _ => {}
        }
    }
    count_original(input)
}

/// Restored/original typed state can exist without a new write_todos call.
/// Bound it without cloning BEFORE the known middleware allocates its context.
pub(super) fn admit_todo_context(original: &[Todo]) -> Result<(), Fault> {
    if original.len() > (NODES - 1) / 3 || original.iter().any(|t| t.content.len() > RECORD) {
        return Err(Fault::Limit);
    }
    count_original(original)
}
fn count_original<T: serde::Serialize + ?Sized>(input: &T) -> Result<(), Fault> {
    struct Counter(usize);
    impl Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0 = self
                .0
                .checked_add(bytes.len())
                .filter(|n| *n <= RECORD)
                .ok_or_else(|| io::Error::other("private record limit"))?;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    serde_json::to_writer(Counter(0), input).map_err(|_| Fault::Limit)
}
fn verify(redactor: &Redactor, bytes: &[u8]) -> Result<(), Fault> {
    if bytes.len() > RECORD {
        return Err(Fault::Limit);
    }
    let mut check = redactor.byte_stream();
    for chunk in bytes.chunks(64 * 1024) {
        check.push(chunk).map_err(redaction)?;
        if check.matched() {
            return Err(Fault::Unsafe);
        }
    }
    check.abort();
    Ok(())
}

pub(super) fn text(redactor: &Redactor, original: &str) -> Result<View, Fault> {
    if original.len() > RECORD {
        return Err(Fault::Limit);
    }
    let mut output = Output::new(redactor);
    let complete = output.frame(original)?;
    output.finish(complete).map_err(Into::into)
}

/// Fields come from successfully parsed ORIGINAL Todo values, never redacted
/// arguments or a JSON prefix. Redact content before encoding, but a credential
/// collision with the fixed schema or an enum status must fail, not rename a
/// key/change a status. A todo JSON record is whole or fails; no repaired prefix.
pub(super) fn todos(redactor: &Redactor, original: &[Todo]) -> Result<View, Fault> {
    let value = serde_json::to_value(original).map_err(|_| Fault::Unsafe)?;
    let safe = redactor.json(&value).map_err(redaction)?;
    let rows = safe.as_array().ok_or(Fault::Unsafe)?;
    let source = value.as_array().ok_or(Fault::Unsafe)?;
    if rows.len() != source.len() {
        return Err(Fault::Unsafe);
    }
    for (row, source) in rows.iter().zip(source) {
        let fields = row.as_object().ok_or(Fault::Unsafe)?;
        if fields.len() != 2
            || !fields.get("content").is_some_and(Value::is_string)
            || fields.get("status") != source.get("status")
        {
            return Err(Fault::Unsafe);
        }
    }
    let encoded = serde_json::to_string(&safe).map_err(|_| Fault::Unsafe)?;
    verify(redactor, encoded.as_bytes())?;
    let prefix = redactor.text(PREFIX).map_err(redaction)?;
    verify(redactor, prefix.as_bytes())?;
    let mut output = Output::new(redactor);
    if !output.frame(&prefix)? || !output.frame(&encoded)? {
        return Err(Fault::Limit);
    }
    let view = output.finish(true)?;
    if view.truncated {
        return Err(Fault::Limit);
    }
    // The framing/JSON join cannot silently redact JSON syntax or reintroduce
    // a replacement-boundary literal. Verify the exact already-safe record.
    if view.text != format!("{prefix}{encoded}") {
        return Err(Fault::Unsafe);
    }
    Ok(view)
}

#[cfg(test)]
mod tests {
    use super::*;
    use craftsman_harness::state::TodoStatus;
    use serde_json::json;
    fn redactor(seeds: &[&[u8]]) -> Redactor {
        Redactor::from_values(seeds.iter().map(|s| s.to_vec()).collect()).unwrap()
    }
    #[test]
    fn complete_original_admission_bounds_encoded_bytes_depth_and_work_before_copy() {
        assert!(admit_todos(&Value::String("q".repeat(RECORD - 2))).is_ok());
        assert!(matches!(
            admit_todos(&Value::String("q".repeat(RECORD - 1))),
            Err(Fault::Limit)
        ));
        assert!(matches!(
            admit_todos(&Value::String("\n".repeat(RECORD / 2 + 1))),
            Err(Fault::Limit)
        ));
        let mut nested = Value::Null;
        for _ in 0..DEPTH + 1 {
            nested = json!([nested]);
        }
        assert!(matches!(admit_todos(&nested), Err(Fault::Limit)));
        assert!(matches!(
            admit_todos(&Value::Array(vec![Value::Null; NODES])),
            Err(Fault::Limit)
        ));
    }
    #[test]
    fn complete_plaintext_redaction_precedes_caps_and_never_finishes_a_cut_suffix() {
        let r = redactor(&[b"credential-value"]);
        let original = format!("{}credential-value tail", "q".repeat(98_971));
        let view = text(&r, &original).unwrap();
        assert!(view.truncated && view.text.len() <= 100_000);
        assert!(!view.text.contains("cred") && !view.text.contains("tail"));
    }
    #[test]
    fn todo_schema_status_syntax_and_rendered_replacement_are_not_silent_aliases() {
        let input = vec![Todo {
            content: "xsecret".into(),
            status: TodoStatus::Pending,
        }];
        for seeds in [
            vec![b"status".as_slice()],
            vec![b"pending".as_slice()],
            vec![b":".as_slice()],
            vec![b"secret".as_slice(), b"x[REDACTED]".as_slice()],
        ] {
            assert!(matches!(
                todos(&redactor(&seeds), &input),
                Err(Fault::Unsafe)
            ));
        }
    }
    #[test]
    fn todo_source_is_a_whole_typed_array_not_an_arbitrary_plaintext_json_repair() {
        let r = redactor(&[b"secret"]);
        let input = vec![Todo {
            content: "secret \" quoted".into(),
            status: TodoStatus::Completed,
        }];
        let view = todos(&r, &input).unwrap();
        let decoded: Value = serde_json::from_str(view.text.strip_prefix(PREFIX).unwrap()).unwrap();
        assert_eq!(
            decoded,
            json!([{"content":"[REDACTED] \" quoted","status":"completed"}])
        );
        assert_eq!(input[0].content, "secret \" quoted");
        assert!(!view.truncated);
    }
}
