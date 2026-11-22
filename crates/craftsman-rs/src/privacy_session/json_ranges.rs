//! Lexical facts about the strings of one JSON document.
//!
//! Nothing selects this module. It reads a document and reports where its
//! strings are, and that is all. It knows no source, permission, field,
//! policy or alias, it grants nothing, and it produces no document text: it
//! never writes, rewrites, normalizes, masks or restores a byte. Whoever
//! restores a value into a file decides, with its own checks, what a fact
//! reported here may be used for.
//!
//! [`JsonRanges::index`] admits a complete document or refuses it with one
//! fault and its byte offset. For an admitted document it lists every string
//! token in the order written: whether it is the key of an object or a
//! value, its exact byte span, its decoded text, and the keys and array
//! positions it stands under. An object that has the same decoded key twice
//! is refused, however each of the two is escaped.
//!
//! [`JsonRanges::locate`] answers one question about a byte range of that
//! document: does it lie wholly inside the content of one string value? A
//! key, a quote, anything between strings, a range that reaches from one
//! string into another and a range that cuts an escape or a character in
//! two are each refused with their own reason.
//! [`JsonRanges::locate_all`] asks it of every occurrence of a text, found
//! the way a text replacement finds them.
//!
//! The document as written and its decoded text are kept apart. Offsets of
//! the document are offsets of its bytes as given; decoded text is a
//! separate copy that is only reported, and whether a range has an escape in
//! it is reported with it. Text that only decodes to something was not
//! written as that something.
//!
//! The bounds are the ones a restored file already has: 256 KiB of document,
//! 64 levels of nesting and 4096 strings. A number must also fit a 64-bit
//! float, as the existing admission of such a document requires.

#[cfg(test)]
mod tests;

use std::ops::Range;
use zeroize::Zeroizing;

/// The most bytes of a document: the bound of a file an edit is derived for
/// and of a text a value is restored into.
pub const MAX_DOCUMENT: usize = 256 * 1024;
/// The deepest nesting of objects and arrays.
pub const MAX_DEPTH: usize = 64;
/// The most string tokens of a document, keys and values together.
pub const MAX_STRINGS: usize = 4096;

/// Why a document is not indexed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JsonFault {
    /// Longer than [`MAX_DOCUMENT`].
    Size,
    /// Nested deeper than [`MAX_DEPTH`].
    Depth,
    /// More than [`MAX_STRINGS`] string tokens.
    Strings,
    /// Not UTF-8.
    Utf8,
    /// A byte that cannot stand where it is, or the document ends early.
    Structure,
    /// A control character written as it is inside a string.
    Control,
    /// An escape JSON does not have, or `\u` without four hex digits.
    Escape,
    /// One half of a surrogate pair without the other.
    Surrogate,
    /// A number that is not written the way JSON writes numbers.
    Number,
    /// A number beyond a 64-bit float.
    NumberRange,
    /// A key its object already has. `first` is where the earlier key's
    /// token starts.
    DuplicateKey { first: usize },
    /// Something after the document's one value.
    Trailing,
}

/// One fault and the byte offset it was found at. It holds no document text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JsonError {
    pub fault: JsonFault,
    pub at: usize,
}

fn fault(fault: JsonFault, at: usize) -> JsonError {
    JsonError { fault, at }
}

/// What a string token is in its document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StringRole {
    /// The key of an object's member.
    Key,
    /// A value: the document itself, a member's value or an array's element.
    Value,
}

/// One string token of an indexed document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JsonString {
    role: StringRole,
    token: Range<usize>,
    decoded: Range<usize>,
    escapes: Range<usize>,
    place: Place,
}

impl JsonString {
    pub fn role(&self) -> StringRole {
        self.role
    }

    /// The token as written, both quotes included.
    pub fn token(&self) -> Range<usize> {
        self.token.clone()
    }

    /// What stands between the quotes, as written.
    pub fn content(&self) -> Range<usize> {
        self.token.start + 1..self.token.end - 1
    }

    /// Whether an escape stands in it. Without one, its content as written
    /// is its decoded text.
    pub fn escaped(&self) -> bool {
        !self.escapes.is_empty()
    }
}

/// One step from the document's value down to a string.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathStep<'a> {
    /// The member with this decoded key.
    Key(&'a str),
    /// The element at this position.
    Index(usize),
}

/// Where a string or a container stands. Containers are numbered in the
/// order they open; a key is named by its string's number.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Place {
    Root,
    KeyOf(u32),
    Member { object: u32, key: u32 },
    Element { array: u32, index: u32 },
}

