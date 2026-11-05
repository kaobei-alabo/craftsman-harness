//! Complete original-byte coverage with retained source-line positions.
//! No raw accessor/Debug/Serialize. The bitmap is one bit per original byte;
//! the matcher is the SAME bounded stream core, never O(seed-length) per match.

use super::{MARKER, RedactionError, Redactor, Result};

const MAX_SOURCE_BYTES: usize = 10 * 1024 * 1024;
const CHUNK: usize = 8192;

pub(crate) struct MaskedText<'a> {
    original: &'a str,
    covered: Vec<u64>,
}
impl<'a> MaskedText<'a> {
    pub(super) fn new(redactor: &Redactor, input: &'a str) -> Result<Self> {
        if input.len() > MAX_SOURCE_BYTES {
            return Err(RedactionError::Record);
        }
        let mut view = Self {
            original: input,
            covered: vec![0; input.len().div_ceil(64)],
        };
        let mut stream = redactor.byte_stream();
        let mut position = 0;
        for &byte in input.as_bytes() {
            stream.accept(byte);
            if stream.pending.len() >= stream.inner.longest {
                let (_, covered) = stream.covered_byte();
                view.mark(position, covered);
                position += 1;
            }
        }
        // The ENTIRE original UTF-8 record is present and bounded. This is a
        // real record EOF, never a page/read/output cap or a partial pipe read.
        while !stream.pending.is_empty() {
            let (_, covered) = stream.covered_byte();
            view.mark(position, covered);
            position += 1;
        }
        stream.abort();
        debug_assert_eq!(position, input.len());
        Ok(view)
    }
    fn mark(&mut self, position: usize, covered: bool) {
        if covered {
            self.covered[position / 64] |= 1u64 << (position % 64);
        }
    }
    fn at(&self, position: usize) -> bool {
        self.covered[position / 64] & (1u64 << (position % 64)) != 0
    }
    pub(crate) fn lines(&self) -> Lines<'_, 'a> {
        Lines {
            source: self,
            segments: self.original.split_inclusive('\n'),
            offset: 0,
            number: 0,
        }
    }
}

pub(crate) struct Lines<'v, 'a> {
    source: &'v MaskedText<'a>,
    segments: std::str::SplitInclusive<'a, char>,
    offset: usize,
    number: usize,
}
impl<'v, 'a> Iterator for Lines<'v, 'a> {
    type Item = Line<'v, 'a>;
    fn next(&mut self) -> Option<Self::Item> {
        let segment = self.segments.next()?;
        // Use Rust's OWN line semantics, including CRLF, final bare CR,
        // empty lines and no invented trailing empty line. Position is still
        // measured in ORIGINAL bytes, not a post-redaction line count.
        let content = segment.lines().next().unwrap_or("");
        let start = self.offset;
        self.offset += segment.len();
        self.number += 1;
        Some(Line {
            source: self.source,
            start,
            end: start + content.len(),
            number: self.number,
        })
    }
}

pub(crate) struct Line<'v, 'a> {
    source: &'v MaskedText<'a>,
    start: usize,
    end: usize,
    number: usize,
}
impl<'v, 'a> Line<'v, 'a> {
    pub(crate) fn number(&self) -> usize {
        self.number
    }
    /// A boolean match against the ORIGINAL line, never redacted query/data.
    pub(crate) fn matches(&self, pattern: &str) -> bool {
        self.source.original[self.start..self.end].contains(pattern)
    }
    pub(crate) fn chunks(&self) -> Chunks<'v, 'a> {
        Chunks {
            source: self.source,
            position: self.start,
            end: self.end,
            failed: false,
        }
    }
}

pub(crate) struct Chunks<'v, 'a> {
    source: &'v MaskedText<'a>,
    position: usize,
    end: usize,
    failed: bool,
}
impl<'a> Iterator for Chunks<'_, 'a> {
    type Item = Result<&'a str>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.failed || self.position == self.end {
            return None;
        }
        let start = self.position;
        let covered = self.source.at(start);
        while self.position < self.end {
            let ch = self.source.original[self.position..self.end]
                .chars()
                .next()
                .expect("validated original scalar");
            let end = self.position + ch.len_utf8();
            let current = self.source.at(self.position);
            if (self.position..end).any(|p| self.source.at(p) != current) {
                self.failed = true;
                return Some(Err(RedactionError::Utf8));
            }
            if current != covered || (!covered && end - start > CHUNK) {
                break;
            }
            self.position = end;
        }
        Some(if covered {
            // A multiline covered range gets one marker in EACH original line.
            // Every covered byte stays hidden, including a page's middle/tail.
            Ok(std::str::from_utf8(MARKER).expect("constant UTF-8 marker"))
        } else {
            Ok(&self.source.original[start..self.position])
        })
    }
}

#[cfg(test)]
mod tests;
