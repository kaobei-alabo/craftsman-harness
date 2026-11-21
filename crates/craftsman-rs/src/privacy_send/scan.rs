//! Bounded verification of one final encoded request body.
//!
//! Three passes share one work budget:
//!
//! 1. the exact bytes as sent;
//! 2. the request's own text nodes, when the caller supplies them;
//! 3. the decoded JSON structure, including JSON documents carried inside
//!    string values (tool-call arguments on the Chat and Responses wires).
//!
//! Admitted originals contain no byte that a JSON string encoder must escape.
//! A minimal encoder therefore leaves them literal at every nesting depth,
//! which pass 1 sees. Pass 3 covers encoders that escape more than they must,
//! down to a fixed number of embedded documents, and refuses beyond it.
//!
//! # What the work budget counts
//!
//! Every unit below is charged before the work it stands for, and a refused
//! charge ends the verification:
//!
//! - one unit per byte of each text given to the literal pass (the body, each
//!   supplied text node, and each decoded string or key), for the two-byte
//!   filter probe at that position;
//! - four units for each four-byte prefix comparison of the candidate search:
//!   every step of the binary search, and every equality test that accepts a
//!   candidate or ends the run of candidates;
//! - the full length of a candidate original for its comparison with the text;
//! - one unit per byte of the body for UTF-8 validation;
//! - one unit per byte of every JSON parse input, the body and each embedded
//!   candidate document, for tokenizing and unescaping;
//! - one unit per byte examined while skipping the leading whitespace of a
//!   decoded string;
//! - one unit per supplied text node for its bookkeeping.
//!
//! It is a count of those operations. It is not a measure of CPU time, and it
//! does not count constant bookkeeping per charged step, allocator work, the
//! one copy of the supplied text nodes into a seal, or liveness probes. Those
//! are bounded by the body ceiling, not by this budget.
//!
//! Admission is separate and is never charged here. It is bounded by the
//! originals ceilings alone: at most that many records are read, each value is
//! checked in place and copied only if it has a supported size and shape, and
//! at most twice that many values are sorted.

use super::{Limits, MIN_ORIGINAL_BYTES, SendRefusal};
use serde::de::{self, DeserializeSeed, Deserializer, MapAccess, SeqAccess, Visitor};
use std::cell::Cell;
use std::fmt;
use zeroize::Zeroizing;

/// Sorted distinct originals and a one-bit-per-two-byte-prefix filter.
pub(super) struct Originals {
    items: Vec<Zeroizing<Vec<u8>>>,
    prefixes: Zeroizing<Vec<u64>>,
}

impl Originals {
    pub(super) fn empty() -> Self {
        Self {
            items: Vec::new(),
            prefixes: Zeroizing::new(vec![0; 1024]),
        }
    }

    /// The union with `added`, or `None` when nothing new was supplied. The
    /// caller has already bounded `added` by the originals ceilings.
    pub(super) fn with(
        &self,
        added: Vec<Zeroizing<Vec<u8>>>,
        limits: &Limits,
    ) -> Result<Option<Self>, SendRefusal> {
        let mut items: Vec<Zeroizing<Vec<u8>>> = self
            .items
            .iter()
            .map(|item| Zeroizing::new(item.to_vec()))
            .collect();
        items.extend(added);
        items.sort_unstable_by(|left, right| left.as_slice().cmp(right.as_slice()));
        items.dedup_by(|left, right| left.as_slice() == right.as_slice());
        if items.len() == self.items.len() {
            return Ok(None);
        }
        let bytes: usize = items.iter().map(|item| item.len()).sum();
        if items.len() > limits.originals || bytes > limits.total_original_bytes {
            return Err(SendRefusal::Bounds);
        }
        let mut prefixes = Zeroizing::new(vec![0u64; 1024]);
        for item in &items {
            let key = usize::from(item[0]) << 8 | usize::from(item[1]);
            prefixes[key >> 6] |= 1 << (key & 63);
        }
        Ok(Some(Self { items, prefixes }))
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.items.len()
    }
}

/// The conservative shape this guard can prove coverage for. Anything else is
/// refused at admission instead of being checked incompletely. The size is
/// checked first, so the byte scan is bounded by the per-original ceiling.
pub(super) fn supported_original(original: &[u8], limits: &Limits) -> Result<(), SendRefusal> {
    if original.len() < MIN_ORIGINAL_BYTES || original.len() > limits.original_bytes {
        return Err(SendRefusal::Bounds);
    }
    if original
        .iter()
        .any(|byte| matches!(byte, b'"' | b'\\' | 0x00..=0x1f | 0x7f))
    {
        return Err(SendRefusal::Unsupported);
    }
    Ok(())
}

