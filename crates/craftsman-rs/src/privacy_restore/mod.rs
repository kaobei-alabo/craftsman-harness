//! Private host views for known aliases: display text and provisional file data.
//!
//! This is not a permission, path, code-language or file-staleness boundary. It
//! does not dispatch a tool, render a terminal cell, send model data or activate
//! a hook. A future host adapter must classify non-executable text or a complete
//! JSON document, then run the real file/approval gates on the restored candidate.
//! Public reachability is confined to the app binary, as with privacy_vault;
//! it keeps these unselected APIs typechecked without a library or CLI surface.

#[cfg(test)]
mod tests;

use crate::privacy_vault::{Category, MappingSession, PrivacyError, ResolutionRecords};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use zeroize::Zeroizing;

const MAX_TEXT: usize = 256 * 1024;
const MAX_RECORDS: usize = 512;
const MAX_REPLACEMENTS: usize = 4096;
const MAX_WORK: usize = 64 * 1024 * 1024;
const MAX_JSON_DEPTH: usize = 64;
const MAX_JSON_STRINGS: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RestoreError {
    #[error("The private restoration input exceeds its bounds.")]
    Bounds,
    #[error("The private restoration input is invalid or ambiguous.")]
    Input,
    #[error("A file candidate contains an unresolved private alias.")]
    UnknownAlias,
    #[error("The private restoration scope is unavailable.")]
    Scope,
}

type Result<T> = std::result::Result<T, RestoreError>;

/// These are data-format selections, not authority to classify a destination.
/// Code, shell, paths, JSON keys and arbitrary tool fields are not supported.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileDataFormat {
    PlainText,
    JsonDocument,
}

/// Borrowed actual-session records. Equal namespace labels do not substitute
/// for the vault's actual live session identity.
pub struct Restorer<'a> {
    session: &'a MappingSession,
    records: &'a ResolutionRecords,
}

#[derive(Clone, Copy)]
struct Replacement {
    start: usize,
    end: usize,
    record: usize,
}

struct RestoredText {
    text: Zeroizing<String>,
    replacements: usize,
}

impl<'a> Restorer<'a> {
    pub fn from_records(
        session: &'a MappingSession,
        records: &'a ResolutionRecords,
    ) -> Result<Self> {
        let this = Self { session, records };
        if this.check().is_err() {
            return this.refuse(RestoreError::Scope);
        }
        if this.records.len().map_err(|_| RestoreError::Scope)? > MAX_RECORDS {
            return this.refuse(RestoreError::Bounds);
        }
        Ok(this)
    }

    fn check(&self) -> Result<()> {
        self.records
            .validate_session_binding(self.session)
            .map_err(|_| RestoreError::Scope)
    }

    fn refuse<T>(&self, error: RestoreError) -> Result<T> {
        self.session.close();
        Err(error)
    }

    fn whole(&self, result: Result<Zeroizing<String>>) -> Result<Zeroizing<String>> {
        match result {
            Ok(value) => {
                self.check()?;
                Ok(value)
            }
            Err(error) => self.refuse(error),
        }
    }