/// One escape of a string: where it is written and what it decoded to. A
/// surrogate pair is one escape of twelve bytes.
#[derive(Clone, Copy)]
struct Escape {
    raw: u32,
    raw_len: u8,
    decoded: u32,
    decoded_len: u8,
}

enum Open {
    /// An object and its keys so far, ordered by their decoded text.
    Object { id: u32, keys: Vec<u32> },
    /// An array and the position of its latest element.
    Array { id: u32, index: u32 },
}

/// A byte range that lies wholly inside the content of one string value.
/// Only [`JsonRanges::locate`] makes one, and it is a fact about that
/// document alone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Within {
    string: usize,
    range: Range<usize>,
    content: Range<usize>,
    decoded: Range<usize>,
    literal: bool,
}

impl Within {
    /// Which of [`JsonRanges::strings`] it lies in. That string is a value.
    pub fn string(&self) -> usize {
        self.string
    }

    /// The range that was asked about, as given.
    pub fn range(&self) -> Range<usize> {
        self.range.clone()
    }

    /// The value's content before the range, as written.
    pub fn before(&self) -> Range<usize> {
        self.content.start..self.range.start
    }

    /// The value's content after the range, as written.
    pub fn after(&self) -> Range<usize> {
        self.range.end..self.content.end
    }

    /// The same span in the value's decoded text.
    pub fn decoded(&self) -> Range<usize> {
        self.decoded.clone()
    }

    /// No escape stands in the range, so its bytes as written are its
    /// decoded text.
    pub fn literal(&self) -> bool {
        self.literal
    }
}

/// Why a byte range does not lie wholly inside one string value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outside {
    /// Not a range of this document.
    Document,
    /// An empty text has no occurrence.
    Empty,
    /// It touches no string: punctuation, white space, a number or a
    /// literal.
    Structure,
    /// It lies in one string token and takes in a quote of it.
    Delimiter { string: usize },
    /// It lies in the content of a key.
    Key { string: usize },
    /// It reaches beyond one string: into another string, or into what
    /// stands between strings.
    Spans,
    /// It lies in one value and begins or ends inside an escape or inside
    /// the bytes of one character. `at` is that end.
    Splits { at: usize },
}

/// The first occurrence of a text that does not lie wholly inside one
/// string value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refused {
    /// Which occurrence, counted from zero in the order written.
    pub occurrence: usize,
    pub range: Range<usize>,
    pub outside: Outside,
}

/// The strings of one admitted document. It borrows the document, so its
/// offsets are offsets of exactly those bytes.
pub struct JsonRanges<'a> {
    text: &'a str,
    strings: Vec<JsonString>,
    /// Every string's decoded text, one after the other.
    decoded: Zeroizing<String>,
    escapes: Vec<Escape>,
    containers: Vec<Place>,
}

impl std::fmt::Debug for JsonRanges<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JsonRanges")
            .field("bytes", &self.text.len())
            .field("strings", &self.strings.len())
            .finish()
    }
}