pub(super) fn verify(
    originals: &Originals,
    limits: &Limits,
    encoded: &[u8],
    portable: &[&str],
) -> Result<(), SendRefusal> {
    run(originals, limits, encoded, portable).0
}

/// Test-only: the same verification together with the work units it charged.
#[cfg(test)]
pub(super) fn verify_counting(
    originals: &Originals,
    limits: &Limits,
    encoded: &[u8],
    portable: &[&str],
) -> (Result<(), SendRefusal>, u64) {
    run(originals, limits, encoded, portable)
}

fn run(
    originals: &Originals,
    limits: &Limits,
    encoded: &[u8],
    portable: &[&str],
) -> (Result<(), SendRefusal>, u64) {
    let scan = Scan {
        originals,
        limits,
        work: Cell::new(0),
        containers: Cell::new(0),
        refusal: Cell::new(None),
    };
    let result = scan.verify(encoded, portable);
    (result, scan.work.get())
}

struct Scan<'a> {
    originals: &'a Originals,
    limits: &'a Limits,
    work: Cell<u64>,
    containers: Cell<usize>,
    /// The first refusal. serde reports only a fixed marker; this keeps the kind.
    refusal: Cell<Option<SendRefusal>>,
}

struct Open<'s, 'a>(&'s Scan<'a>);

impl Drop for Open<'_, '_> {
    fn drop(&mut self) {
        self.0.containers.set(self.0.containers.get() - 1);
    }
}

impl<'a> Scan<'a> {
    fn verify(&self, encoded: &[u8], portable: &[&str]) -> Result<(), SendRefusal> {
        if encoded.len() > self.limits.body_bytes {
            return Err(SendRefusal::Bounds);
        }
        // The supplied text nodes, one byte each for the separator a seal
        // keeps between them, share the body's ceiling. Each of them is part
        // of the body it was encoded into, so this refuses no real request.
        let mut text_bytes = 0usize;
        for text in portable {
            self.charge(1)?;
            text_bytes = text_bytes.saturating_add(text.len()).saturating_add(1);
            if text_bytes > self.limits.body_bytes {
                return Err(SendRefusal::Bounds);
            }
        }
        self.literal(encoded)?;
        for text in portable {
            self.literal(text.as_bytes())?;
        }
        self.charge(encoded.len())?;
        let body = std::str::from_utf8(encoded).map_err(|_| SendRefusal::Unsupported)?;
        if self.document(body, 0) {
            return Ok(());
        }
        // Not one complete JSON value: no supported transport sends such a body.
        Err(self.refusal.get().unwrap_or(SendRefusal::Unsupported))
    }

    fn refuse<T>(&self, refusal: SendRefusal) -> Result<T, SendRefusal> {
        if self.refusal.get().is_none() {
            self.refusal.set(Some(refusal));
        }
        Err(self.refusal.get().unwrap_or(refusal))
    }

    /// Counts `units` and refuses once the budget is exceeded. Always called
    /// before the work the units stand for.
    fn charge(&self, units: usize) -> Result<(), SendRefusal> {
        let work = self.work.get().saturating_add(units as u64);
        self.work.set(work);
        if work > self.limits.work {
            return self.refuse(SendRefusal::Bounds);
        }
        Ok(())
    }

    fn enter(&self) -> Result<Open<'_, 'a>, SendRefusal> {
        if self.containers.get() >= self.limits.containers {
            return self.refuse(SendRefusal::Bounds);
        }
        self.containers.set(self.containers.get() + 1);
        Ok(Open(self))
    }

    /// Any admitted original as a contiguous byte run of `text`.
    fn literal(&self, text: &[u8]) -> Result<(), SendRefusal> {
        self.charge(text.len())?;
        let items = &self.originals.items;
        if items.is_empty() || text.len() < MIN_ORIGINAL_BYTES {
            return Ok(());
        }
        for start in 0..=text.len() - MIN_ORIGINAL_BYTES {
            let key = usize::from(text[start]) << 8 | usize::from(text[start + 1]);
            if self.originals.prefixes[key >> 6] & (1 << (key & 63)) == 0 {
                continue;
            }
            let prefix = &text[start..start + MIN_ORIGINAL_BYTES];
            let mut index = self.first_candidate(prefix)?;
            while let Some(item) = items.get(index) {
                // The prefix equality that accepts this candidate or ends the run.
                self.charge(MIN_ORIGINAL_BYTES)?;
                if &item[..MIN_ORIGINAL_BYTES] != prefix {
                    break;
                }
                self.charge(item.len())?;
                if text[start..].starts_with(item) {
                    return self.refuse(SendRefusal::KnownOriginal);
                }
                index += 1;
            }
        }
        Ok(())
    }