    /// Whole-response strategy: nothing is emitted while aliases may be split
    /// between transport chunks. Unknown/altered tokens stay visible as tokens;
    /// they never select originals. This API owns no stream or output sink.
    pub fn display(&self, text: &str) -> Result<DisplayView<'a>> {
        self.check()?;
        let text = self.whole(self.restore_text(text, false).map(|view| view.text))?;
        Ok(DisplayView {
            text,
            session: self.session,
            records: self.records,
        })
    }

    /// The original behind one occurrence of this scope's alias: the alias
    /// of `record` standing at `range` of `text` as a whole token. The caller
    /// names the occurrence; this checks it against the live scope's own
    /// record. `None` when the alias stands there inside a longer token, which
    /// stays as it is. An occurrence whose text is not that record's alias is
    /// refused: whoever named it does not hold what it claims.
    pub fn occurrence(
        &self,
        text: &str,
        range: std::ops::Range<usize>,
        record: usize,
    ) -> Result<Option<Zeroizing<String>>> {
        self.check()?;
        if text.len() > MAX_TEXT {
            return self.refuse(RestoreError::Bounds);
        }
        let Some(token) = text.get(range.clone()) else {
            return self.refuse(RestoreError::Input);
        };
        let found = self
            .records
            .with_host_field(record, |field| {
                let alias = field.known_alias();
                if alias.is_empty() || alias.len() > 256 || alias != token {
                    return Err(RestoreError::Input);
                }
                Ok(
                    token_boundary(text, range.start, range.end, field.category()).then(|| {
                        Zeroizing::new(field.original_for_validated_host_field().to_owned())
                    }),
                )
            })
            .map_err(|_| RestoreError::Scope);
        match found {
            Ok(found) => self.whole_occurrence(found),
            Err(error) => self.refuse(error),
        }
    }

    fn whole_occurrence(
        &self,
        found: Result<Option<Zeroizing<String>>>,
    ) -> Result<Option<Zeroizing<String>>> {
        match found {
            Ok(found) => {
                self.check()?;
                Ok(found)
            }
            Err(error) => self.refuse(error),
        }
    }

    /// A restored candidate only. Its callback must not be treated as approval,
    /// read authority, a filesystem CAS, a tool-history mutation or code escaping.
    pub fn file_data(&self, format: FileDataFormat, text: &str) -> Result<FileDataView<'a>> {
        self.check()?;
        let candidate = match format {
            FileDataFormat::PlainText => self.restore_text(text, true).map(|view| view.text),
            FileDataFormat::JsonDocument => self.restore_json(text),
        };
        let text = self.whole(candidate)?;
        Ok(FileDataView {
            text,
            session: self.session,
            records: self.records,
        })
    }

    fn matches(&self, text: &str, reject_ambiguous: bool) -> Result<Vec<Replacement>> {
        let count = self.records.len().map_err(|_| RestoreError::Scope)?;
        if text.len() > MAX_TEXT
            || count > MAX_RECORDS
            || text
                .len()
                .checked_mul(count.max(1))
                .is_none_or(|n| n > MAX_WORK)
        {
            return Err(RestoreError::Bounds);
        }
        let mut matches = Vec::new();
        for record in 0..count {
            self.records
                .with_host_field(record, |field| -> Result<()> {
                    let alias = field.known_alias();
                    if alias.is_empty() || alias.len() > 256 {
                        return Err(RestoreError::Input);
                    }
                    for (start, matched) in text.match_indices(alias) {
                        let end = start + matched.len();
                        if token_boundary(text, start, end, field.category()) {
                            if matches.len() == MAX_REPLACEMENTS {
                                return Err(RestoreError::Bounds);
                            }
                            matches.push(Replacement { start, end, record });
                        } else if reject_ambiguous {
                            // A known alias embedded in another token is not
                            // an exact file field, including numeric phones
                            // that have no reserved marker of their own.
                            return Err(RestoreError::UnknownAlias);
                        }
                    }
                    Ok(())
                })
                .map_err(|_| RestoreError::Scope)??;
        }
        matches.sort_unstable_by_key(|m| (m.start, m.end));
        if matches.windows(2).any(|pair| pair[0].end > pair[1].start) {
            return Err(RestoreError::Input);
        }
        Ok(matches)
    }

    fn restore_text(&self, text: &str, refuse_unknown: bool) -> Result<RestoredText> {
        let matches = self.matches(text, refuse_unknown)?;
        let replacements = matches.len();
        if refuse_unknown {
            let mut selected = 0;
            for (start, _) in text.match_indices("pv1-") {
                while matches.get(selected).is_some_and(|m| m.end <= start) {
                    selected += 1;
                }
                if matches
                    .get(selected)
                    .is_none_or(|m| start < m.start || start >= m.end)
                {
                    return Err(RestoreError::UnknownAlias);
                }
            }
        }
        let mut length = text.len();
        for m in &matches {
            let original = self
                .records
                .with_host_field(m.record, |field| {
                    field.original_for_validated_host_field().len()
                })
                .map_err(|_| RestoreError::Scope)?;
            length = length
                .checked_sub(m.end - m.start)
                .and_then(|n| n.checked_add(original))
                .ok_or(RestoreError::Bounds)?;
            if length > MAX_TEXT {
                return Err(RestoreError::Bounds);
            }
        }
        let mut output = Zeroizing::new(String::with_capacity(length));
        let mut position = 0;
        for m in matches {
            output.push_str(&text[position..m.start]);
            self.records
                .with_host_field(m.record, |field| {
                    output.push_str(field.original_for_validated_host_field());
                })
                .map_err(|_| RestoreError::Scope)?;
            position = m.end;
        }
        output.push_str(&text[position..]);
        if output.len() != length {
            return Err(RestoreError::Input);
        }
        self.check()?;
        Ok(RestoredText {
            text: output,
            replacements,
        })
    }

    fn restore_json(&self, text: &str) -> Result<Zeroizing<String>> {
        if text.len() > MAX_TEXT {
            return Err(RestoreError::Bounds);
        }
        // Serde admits the complete grammar first. The bounded lexer below
        // preserves every byte outside changed string-value tokens; it does not
        // reserialize numbers, reorder keys or silently merge duplicate keys.
        serde_json::from_str::<serde_json::Value>(text).map_err(|_| RestoreError::Input)?;
        let bytes = text.as_bytes();
        let mut objects: Vec<Option<BTreeSet<[u8; 32]>>> = Vec::new();
        let mut output = Zeroizing::new(String::new());
        let mut position = 0;
        let mut copied = 0;
        let mut strings = 0usize;
        let mut work = 0usize;
        let mut replacements = 0usize;
        while position < bytes.len() {
            match bytes[position] {
                b'{' | b'[' => {
                    if objects.len() == MAX_JSON_DEPTH {
                        return Err(RestoreError::Bounds);
                    }
                    objects.push((bytes[position] == b'{').then(BTreeSet::new));
                    position += 1;
                }
                b'}' | b']' => {
                    objects.pop();
                    position += 1;
                }
                b'"' => {
                    strings += 1;
                    if strings > MAX_JSON_STRINGS {
                        return Err(RestoreError::Bounds);
                    }
                    let start = position;
                    position += 1;
                    while position < bytes.len() && bytes[position] != b'"' {
                        position += if bytes[position] == b'\\' { 2 } else { 1 };
                    }
                    if position >= bytes.len() {
                        return Err(RestoreError::Input);
                    }
                    position += 1;
                    let decoded = Zeroizing::new(
                        serde_json::from_str::<String>(&text[start..position])
                            .map_err(|_| RestoreError::Input)?,
                    );
                    let mut following = position;
                    while bytes.get(following).is_some_and(u8::is_ascii_whitespace) {
                        following += 1;
                    }
                    let key = bytes.get(following) == Some(&b':');
                    let count = self.records.len().map_err(|_| RestoreError::Scope)?;
                    work = work
                        .checked_add(
                            decoded
                                .len()
                                .checked_mul(count.max(1))
                                .ok_or(RestoreError::Bounds)?,
                        )
                        .ok_or(RestoreError::Bounds)?;
                    if work > MAX_WORK {
                        return Err(RestoreError::Bounds);
                    }
                    if key {
                        let Some(Some(keys)) = objects.last_mut() else {
                            return Err(RestoreError::Input);
                        };
                        if !keys.insert(Sha256::digest(decoded.as_bytes()).into()) {
                            return Err(RestoreError::Input);
                        }
                        if decoded.contains("pv1-") || !self.matches(&decoded, true)?.is_empty() {
                            return Err(RestoreError::UnknownAlias);
                        }
                    } else {
                        let restored = self.restore_text(&decoded, true)?;
                        replacements = replacements
                            .checked_add(restored.replacements)
                            .ok_or(RestoreError::Bounds)?;
                        if replacements > MAX_REPLACEMENTS {
                            return Err(RestoreError::Bounds);
                        }
                        if restored.text.as_str() != decoded.as_str() {
                            let encoded = Zeroizing::new(
                                serde_json::to_string(restored.text.as_str())
                                    .map_err(|_| RestoreError::Input)?,
                            );
                            push_bounded(&mut output, &text[copied..start])?;
                            push_bounded(&mut output, &encoded)?;
                            copied = position;
                        }
                    }
                }
                _ => {
                    position += 1;
                }
            }
        }
        push_bounded(&mut output, &text[copied..])?;
        self.check()?;
        Ok(output)
    }
}