impl<'a> JsonRanges<'a> {
    /// Admits `document` as one complete JSON document within the bounds and
    /// indexes its strings, or refuses it. The document is only read.
    pub fn index(document: &'a [u8]) -> Result<Self, JsonError> {
        if document.len() > MAX_DOCUMENT {
            return Err(fault(JsonFault::Size, MAX_DOCUMENT));
        }
        let text = std::str::from_utf8(document)
            .map_err(|error| fault(JsonFault::Utf8, error.valid_up_to()))?;
        let mut ranges = Self {
            text,
            strings: Vec::new(),
            // Decoded text is never longer than what it is decoded from, so
            // this never grows and leaves no copy behind.
            decoded: Zeroizing::new(String::with_capacity(document.len())),
            escapes: Vec::new(),
            containers: Vec::new(),
        };
        let mut open: Vec<Open> = Vec::new();
        let mut at = skip(document, 0);
        let mut place = Place::Root;
        'value: loop {
            // A value begins at `at` and stands at `place`.
            match document.get(at) {
                Some(b'{' | b'[') => {
                    if open.len() == MAX_DEPTH {
                        return Err(fault(JsonFault::Depth, at));
                    }
                    let id = ranges.containers.len() as u32;
                    ranges.containers.push(place);
                    let object = document[at] == b'{';
                    at = skip(document, at + 1);
                    if object {
                        open.push(Open::Object {
                            id,
                            keys: Vec::new(),
                        });
                        if document.get(at) != Some(&b'}') {
                            place = ranges.key(&mut at, &mut open)?;
                            continue 'value;
                        }
                    } else {
                        open.push(Open::Array { id, index: 0 });
                        if document.get(at) != Some(&b']') {
                            place = Place::Element {
                                array: id,
                                index: 0,
                            };
                            continue 'value;
                        }
                    }
                    at += 1;
                    open.pop();
                }
                Some(b'"') => at = ranges.string(at, StringRole::Value, place)?,
                Some(b'-' | b'0'..=b'9') => at = number(text, at)?,
                Some(b't' | b'f' | b'n') => at = literal(document, at)?,
                _ => return Err(fault(JsonFault::Structure, at)),
            }
            // A value is complete. What follows it says where the next one
            // stands, or that the document is over.
            loop {
                at = skip(document, at);
                match open.last_mut() {
                    None if at == document.len() => return Ok(ranges),
                    None => return Err(fault(JsonFault::Trailing, at)),
                    Some(Open::Object { .. }) => match document.get(at) {
                        Some(b',') => {
                            at = skip(document, at + 1);
                            place = ranges.key(&mut at, &mut open)?;
                            continue 'value;
                        }
                        Some(b'}') => at += 1,
                        _ => return Err(fault(JsonFault::Structure, at)),
                    },
                    Some(Open::Array { id, index }) => match document.get(at) {
                        Some(b',') => {
                            *index += 1;
                            place = Place::Element {
                                array: *id,
                                index: *index,
                            };
                            at = skip(document, at + 1);
                            continue 'value;
                        }
                        Some(b']') => at += 1,
                        _ => return Err(fault(JsonFault::Structure, at)),
                    },
                }
                open.pop();
            }
        }
    }

    /// One member's key and colon. `at` ends where the member's value
    /// begins.
    fn key(&mut self, at: &mut usize, open: &mut [Open]) -> Result<Place, JsonError> {
        let document = self.text.as_bytes();
        let start = *at;
        let Some(Open::Object { id, keys }) = open.last_mut() else {
            return Err(fault(JsonFault::Structure, start));
        };
        if document.get(start) != Some(&b'"') {
            return Err(fault(JsonFault::Structure, start));
        }
        let end = self.string(start, StringRole::Key, Place::KeyOf(*id))?;
        let this = self.strings.len() - 1;
        let name = &self.decoded[self.strings[this].decoded.clone()];
        let earlier = keys.binary_search_by(|&other| {
            self.decoded[self.strings[other as usize].decoded.clone()].cmp(name)
        });
        match earlier {
            Ok(found) => {
                let first = self.strings[keys[found] as usize].token.start;
                return Err(fault(JsonFault::DuplicateKey { first }, start));
            }
            Err(slot) => keys.insert(slot, this as u32),
        }
        let colon = skip(document, end);
        if document.get(colon) != Some(&b':') {
            return Err(fault(JsonFault::Structure, colon));
        }
        *at = skip(document, colon + 1);
        Ok(Place::Member {
            object: *id,
            key: this as u32,
        })
    }

    /// The string token whose opening quote is at `start`. Gives where the
    /// token ends.
    fn string(&mut self, start: usize, role: StringRole, place: Place) -> Result<usize, JsonError> {
        if self.strings.len() == MAX_STRINGS {
            return Err(fault(JsonFault::Strings, start));
        }
        let text = self.text;
        let document = text.as_bytes();
        let decoded = self.decoded.len();
        let escapes = self.escapes.len();
        let mut at = start + 1;
        loop {
            // Up to the next quote, backslash or control character the text
            // is what it decodes to. Those are single bytes, so the run
            // begins and ends between characters.
            let run = at;
            while document
                .get(at)
                .is_some_and(|byte| !matches!(byte, b'"' | b'\\' | 0..=0x1f))
            {
                at += 1;
            }
            self.decoded.push_str(&text[run..at]);
            match document.get(at) {
                None => return Err(fault(JsonFault::Structure, at)),
                Some(b'"') => break,
                Some(b'\\') => at = self.escape(at)?,
                Some(_) => return Err(fault(JsonFault::Control, at)),
            }
        }
        self.strings.push(JsonString {
            role,
            token: start..at + 1,
            decoded: decoded..self.decoded.len(),
            escapes: escapes..self.escapes.len(),
            place,
        });
        Ok(at + 1)
    }

    /// The escape whose backslash is at `start`. Gives where it ends.
    fn escape(&mut self, start: usize) -> Result<usize, JsonError> {
        let document = self.text.as_bytes();
        let mut raw_len = 2;
        let scalar = match document.get(start + 1) {
            None => return Err(fault(JsonFault::Structure, document.len())),
            Some(b'"') => '"',
            Some(b'\\') => '\\',
            Some(b'/') => '/',
            Some(b'b') => '\u{8}',
            Some(b'f') => '\u{c}',
            Some(b'n') => '\n',
            Some(b'r') => '\r',
            Some(b't') => '\t',
            Some(b'u') => {
                let unit = hex4(document, start + 2, start)?;
                let code = match unit {
                    0xD800..=0xDBFF => {
                        if document.get(start + 6..start + 8) != Some(&b"\\u"[..]) {
                            return Err(fault(JsonFault::Surrogate, start));
                        }
                        let low = hex4(document, start + 8, start + 6)?;
                        if !(0xDC00..=0xDFFF).contains(&low) {
                            return Err(fault(JsonFault::Surrogate, start));
                        }
                        raw_len = 12;
                        0x10000 + ((unit - 0xD800) << 10) + (low - 0xDC00)
                    }
                    0xDC00..=0xDFFF => return Err(fault(JsonFault::Surrogate, start)),
                    unit => {
                        raw_len = 6;
                        unit
                    }
                };
                char::from_u32(code).ok_or(fault(JsonFault::Surrogate, start))?
            }
            Some(_) => return Err(fault(JsonFault::Escape, start)),
        };
        self.escapes.push(Escape {
            raw: start as u32,
            raw_len,
            decoded: self.decoded.len() as u32,
            decoded_len: scalar.len_utf8() as u8,
        });
        self.decoded.push(scalar);
        Ok(start + usize::from(raw_len))
    }

    /// Every string token, in the order written.
    pub fn strings(&self) -> &[JsonString] {
        &self.strings
    }

    /// The decoded text of one of [`Self::strings`].
    pub fn decoded(&self, string: usize) -> Option<&str> {
        let string = self.strings.get(string)?;
        Some(&self.decoded[string.decoded.clone()])
    }

    /// The keys and positions one of [`Self::strings`] stands under, from
    /// the document's value down. A value's path ends with its own key or
    /// position; a key's path is the path of its object. The document's own
    /// value has the empty path.
    pub fn path(&self, string: usize) -> Option<Vec<PathStep<'_>>> {
        let mut place = self.strings.get(string)?.place;
        let mut steps = Vec::new();
        loop {
            let container = match place {
                Place::Root => break,
                Place::KeyOf(object) => object,
                Place::Member { object, key } => {
                    let key = &self.strings[key as usize];
                    steps.push(PathStep::Key(&self.decoded[key.decoded.clone()]));
                    object
                }
                Place::Element { array, index } => {
                    steps.push(PathStep::Index(index as usize));
                    array
                }
            };
            place = self.containers[container as usize];
        }
        steps.reverse();
        Some(steps)
    }

    /// Whether `range` lies wholly inside the content of one string value.
    ///
    /// It does when every byte of it is content of that one value and it
    /// begins and ends between two of the value's units: a character as
    /// written, or one whole escape. An empty range is a position, and lies
    /// in a value when it stands between two such units or at either end of
    /// the content.
    pub fn locate(&self, range: Range<usize>) -> Result<Within, Outside> {
        if range.start > range.end || range.end > self.text.len() {
            return Err(Outside::Document);
        }
        // The first string that does not end at or before the range's start.
        let string = self
            .strings
            .partition_point(|string| string.token.end <= range.start);
        let Some(found) = self.strings.get(string) else {
            return Err(Outside::Structure);
        };
        if range.end <= found.token.start {
            return Err(Outside::Structure);
        }
        if range.start < found.token.start || range.end > found.token.end {
            return Err(Outside::Spans);
        }
        let content = found.content();
        if range.start < content.start || range.end > content.end {
            return Err(Outside::Delimiter { string });
        }
        if found.role == StringRole::Key {
            return Err(Outside::Key { string });
        }
        let escapes = &self.escapes[found.escapes.clone()];
        let start = self
            .unit(found, escapes, range.start)
            .ok_or(Outside::Splits { at: range.start })?;
        let end = self
            .unit(found, escapes, range.end)
            .ok_or(Outside::Splits { at: range.end })?;
        // Both ends stand between units, so an escape that begins in the
        // range is wholly in it.
        let first = escapes.partition_point(|escape| (escape.raw as usize) < range.start);
        let literal = escapes
            .get(first)
            .is_none_or(|escape| escape.raw as usize >= range.end);
        Ok(Within {
            string,
            range,
            content,
            decoded: start..end,
            literal,
        })
    }

    /// Where `at`, an offset in the content of `string`, falls in that
    /// string's decoded text. Nothing when it is inside an escape or inside
    /// the bytes of one character.
    fn unit(&self, string: &JsonString, escapes: &[Escape], at: usize) -> Option<usize> {
        if self.text.as_bytes()[at] & 0xC0 == 0x80 {
            return None;
        }
        let before = escapes.partition_point(|escape| escape.raw as usize <= at);
        let arena = match before.checked_sub(1).map(|last| escapes[last]) {
            None => string.decoded.start + (at - string.content().start),
            Some(escape) => {
                let raw = escape.raw as usize;
                let end = raw + usize::from(escape.raw_len);
                if at == raw {
                    escape.decoded as usize
                } else if at < end {
                    return None;
                } else {
                    escape.decoded as usize + usize::from(escape.decoded_len) + (at - end)
                }
            }
        };
        Some(arena - string.decoded.start)
    }

    /// Every occurrence of `needle` in the document as written, each lying
    /// wholly inside one string value, or the first one that does not.
    ///
    /// Occurrences are found the way `str::replace` finds them: from the
    /// left, not overlapping. A text that does not occur gives no
    /// occurrence. An empty text is refused.
    pub fn locate_all(&self, needle: &str) -> Result<Vec<Within>, Refused> {
        if needle.is_empty() {
            return Err(Refused {
                occurrence: 0,
                range: 0..0,
                outside: Outside::Empty,
            });
        }
        let mut found = Vec::new();
        for (occurrence, (start, _)) in self.text.match_indices(needle).enumerate() {
            let range = start..start + needle.len();
            match self.locate(range.clone()) {
                Ok(within) => found.push(within),
                Err(outside) => {
                    return Err(Refused {
                        occurrence,
                        range,
                        outside,
                    });
                }
            }
        }
        Ok(found)
    }
}