    /// The first original whose four-byte prefix is not below `prefix`. A
    /// plain binary search, so that each comparison can be charged before it
    /// is made and a refused charge stops the search.
    fn first_candidate(&self, prefix: &[u8]) -> Result<usize, SendRefusal> {
        let items = &self.originals.items;
        let (mut low, mut high) = (0, items.len());
        while low < high {
            let middle = low + (high - low) / 2;
            self.charge(MIN_ORIGINAL_BYTES)?;
            if &items[middle][..MIN_ORIGINAL_BYTES] < prefix {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        Ok(low)
    }

    /// The first byte after leading JSON whitespace, if there is one. Each
    /// byte is charged before it is examined.
    fn first_significant(&self, text: &str) -> Result<Option<u8>, SendRefusal> {
        for byte in text.bytes() {
            self.charge(1)?;
            if !matches!(byte, b' ' | b'\t' | b'\n' | b'\r') {
                return Ok(Some(byte));
            }
        }
        Ok(None)
    }

    /// True when `text` is exactly one JSON value and nothing in it refused.
    fn document(&self, text: &str, level: usize) -> bool {
        if self.charge(text.len()).is_err() {
            return false;
        }
        let mut input = serde_json::Deserializer::from_str(text);
        let walked = Walk { scan: self, level }.deserialize(&mut input).is_ok();
        walked && input.end().is_ok() && self.refusal.get().is_none()
    }

    /// A decoded string value found while walking the document at `level`.
    fn decoded(&self, text: &str, level: usize) -> Result<(), SendRefusal> {
        self.literal(text.as_bytes())?;
        if level > self.limits.embedded
            || !matches!(self.first_significant(text)?, Some(b'{' | b'[' | b'"'))
        {
            return Ok(());
        }
        let embedded = self.document(text, level + 1);
        if let Some(refusal) = self.refusal.get() {
            return Err(refusal);
        }
        if embedded && level == self.limits.embedded {
            // One more embedded document than this guard decodes.
            return self.refuse(SendRefusal::Bounds);
        }
        Ok(())
    }
}

struct Walk<'s, 'a> {
    scan: &'s Scan<'a>,
    level: usize,
}

fn refused<E: de::Error>(_: SendRefusal) -> E {
    E::custom("refused")
}

impl<'de> DeserializeSeed<'de> for Walk<'_, '_> {
    type Value = ();

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Walk<'_, '_> {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value")
    }

    fn visit_bool<E: de::Error>(self, _: bool) -> Result<(), E> {
        Ok(())
    }

    fn visit_i64<E: de::Error>(self, _: i64) -> Result<(), E> {
        Ok(())
    }

    fn visit_u64<E: de::Error>(self, _: u64) -> Result<(), E> {
        Ok(())
    }

    fn visit_f64<E: de::Error>(self, _: f64) -> Result<(), E> {
        Ok(())
    }

    fn visit_unit<E: de::Error>(self) -> Result<(), E> {
        Ok(())
    }

    /// A string borrowed from the input is a verbatim, escape-free slice of
    /// text that was already checked literally. It cannot hold a quoted string
    /// either, so there is nothing further to decode inside it.
    fn visit_borrowed_str<E: de::Error>(self, _: &'de str) -> Result<(), E> {
        Ok(())
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<(), E> {
        self.scan.decoded(value, self.level).map_err(refused)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<(), A::Error> {
        let _open = self.scan.enter().map_err(refused)?;
        while sequence
            .next_element_seed(Walk {
                scan: self.scan,
                level: self.level,
            })?
            .is_some()
        {}
        Ok(())
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        let _open = self.scan.enter().map_err(refused)?;
        while map
            .next_key_seed(Key {
                scan: self.scan,
                level: self.level,
            })?
            .is_some()
        {
            map.next_value_seed(Walk {
                scan: self.scan,
                level: self.level,
            })?;
        }
        Ok(())
    }
}

struct Key<'s, 'a> {
    scan: &'s Scan<'a>,
    level: usize,
}

impl<'de> DeserializeSeed<'de> for Key<'_, '_> {
    type Value = ();

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        deserializer.deserialize_str(self)
    }
}

impl<'de> Visitor<'de> for Key<'_, '_> {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON object key")
    }

    fn visit_borrowed_str<E: de::Error>(self, _: &'de str) -> Result<(), E> {
        Ok(())
    }

    fn visit_str<E: de::Error>(self, key: &str) -> Result<(), E> {
        self.scan.decoded(key, self.level).map_err(refused)
    }
}