/// One validated occurrence with the original the live scope gave for it.
pub struct Restored {
    pub range: std::ops::Range<usize>,
    pub original: Zeroizing<String>,
}

/// `text` with each validated occurrence replaced by its original, within
/// the bounds of every view of this module. The occurrences are in order
/// and do not overlap. Everything else stays as it is, alias-shaped tokens
/// that nobody validated included.
pub fn display_text(text: &str, restored: &[Restored]) -> Result<Zeroizing<String>> {
    if text.len() > MAX_TEXT || restored.len() > MAX_REPLACEMENTS {
        return Err(RestoreError::Bounds);
    }
    let mut output = Zeroizing::new(String::new());
    let mut position = 0;
    for occurrence in restored {
        let range = &occurrence.range;
        if range.start < position || range.end < range.start {
            return Err(RestoreError::Input);
        }
        let between = text.get(position..range.start).ok_or(RestoreError::Input)?;
        text.get(range.clone()).ok_or(RestoreError::Input)?;
        push_bounded(&mut output, between)?;
        push_bounded(&mut output, &occurrence.original)?;
        position = range.end;
    }
    push_bounded(
        &mut output,
        text.get(position..).ok_or(RestoreError::Input)?,
    )?;
    Ok(output)
}

/// The candidate content of a file from `text` and its validated
/// occurrences. Unlike a display, a file takes no alias that nobody
/// validated: one alias-shaped token outside the occurrences refuses the
/// whole candidate. A JSON document must be complete; its occurrences must
/// stand in string values, where each original is encoded as JSON, and its
/// keys must hold none.
pub fn file_candidate(
    format: FileDataFormat,
    text: &str,
    restored: &[Restored],
) -> Result<Zeroizing<String>> {
    if text.len() > MAX_TEXT || restored.len() > MAX_REPLACEMENTS {
        return Err(RestoreError::Bounds);
    }
    let mut covered = 0;
    for (start, _) in text.match_indices("pv1-") {
        while restored
            .get(covered)
            .is_some_and(|occurrence| occurrence.range.end <= start)
        {
            covered += 1;
        }
        if restored
            .get(covered)
            .is_none_or(|occurrence| start < occurrence.range.start)
        {
            return Err(RestoreError::UnknownAlias);
        }
    }
    match format {
        FileDataFormat::PlainText => display_text(text, restored),
        FileDataFormat::JsonDocument => {
            serde_json::from_str::<serde_json::Value>(text).map_err(|_| RestoreError::Input)?;
            let values = json_value_strings(text)?;
            let mut encoded = Vec::with_capacity(restored.len());
            for occurrence in restored {
                // An occurrence inside the raw text of a string value. An
                // alias needs no escape, so it stands there as it is.
                let inside = values.iter().any(|value| {
                    value.start < occurrence.range.start && occurrence.range.end < value.end
                });
                if !inside {
                    return Err(RestoreError::Input);
                }
                let quoted = Zeroizing::new(
                    serde_json::to_string(occurrence.original.as_str())
                        .map_err(|_| RestoreError::Input)?,
                );
                encoded.push(Restored {
                    range: occurrence.range.clone(),
                    original: Zeroizing::new(quoted[1..quoted.len() - 1].to_owned()),
                });
            }
            let candidate = display_text(text, &encoded)?;
            serde_json::from_str::<serde_json::Value>(&candidate)
                .map_err(|_| RestoreError::Input)?;
            Ok(candidate)
        }
    }
}

