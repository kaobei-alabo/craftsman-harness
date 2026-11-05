//! Private deterministic literal-secret redaction, not a public output adapter.
//!
//! Seeds are explicit captured values, never an environment/config discovery
//! heuristic. Streams delay enough original bytes to cover cross-chunk AND
//! overlapping matches. Use BEFORE truncation/serialization/publication; no
//! storage/tool/provider writer or callable visibility is enabled here.

pub(crate) mod capture;
mod masked;
pub(crate) mod publication;
pub(crate) use masked::{Line as MaskedLine, MaskedText};
#[cfg(test)]
mod tests;

use serde_json::Value;
use std::collections::{BTreeSet, VecDeque};
use std::sync::Arc;
use thiserror::Error;

const MAX_SEEDS: usize = 64;
const MAX_SEED_BYTES: usize = 16 * 1024;
const MAX_TOTAL_SEED_BYTES: usize = 64 * 1024;
const MAX_INPUT_SEEDS: usize = 256;
const MAX_CHUNK: usize = 64 * 1024;
const MAX_RECORD: usize = 1024 * 1024;
const MAX_DEPTH: usize = 64;
const MAX_NODES: usize = 100_000;
const MARKER: &[u8] = b"[REDACTED]";

type Result<T> = std::result::Result<T, RedactionError>;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub(crate) enum RedactionError {
    #[error("invalid, empty or oversized captured credential seed")]
    Seed,
    #[error("captured credential seeds exceed their resource bound")]
    Seeds,
    #[error("captured credential conflicts with the fixed redaction marker")]
    Marker,
    #[error("redaction input chunk exceeds 64 KiB")]
    Chunk,
    #[error("redaction input or result exceeds its record bound")]
    Record,
    #[error("redaction stream is closed")]
    Closed,
    #[error("redaction text is not valid UTF-8")]
    Utf8,
    #[error("redaction would collide structured record keys")]
    KeyCollision,
    #[error("redaction stream position exceeds its bound")]
    Position,
}

