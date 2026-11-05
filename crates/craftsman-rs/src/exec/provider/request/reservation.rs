//! Bounded, source-bound token preflight. Never a usage/receipt estimator.
//! The unchanged request codec admits the original request before this module
//! parses its checked bytes. Only the actual meter chooses a reply allocation.
use super::{BYTES, Checked, Fault, Source, encode};
use crate::exec::runtime::InvocationGate;
use crate::token_counter::TiktokenCounter;
use serde_json::{Value, json};
use std::sync::Arc;

/// Private reply policy, not a provider's maximum or a public CLI default.
const DEFAULT_REPLY: u64 = 4096;
const PIECE: usize = 4096;

pub(in crate::exec) struct Plan {
    source: Arc<Source>,
    identity: Arc<()>,
    body: Value,
    field: &'static str,
    ceiling: u64,
    input: u64,
}
impl Plan {
    pub(in crate::exec) fn prepare(
        checked: Checked,
        counter: &TiktokenCounter,
    ) -> Result<Self, Fault> {
        checked.source.ready(&checked)?;
        // Already finite/UTF-8/encoded by the SAME original request codec.
        if checked.bytes.len() > BYTES {
            return Err(Fault::Limit);
        }
        let mut body: Value =
            serde_json::from_slice(&checked.bytes).map_err(|_| Fault::Collision)?;
        let object = body.as_object_mut().ok_or(Fault::Collision)?;
        // A narrow chat-completions subset, not arbitrary provider kwargs whose
        // hidden candidate/output controls could escape a single reply grant.
        for key in object.keys() {
            if !matches!(
                key.as_str(),
                "model"
                    | "messages"
                    | "tools"
                    | "stream"
                    | "stream_options"
                    | "max_tokens"
                    | "max_completion_tokens"
                    | "n"
                    | "temperature"
                    | "top_p"
                    | "stop"
                    | "seed"
                    | "presence_penalty"
                    | "frequency_penalty"
                    | "logit_bias"
                    | "logprobs"
                    | "top_logprobs"
                    | "tool_choice"
                    | "parallel_tool_calls"
                    | "response_format"
                    | "metadata"
            ) {
                return Err(Fault::Collision);
            }
        }
        if object.get("n").is_some_and(|v| v.as_u64() != Some(1)) {
            return Err(Fault::Collision);
        }
        let field = match (
            object.contains_key("max_tokens"),
            object.contains_key("max_completion_tokens"),
        ) {
            (true, false) | (false, false) => "max_tokens",
            (false, true) => "max_completion_tokens",
            (true, true) => return Err(Fault::Collision),
        };
        let ceiling = match object.remove(field) {
            Some(v) => v.as_u64().filter(|n| *n > 0).ok_or(Fault::Collision)?,
            None => DEFAULT_REPLY,
        };
        // Count the WHOLE checked input envelope, including system/messages,
        // calls/arguments/tools/schema/defaults. The reply-control integer is
        // deliberately excluded: a huge configured output ceiling must not
        // manufacture an input estimate or an insufficient-minimum refusal.
        let input_bytes = encode(&checked.source.redactor, &body)?;
        let input = count_input(counter, &input_bytes)?;
        body.as_object_mut()
            .ok_or(Fault::Collision)?
            .insert(field.into(), json!(ceiling));
        checked.source.ready(&checked)?;
        Ok(Self {
            source: checked.source,
            identity: checked.identity,
            body,
            field,
            ceiling,
            input,
        })
    }
    pub(in crate::exec) fn matches_gate(&self, gate: &Arc<InvocationGate>) -> bool {
        Arc::ptr_eq(&self.source.gate, gate)
    }
    pub(in crate::exec) fn input(&self) -> u64 {
        self.input
    }
    pub(in crate::exec) fn ceiling(&self) -> u64 {
        self.ceiling
    }
    /// Called INSIDE the actual meter's bounded admission decision, BEFORE
    /// either count/allocation changes. New digits/joins must pass the SAME
    /// whole encoded-literal check; failure never charges then refunds a call.
    pub(in crate::exec) fn finish(mut self, output: u64) -> Result<Checked, Fault> {
        if output == 0 || output > self.ceiling {
            return Err(Fault::Collision);
        }
        self.body
            .as_object_mut()
            .ok_or(Fault::Collision)?
            .insert(self.field.into(), json!(output));
        let bytes = encode(&self.source.redactor, &self.body)?;
        let checked = Checked {
            source: self.source,
            identity: self.identity,
            bytes,
        };
        checked.source.ready(&checked)?;
        Ok(checked)
    }
}

fn count_input(counter: &TiktokenCounter, bytes: &[u8]) -> Result<u64, Fault> {
    if bytes.len() > BYTES {
        return Err(Fault::Limit);
    }
    let mut text = std::str::from_utf8(bytes).map_err(|_| Fault::Collision)?;
    let mut total = 0u64;
    while !text.is_empty() {
        let mut end = text.len().min(PIECE);
        while !text.is_char_boundary(end) {
            end = end.checked_sub(1).ok_or(Fault::Collision)?;
        }
        if end == 0 {
            return Err(Fault::Collision);
        }
        let count = counter.count_admitted_piece(&text[..end]);
        total = total.checked_add(count).ok_or(Fault::Limit)?;
        text = &text[end..];
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_projection_is_zero_not_a_minimum_reply() {
        assert_eq!(count_input(&TiktokenCounter::new(), b"").unwrap(), 0);
    }
    #[test]
    fn exact_small_piece_uses_the_existing_encoding() {
        let counter = TiktokenCounter::new();
        let text = "original Unicode input \u{1f600} / tool arguments";
        assert_eq!(
            count_input(&counter, text.as_bytes()).unwrap(),
            counter.count_admitted_piece(text)
        );
    }
    #[test]
    fn scalar_aligned_pieces_count_all_original_text() {
        let counter = TiktokenCounter::new();
        let prefix = "a".repeat(4095);
        let tail = format!("\u{1f600}{}", "b".repeat(100));
        let text = format!("{prefix}{tail}");
        assert_eq!(
            count_input(&counter, text.as_bytes()).unwrap(),
            counter.count_admitted_piece(&prefix) + counter.count_admitted_piece(&tail)
        );
    }
    #[test]
    fn exact_whole_bound_is_admitted_without_a_larger_piece() {
        let text = "x".repeat(BYTES);
        let counter = TiktokenCounter::new();
        assert_eq!(
            count_input(&counter, text.as_bytes()).unwrap(),
            counter.count_admitted_piece(&text[..PIECE]) * (BYTES / PIECE) as u64
        );
    }
    #[test]
    fn overbound_or_invalid_utf8_refuses_before_tokenization() {
        let counter = TiktokenCounter::new();
        assert_eq!(
            count_input(&counter, &vec![b'x'; BYTES + 1]),
            Err(Fault::Limit)
        );
        assert_eq!(count_input(&counter, &[0xff]), Err(Fault::Collision));
    }
}