/// The raw ranges, quotes included, of every string value of a JSON
/// document that serde already admitted. A key that looks like an alias, a
/// repeated key and a document beyond the bounds are refused, as in
/// [`Restorer::file_data`].
pub(crate) fn json_value_strings(text: &str) -> Result<Vec<std::ops::Range<usize>>> {
    let bytes = text.as_bytes();
    let mut objects: Vec<Option<BTreeSet<[u8; 32]>>> = Vec::new();
    let mut values = Vec::new();
    let mut position = 0;
    let mut strings = 0usize;
    while position < bytes.len() {
        match bytes[position] {
            b'{' | b'[' => {
                if objects.len() == MAX_JSON_DEPTH {
                    return Err(RestoreError::Bounds);
                }
                objects.push((bytes[position] == b'{').then(BTreeSet::new));
                position += 1;
            }
            b'}' | b']' => {
                objects.pop();
                position += 1;
            }
            b'"' => {
                strings += 1;
                if strings > MAX_JSON_STRINGS {
                    return Err(RestoreError::Bounds);
                }
                let start = position;
                position += 1;
                while position < bytes.len() && bytes[position] != b'"' {
                    position += if bytes[position] == b'\\' { 2 } else { 1 };
                }
                if position >= bytes.len() {
                    return Err(RestoreError::Input);
                }
                position += 1;
                let mut following = position;
                while bytes.get(following).is_some_and(u8::is_ascii_whitespace) {
                    following += 1;
                }
                if bytes.get(following) == Some(&b':') {
                    let decoded = Zeroizing::new(
                        serde_json::from_str::<String>(&text[start..position])
                            .map_err(|_| RestoreError::Input)?,
                    );
                    let Some(Some(keys)) = objects.last_mut() else {
                        return Err(RestoreError::Input);
                    };
                    if !keys.insert(Sha256::digest(decoded.as_bytes()).into()) {
                        return Err(RestoreError::Input);
                    }
                    if decoded.contains("pv1-") {
                        return Err(RestoreError::UnknownAlias);
                    }
                } else {
                    values.push(start..position);
                }
            }
            _ => position += 1,
        }
    }
    Ok(values)
}