struct Pattern {
    bytes: Vec<u8>,
    prefix: Vec<usize>,
}
struct Inner {
    patterns: Vec<Pattern>,
    longest: usize,
}
/// No Debug/Serialize implementation can expose seed values or KMP state.
#[derive(Clone)]
pub(crate) struct Redactor(Arc<Inner>);
impl Redactor {
    pub(crate) fn from_values(values: Vec<Vec<u8>>) -> Result<Self> {
        if values.len() > MAX_INPUT_SEEDS {
            return Err(RedactionError::Seeds);
        }
        let mut unique = BTreeSet::new();
        for value in values {
            if value.is_empty() || value.len() > MAX_SEED_BYTES {
                return Err(RedactionError::Seed);
            }
            // Fail closed rather than rendering a replacement containing the
            // credential itself (including short/single-byte credential values).
            if MARKER.windows(value.len()).any(|w| w == value) {
                return Err(RedactionError::Marker);
            }
            unique.insert(value);
        }
        if unique.len() > MAX_SEEDS
            || unique.iter().map(Vec::len).sum::<usize>() > MAX_TOTAL_SEED_BYTES
        {
            return Err(RedactionError::Seeds);
        }
        let longest = unique.iter().map(Vec::len).max().unwrap_or(1);
        let patterns = unique
            .into_iter()
            .map(|bytes| {
                let mut prefix = vec![0; bytes.len()];
                for index in 1..bytes.len() {
                    let mut matched = prefix[index - 1];
                    while matched > 0 && bytes[index] != bytes[matched] {
                        matched = prefix[matched - 1];
                    }
                    if bytes[index] == bytes[matched] {
                        matched += 1;
                    }
                    prefix[index] = matched;
                }
                Pattern { bytes, prefix }
            })
            .collect();
        Ok(Self(Arc::new(Inner { patterns, longest })))
    }
    pub(crate) fn byte_stream(&self) -> ByteStream {
        ByteStream {
            inner: self.0.clone(),
            states: vec![0; self.0.patterns.len()],
            pending: VecDeque::new(),
            ranges: VecDeque::new(),
            consumed: 0,
            emitted: 0,
            inside: false,
            matched: false,
            closed: false,
        }
    }
    /// Complete, bounded original-source coverage BEFORE pagination. The view
    /// keeps original line positions without exposing raw bytes or mask state.
    pub(crate) fn masked_text<'a>(&self, input: &'a str) -> Result<MaskedText<'a>> {
        MaskedText::new(self, input)
    }
    pub(crate) fn text_stream(&self) -> TextStream {
        TextStream {
            bytes: self.byte_stream(),
            pending: Vec::new(),
            closed: false,
        }
    }
    pub(crate) fn bytes(&self, input: &[u8]) -> Result<Vec<u8>> {
        if input.len() > MAX_RECORD {
            return Err(RedactionError::Record);
        }
        let mut stream = self.byte_stream();
        let mut output = Vec::new();
        for chunk in input.chunks(MAX_CHUNK) {
            append(&mut output, stream.push(chunk)?)?;
        }
        append(&mut output, stream.finish()?)?;
        Ok(output)
    }
    pub(crate) fn text(&self, input: &str) -> Result<String> {
        String::from_utf8(self.bytes(input.as_bytes())?).map_err(|_| RedactionError::Utf8)
    }
    /// Redact string values AND member names before encoding; detect key
    /// collisions, depth/work/output overflow without echoing raw data.
    pub(crate) fn json(&self, input: &Value) -> Result<Value> {
        let mut budget = JsonBudget {
            nodes: MAX_NODES,
            bytes: MAX_RECORD,
        };
        let result = self.json_inner(input, 0, &mut budget)?;
        let encoded = serde_json::to_vec(&result).map_err(|_| RedactionError::Record)?;
        if encoded.len() > MAX_RECORD {
            return Err(RedactionError::Record);
        }
        Ok(result)
    }
    fn json_inner(&self, input: &Value, depth: usize, budget: &mut JsonBudget) -> Result<Value> {
        if depth > MAX_DEPTH || budget.nodes == 0 {
            return Err(RedactionError::Record);
        }
        budget.nodes -= 1;
        budget.charge(1)?;
        Ok(match input {
            Value::String(s) => {
                budget.charge(s.len())?;
                let out = self.text(s)?;
                budget.charge(out.len())?;
                Value::String(out)
            }
            Value::Array(items) => Value::Array(
                items
                    .iter()
                    .map(|v| self.json_inner(v, depth + 1, budget))
                    .collect::<Result<_>>()?,
            ),
            Value::Object(map) => {
                let mut out = serde_json::Map::new();
                for (key, value) in map {
                    budget.charge(key.len())?;
                    let key = self.text(key)?;
                    budget.charge(key.len())?;
                    if out.contains_key(&key) {
                        return Err(RedactionError::KeyCollision);
                    }
                    out.insert(key, self.json_inner(value, depth + 1, budget)?);
                }
                Value::Object(out)
            }
            Value::Number(number) => {
                budget.charge(number.to_string().len())?;
                input.clone()
            }
            _ => input.clone(),
        })
    }
}
struct JsonBudget {
    nodes: usize,
    bytes: usize,
}
impl JsonBudget {
    fn charge(&mut self, count: usize) -> Result<()> {
        self.bytes = self
            .bytes
            .checked_sub(count)
            .ok_or(RedactionError::Record)?;
        Ok(())
    }
}
fn append(output: &mut Vec<u8>, bytes: Vec<u8>) -> Result<()> {
    if bytes.len() > MAX_RECORD.saturating_sub(output.len()) {
        return Err(RedactionError::Record);
    }
    output.extend(bytes);
    Ok(())
}
struct Span {
    start: u64,
    end: u64,
}
/// Original-byte KMP states and disjoint coverage intervals are bounded by the
/// explicit seed set/longest seed, not by stream length. No arbitrary plaintext
/// history is retained. Replacement bytes are never fed back into the matcher.
pub(crate) struct ByteStream {
    inner: Arc<Inner>,
    states: Vec<usize>,
    pending: VecDeque<u8>,
    ranges: VecDeque<Span>,
    consumed: u64,
    emitted: u64,
    inside: bool,
    matched: bool,
    closed: bool,
}
impl ByteStream {
    pub(crate) fn push(&mut self, input: &[u8]) -> Result<Vec<u8>> {
        if self.closed {
            return Err(RedactionError::Closed);
        }
        if input.len() > MAX_CHUNK {
            return Err(RedactionError::Chunk);
        }
        self.consumed
            .checked_add(input.len() as u64)
            .ok_or(RedactionError::Position)?;
        let mut output = Vec::new();
        for &byte in input {
            self.accept(byte);
            if self.pending.len() >= self.inner.longest {
                self.emit(&mut output);
            }
        }
        Ok(output)
    }
    /// Monotonic literal-match fact only, never a seed/state representation.
    /// A post-render verification stream can reject a match instead of silently
    /// rewriting trusted JSON syntax or a newly synthesized replacement boundary.
    pub(crate) fn matched(&self) -> bool {
        self.matched
    }
    /// Close without releasing the unsafe delayed suffix. Returns only whether
    /// original bytes were discarded; cancellation/truncation must not finish.
    pub(crate) fn abort(&mut self) -> bool {
        let discarded = !self.pending.is_empty();
        self.closed = true;
        self.pending.clear();
        self.ranges.clear();
        self.states.fill(0);
        discarded
    }
    pub(crate) fn finish(&mut self) -> Result<Vec<u8>> {
        if self.closed {
            return Err(RedactionError::Closed);
        }
        self.closed = true;
        let mut output = Vec::new();
        while !self.pending.is_empty() {
            self.emit(&mut output);
        }
        self.ranges.clear();
        self.states.fill(0);
        Ok(output)
    }
    /// Shared matcher, not a second independently evolving coverage algorithm.
    /// Callers check the bounded chunk/position first. These private methods
    /// never expose original bytes outside the redaction module and its child.
    fn accept(&mut self, byte: u8) {
        self.pending.push_back(byte);
        self.consumed += 1;
        let mut earliest = None;
        for (index, pattern) in self.inner.patterns.iter().enumerate() {
            let mut matched = self.states[index];
            while matched > 0 && byte != pattern.bytes[matched] {
                matched = pattern.prefix[matched - 1];
            }
            if byte == pattern.bytes[matched] {
                matched += 1;
            }
            if matched == pattern.bytes.len() {
                let start = self.consumed - pattern.bytes.len() as u64;
                earliest = Some(earliest.map_or(start, |old: u64| old.min(start)));
                matched = pattern.prefix[matched - 1];
            }
            self.states[index] = matched;
        }
        if let Some(start) = earliest {
            self.matched = true;
            self.cover(start, self.consumed);
        }
    }
    fn covered_byte(&mut self) -> (u8, bool) {
        while self
            .ranges
            .front()
            .is_some_and(|span| span.end <= self.emitted)
        {
            self.ranges.pop_front();
        }
        let covered = self
            .ranges
            .front()
            .is_some_and(|span| span.start <= self.emitted && self.emitted < span.end);
        let byte = self.pending.pop_front().expect("bounded pending byte");
        self.emitted += 1;
        (byte, covered)
    }
    fn cover(&mut self, mut start: u64, end: u64) {
        while self.ranges.back().is_some_and(|span| span.end >= start) {
            start = start.min(self.ranges.pop_back().unwrap().start);
        }
        self.ranges.push_back(Span { start, end });
    }
    fn emit(&mut self, output: &mut Vec<u8>) {
        let (byte, covered) = self.covered_byte();
        if covered {
            if !self.inside {
                output.extend_from_slice(MARKER);
            }
            self.inside = true;
        } else {
            output.push(byte);
            self.inside = false;
        }
    }
}
/// Preserve incomplete UTF-8 emission across boundaries; never lossy-replace a
/// byte stream. Binary-seed matches that corrupt text fail closed explicitly.
pub(crate) struct TextStream {
    bytes: ByteStream,
    pending: Vec<u8>,
    closed: bool,
}
impl TextStream {
    pub(crate) fn push(&mut self, input: &str) -> Result<String> {
        if self.closed {
            return Err(RedactionError::Closed);
        }
        let bytes = self.bytes.push(input.as_bytes())?;
        self.decode(bytes, false)
    }
    pub(crate) fn finish(&mut self) -> Result<String> {
        if self.closed {
            return Err(RedactionError::Closed);
        }
        self.closed = true;
        let bytes = self.bytes.finish()?;
        self.decode(bytes, true)
    }
    fn decode(&mut self, bytes: Vec<u8>, final_chunk: bool) -> Result<String> {
        self.pending.extend(bytes);
        let count = match std::str::from_utf8(&self.pending) {
            Ok(_) => self.pending.len(),
            Err(e) if e.error_len().is_none() && !final_chunk => e.valid_up_to(),
            Err(_) => {
                self.closed = true;
                self.pending.clear();
                return Err(RedactionError::Utf8);
            }
        };
        let text = String::from_utf8(self.pending.drain(..count).collect())
            .map_err(|_| RedactionError::Utf8)?;
        Ok(text)
    }
}
