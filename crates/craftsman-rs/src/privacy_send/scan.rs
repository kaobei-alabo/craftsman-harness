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

    /// The union with `added`, or `None` when nothing new was supplied.
    pub(super) fn with(
        &self,
        added: Vec<Zeroizing<Vec<u8>>>,
        limits: &Limits,
    ) -> Result<Option<Self>, SendRefusal> {
        let mut fresh: Vec<Zeroizing<Vec<u8>>> = Vec::new();
        for original in added {
            if self.position(&original).is_none()
                && !fresh
                    .iter()
                    .any(|item| item.as_slice() == original.as_slice())
            {
                fresh.push(original);
            }
        }
        if fresh.is_empty() {
            return Ok(None);
        }
        let mut items: Vec<Zeroizing<Vec<u8>>> = self
            .items
            .iter()
            .map(|item| Zeroizing::new(item.to_vec()))
            .collect();
        items.append(&mut fresh);
        let bytes: usize = items.iter().map(|item| item.len()).sum();
        if items.len() > limits.originals || bytes > limits.total_original_bytes {
            return Err(SendRefusal::Bounds);
        }
        items.sort_unstable_by(|left, right| left.as_slice().cmp(right.as_slice()));
        let mut prefixes = Zeroizing::new(vec![0u64; 1024]);
        for item in &items {
            let key = usize::from(item[0]) << 8 | usize::from(item[1]);
            prefixes[key >> 6] |= 1 << (key & 63);
        }
        Ok(Some(Self { items, prefixes }))
    }

    fn position(&self, original: &[u8]) -> Option<usize> {
        self.items
            .binary_search_by(|item| item.as_slice().cmp(original))
            .ok()
    }
}

/// The conservative shape this guard can prove coverage for. Anything else is
/// refused at admission instead of being checked incompletely.
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
    if encoded.len() > limits.body_bytes {
        return Err(SendRefusal::Bounds);
    }
    let scan = Scan {
        originals,
        limits,
        work: Cell::new(0),
        containers: Cell::new(0),
        refusal: Cell::new(None),
    };
    scan.literal(encoded)?;
    for text in portable {
        scan.literal(text.as_bytes())?;
    }
    let body = std::str::from_utf8(encoded).map_err(|_| SendRefusal::Unsupported)?;
    if scan.document(body, 0) {
        return Ok(());
    }
    // Not one complete JSON value: no supported transport sends such a body.
    Err(scan.refusal.get().unwrap_or(SendRefusal::Unsupported))
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
    fn refuse<T>(&self, refusal: SendRefusal) -> Result<T, SendRefusal> {
        if self.refusal.get().is_none() {
            self.refusal.set(Some(refusal));
        }
        Err(self.refusal.get().unwrap_or(refusal))
    }

    fn charge(&self, bytes: usize) -> Result<(), SendRefusal> {
        let work = self.work.get().saturating_add(bytes as u64);
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
            let first = items.partition_point(|item| &item[..MIN_ORIGINAL_BYTES] < prefix);
            self.charge(1)?;
            for item in &items[first..] {
                if &item[..MIN_ORIGINAL_BYTES] != prefix {
                    break;
                }
                self.charge(item.len())?;
                if text[start..].starts_with(item) {
                    return self.refuse(SendRefusal::KnownOriginal);
                }
            }
        }
        Ok(())
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
        let start = text.trim_start_matches([' ', '\t', '\n', '\r']);
        if level > self.limits.embedded || !start.starts_with(['{', '[', '"']) {
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