fn push_bounded(output: &mut String, text: &str) -> Result<()> {
    if output
        .len()
        .checked_add(text.len())
        .is_none_or(|n| n > MAX_TEXT)
    {
        return Err(RestoreError::Bounds);
    }
    output.push_str(text);
    Ok(())
}

fn token_boundary(text: &str, start: usize, end: usize, category: Category) -> bool {
    let previous = text[..start].chars().next_back();
    let next = text[end..].chars().next();
    match category {
        Category::Email => {
            let local = |c: char| word_continuation(c) || ".!#$%&'*+-/=?^_`{|}~@".contains(c);
            let domain = |c: char| word_continuation(c) || "-_@".contains(c);
            !previous.is_some_and(local)
                && !next.is_some_and(domain)
                && !(next == Some('.') && text[end + 1..].chars().next().is_some_and(domain))
        }
        Category::Telephone => {
            let attached = |c: char| word_continuation(c) || "+-_()".contains(c);
            let preceding_piece = matches!(previous, Some(' ' | '.'))
                && text[..start]
                    .chars()
                    .rev()
                    .nth(1)
                    .is_some_and(|c| c.is_ascii_digit() || "()+".contains(c));
            let following_piece = matches!(next, Some(' ' | '.'))
                && text[end..]
                    .chars()
                    .nth(1)
                    .is_some_and(|c| c.is_ascii_digit() || "()+".contains(c));
            !previous.is_some_and(attached)
                && !next.is_some_and(attached)
                && !preceding_piece
                && !following_piece
        }
        Category::Identifier(_) => {
            let attached = |c: char| word_continuation(c) || "-_".contains(c);
            !previous.is_some_and(attached) && !next.is_some_and(attached)
        }
    }
}

fn word_continuation(c: char) -> bool {
    // Use the existing pinned Unicode mark tables and all ten Unicode Pc
    // connector scalars. Connectors and format controls are attachments: do
    // not remove or normalize them to rescue an alias. The conservative
    // format ranges include joiners, bidi controls and tagged text.
    c.is_alphanumeric()
        || unicode_normalization::char::is_combining_mark(c)
        || matches!(c, '_' | '\u{203f}' | '\u{2040}' | '\u{2054}'
            | '\u{fe33}'..='\u{fe34}' | '\u{fe4d}'..='\u{fe4f}' | '\u{ff3f}')
        || (c.is_control() && !c.is_whitespace())
        || matches!(c,
            '\u{00ad}' | '\u{0600}'..='\u{0605}' | '\u{061c}' | '\u{06dd}'
            | '\u{070f}' | '\u{0890}'..='\u{0891}' | '\u{08e2}' | '\u{180e}'
            | '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{206f}' | '\u{feff}'
            | '\u{fff9}'..='\u{fffb}' | '\u{110bd}' | '\u{110cd}'
            | '\u{13430}'..='\u{13455}' | '\u{1bca0}'..='\u{1bca3}'
            | '\u{1d173}'..='\u{1d17a}' | '\u{e0001}' | '\u{e0020}'..='\u{e007f}')
}

pub struct DisplayView<'a> {
    text: Zeroizing<String>,
    session: &'a MappingSession,
    records: &'a ResolutionRecords,
}

pub struct FileDataView<'a> {
    text: Zeroizing<String>,
    session: &'a MappingSession,
    records: &'a ResolutionRecords,
}

impl DisplayView<'_> {
    pub fn with_host_display_text<T>(&self, read: impl FnOnce(&str) -> T) -> Result<T> {
        access(self.session, self.records, &self.text, read)
    }
}

impl FileDataView<'_> {
    pub fn with_host_candidate_content<T>(&self, read: impl FnOnce(&str) -> T) -> Result<T> {
        access(self.session, self.records, &self.text, read)
    }
}

fn access<T>(
    session: &MappingSession,
    records: &ResolutionRecords,
    text: &str,
    read: impl FnOnce(&str) -> T,
) -> Result<T> {
    records
        .validate_session_binding(session)
        .map_err(|_: PrivacyError| RestoreError::Scope)?;
    let result = read(text);
    records
        .validate_session_binding(session)
        .map_err(|_: PrivacyError| RestoreError::Scope)?;
    Ok(result)
}

impl std::fmt::Debug for Restorer<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Restorer([private])")
    }
}
impl std::fmt::Debug for DisplayView<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DisplayView([private])")
    }
}
impl std::fmt::Debug for FileDataView<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FileDataView([private])")
    }
}
