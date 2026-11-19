//! Borrowed complete JSON preflight before bounded metadata deserialization.
//! Stack-only duplicate keys include equivalent escaped spellings. All strings
//! are counted as decoded UTF8, including surrogate-pair expansion.

use super::{AdmissionError, METADATA_BYTES, Result, text};

const DEPTH: usize = 12;
const NODES: usize = 4096;
const ITEMS: usize = 256;
const FIELDS: usize = 32;
const KEY: usize = 64;
const STRING: usize = 1024;

pub(super) fn preflight(bytes: &[u8]) -> Result<()> {
    if bytes.len() > METADATA_BYTES {
        return Err(AdmissionError::Bound);
    }
    let utf8 = text(bytes)?;
    let mut scanner = Scanner {
        bytes,
        utf8,
        position: 0,
        nodes: 0,
    };
    scanner.value(1, None)?;
    scanner.space();
    if scanner.position != bytes.len() {
        return Err(AdmissionError::Metadata);
    }
    Ok(())
}
struct Scanner<'a> {
    bytes: &'a [u8],
    utf8: &'a str,
    position: usize,
    nodes: usize,
}
impl Scanner<'_> {
    fn space(&mut self) {
        while self
            .bytes
            .get(self.position)
            .is_some_and(|b| matches!(b, b' ' | b'\r' | b'\n' | b'\t'))
        {
            self.position += 1;
        }
    }
    fn take(&mut self, byte: u8) -> Result<()> {
        if self.bytes.get(self.position) != Some(&byte) {
            return Err(AdmissionError::Metadata);
        }
        self.position += 1;
        Ok(())
    }
    fn value(&mut self, depth: usize, field: Option<&[u8]>) -> Result<()> {
        self.space();
        self.nodes += 1;
        if depth > DEPTH || self.nodes > NODES {
            return Err(AdmissionError::Bound);
        }
        match self.bytes.get(self.position) {
            Some(b'{') => self.object(depth),
            Some(b'[') => {
                let cap = match field {
                    Some(b"files") => super::CODE_FILES + super::ASSETS + super::ARTIFACTS,
                    Some(b"sources") => super::SOURCES,
                    Some(b"direct_packages" | b"requires") => super::PACKAGES,
                    _ => ITEMS,
                };
                self.array(depth, cap)
            }
            Some(b'"') => {
                let cap = match field {
                    Some(b"id" | b"source") => super::IDENTIFIER_BYTES,
                    Some(b"origin") => 512,
                    Some(b"version") => 32,
                    _ => STRING,
                };
                self.string(None, cap).map(|_| ())
            }
            Some(b'0'..=b'9') => self.number(),
            Some(b't') => self.literal(b"true"),
            Some(b'f') => self.literal(b"false"),
            Some(b'n') => self.literal(b"null"),
            _ => Err(AdmissionError::Metadata),
        }
    }
    fn literal(&mut self, literal: &[u8]) -> Result<()> {
        for &byte in literal {
            self.take(byte)?;
        }
        Ok(())
    }
    fn number(&mut self) -> Result<()> {
        let start = self.position;
        while self
            .bytes
            .get(self.position)
            .is_some_and(u8::is_ascii_digit)
        {
            self.position += 1;
            if self.position - start > 20 {
                return Err(AdmissionError::Bound);
            }
        }
        if self.position - start > 1 && self.bytes[start] == b'0' {
            return Err(AdmissionError::Metadata);
        }
        // This data schema uses unsigned integers only, never NaN/float or
        // exponent/truncation conversions. Delimiters are checked by caller.
        Ok(())
    }
    fn object(&mut self, depth: usize) -> Result<()> {
        self.take(b'{')?;
        self.space();
        if self.bytes.get(self.position) == Some(&b'}') {
            self.position += 1;
            return Ok(());
        }
        let mut keys = [[0u8; KEY]; FIELDS];
        let mut lengths = [0usize; FIELDS];
        let mut fields = 0;
        loop {
            if fields == FIELDS {
                return Err(AdmissionError::Bound);
            }
            self.space();
            lengths[fields] = self.string(Some(&mut keys[fields]), KEY)?;
            if (0..fields).any(|old| keys[old][..lengths[old]] == keys[fields][..lengths[fields]]) {
                return Err(AdmissionError::Metadata);
            }
            fields += 1;
            self.space();
            self.take(b':')?;
            self.value(depth + 1, Some(&keys[fields - 1][..lengths[fields - 1]]))?;
            self.space();
            match self.bytes.get(self.position) {
                Some(b'}') => {
                    self.position += 1;
                    return Ok(());
                }
                Some(b',') => self.position += 1,
                _ => return Err(AdmissionError::Metadata),
            }
        }
    }
    fn array(&mut self, depth: usize, cap: usize) -> Result<()> {
        self.take(b'[')?;
        self.space();
        if self.bytes.get(self.position) == Some(&b']') {
            self.position += 1;
            return Ok(());
        }
        let mut items = 0;
        loop {
            items += 1;
            if items > cap {
                return Err(AdmissionError::Bound);
            }
            self.value(depth + 1, None)?;
            self.space();
            match self.bytes.get(self.position) {
                Some(b']') => {
                    self.position += 1;
                    return Ok(());
                }
                Some(b',') => self.position += 1,
                _ => return Err(AdmissionError::Metadata),
            }
        }
    }
    fn hex4(&mut self) -> Result<u16> {
        let mut code = 0;
        for _ in 0..4 {
            let byte = self
                .bytes
                .get(self.position)
                .ok_or(AdmissionError::Metadata)?;
            let digit = match byte {
                b'0'..=b'9' => byte - b'0',
                b'a'..=b'f' => byte - b'a' + 10,
                b'A'..=b'F' => byte - b'A' + 10,
                _ => return Err(AdmissionError::Metadata),
            };
            code = code * 16 + u16::from(digit);
            self.position += 1;
        }
        Ok(code)
    }
    fn string(&mut self, mut key: Option<&mut [u8; KEY]>, cap: usize) -> Result<usize> {
        self.take(b'"')?;
        let mut length = 0usize;
        loop {
            let byte = self
                .bytes
                .get(self.position)
                .copied()
                .ok_or(AdmissionError::Metadata)?;
            self.position += 1;
            if byte == b'"' {
                return Ok(length);
            }
            let c = if byte == b'\\' {
                let escape = self
                    .bytes
                    .get(self.position)
                    .copied()
                    .ok_or(AdmissionError::Metadata)?;
                self.position += 1;
                match escape {
                    b'"' | b'\\' | b'/' => char::from(escape),
                    b'b' => '\u{8}',
                    b'f' => '\u{c}',
                    b'n' => '\n',
                    b'r' => '\r',
                    b't' => '\t',
                    b'u' => {
                        let first = self.hex4()?;
                        let code = if (0xd800..=0xdbff).contains(&first) {
                            self.take(b'\\')?;
                            self.take(b'u')?;
                            let second = self.hex4()?;
                            if !(0xdc00..=0xdfff).contains(&second) {
                                return Err(AdmissionError::Metadata);
                            }
                            0x10000 + ((u32::from(first) - 0xd800) << 10) + u32::from(second)
                                - 0xdc00
                        } else {
                            u32::from(first)
                        };
                        char::from_u32(code).ok_or(AdmissionError::Metadata)?
                    }
                    _ => return Err(AdmissionError::Metadata),
                }
            } else if byte < 32 {
                return Err(AdmissionError::Metadata);
            } else if byte < 128 {
                char::from(byte)
            } else {
                // Whole UTF8 was validated once; slicing at the current scalar
                // boundary avoids repeatedly validating the remaining body.
                let tail = &self.utf8[self.position - 1..];
                let c = tail.chars().next().ok_or(AdmissionError::Metadata)?;
                self.position += c.len_utf8() - 1;
                c
            };
            let end = length + c.len_utf8();
            if end > cap {
                return Err(AdmissionError::Bound);
            }
            if let Some(output) = key.as_deref_mut() {
                c.encode_utf8(&mut output[length..end]);
            }
            length = end;
        }
    }
}