fn skip(document: &[u8], mut at: usize) -> usize {
    while matches!(document.get(at), Some(b' ' | b'\t' | b'\n' | b'\r')) {
        at += 1;
    }
    at
}

fn digits(document: &[u8], mut at: usize) -> usize {
    while document.get(at).is_some_and(u8::is_ascii_digit) {
        at += 1;
    }
    at
}

/// The four hex digits at `at` of the escape that begins at `escape`.
fn hex4(document: &[u8], at: usize, escape: usize) -> Result<u32, JsonError> {
    let mut unit = 0;
    for position in at..at + 4 {
        let digit = document
            .get(position)
            .ok_or(fault(JsonFault::Structure, document.len()))?;
        let digit = char::from(*digit)
            .to_digit(16)
            .ok_or(fault(JsonFault::Escape, escape))?;
        unit = unit * 16 + digit;
    }
    Ok(unit)
}

/// The number that begins at `start`. Gives where it ends.
fn number(text: &str, start: usize) -> Result<usize, JsonError> {
    let document = text.as_bytes();
    let malformed = fault(JsonFault::Number, start);
    let mut at = start + usize::from(document[start] == b'-');
    match document.get(at) {
        Some(b'0') => at += 1,
        Some(b'1'..=b'9') => at = digits(document, at),
        _ => return Err(malformed),
    }
    if document.get(at) == Some(&b'.') {
        let end = digits(document, at + 1);
        if end == at + 1 {
            return Err(malformed);
        }
        at = end;
    }
    if matches!(document.get(at), Some(b'e' | b'E')) {
        let mut exponent = at + 1;
        if matches!(document.get(exponent), Some(b'+' | b'-')) {
            exponent += 1;
        }
        let end = digits(document, exponent);
        if end == exponent {
            return Err(malformed);
        }
        at = end;
    }
    // `01`, `1.2.3`, `0x1` and `1e5e5` are one malformed number, not a
    // number and something after it.
    if document
        .get(at)
        .is_some_and(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'+' | b'-'))
    {
        return Err(malformed);
    }
    match text[start..at].parse::<f64>() {
        Ok(value) if value.is_finite() => Ok(at),
        _ => Err(fault(JsonFault::NumberRange, start)),
    }
}

/// The `true`, `false` or `null` that begins at `start`. Gives where it
/// ends.
fn literal(document: &[u8], start: usize) -> Result<usize, JsonError> {
    for word in [&b"true"[..], b"false", b"null"] {
        let end = start + word.len();
        if document[start..].starts_with(word)
            && !document
                .get(end)
                .is_some_and(|byte| byte.is_ascii_alphanumeric())
        {
            return Ok(end);
        }
    }
    Err(fault(JsonFault::Structure, start))
}
