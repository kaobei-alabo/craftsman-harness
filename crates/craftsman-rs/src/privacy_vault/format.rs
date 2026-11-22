//! Exact byte/range admission and declared format pseudonyms, not a detector.

use super::crypto::Keys;
use super::storage::Budget;
use super::{Category, DeclaredIdentifiers, DetectorReport, PrivacyError, Result};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

pub(super) const ALIAS_ATTEMPTS: u32 = 32;

pub(super) struct Selected {
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) category: Category,
    pub(super) value: Zeroizing<String>,
}

pub(super) fn validate_spans(
    text: &str,
    captured_digest: &[u8; 32],
    mut report: DetectorReport,
    declared: &DeclaredIdentifiers,
    budget: &mut Budget,
) -> Result<Vec<Selected>> {
    budget.work(text.len())?;
    if text.len() > budget.limits.text_bytes || report.spans.len() > budget.limits.spans {
        return Err(PrivacyError::Bounds);
    }
    let actual: [u8; 32] = Sha256::digest(text.as_bytes()).into();
    if report.input_sha256 != *captured_digest || actual != *captured_digest {
        return Err(PrivacyError::Input);
    }
    report
        .spans
        .sort_unstable_by_key(|span| (span.start, span.end));
    let mut selected: Vec<Selected> = Vec::with_capacity(report.spans.len());
    for span in report.spans {
        if span.start >= span.end
            || span.end > text.len()
            || !text.is_char_boundary(span.start)
            || !text.is_char_boundary(span.end)
            || selected.last().is_some_and(|prior| prior.end > span.start)
        {
            return Err(PrivacyError::Input);
        }
        let value = &text[span.start..span.end];
        budget.work(value.len())?;
        validate_original(span.category, value)?;
        if let Category::Identifier(kind) = span.category
            && !declared.owns(kind, value)
        {
            return Err(PrivacyError::Input);
        }
        for prior in &selected {
            budget.work(prior.value.len() + value.len())?;
            if prior.value.as_str() == value && prior.category != span.category {
                return Err(PrivacyError::Input);
            }
        }
        selected.push(Selected {
            start: span.start,
            end: span.end,
            category: span.category,
            value: Zeroizing::new(value.to_owned()),
        });
    }
    // Do not knowingly publish another exact occurrence of an admitted original
    // in unchanged text. This does not claim coverage of unknown/encoded PII.
    for (index, value) in selected.iter().enumerate() {
        if selected[..index].iter().any(|p| p.value == value.value) {
            continue;
        }
        budget.work(text.len() + value.value.len())?;
        for (start, occurrence) in text.match_indices(value.value.as_str()) {
            let end = start + occurrence.len();
            if !selected
                .iter()
                .any(|span| span.start <= start && span.end >= end)
            {
                return Err(PrivacyError::Input);
            }
        }
    }
    Ok(selected)
}

pub(super) fn validate_identifier(value: &str) -> Result<()> {
    if value.len() < 4 || value.len() > 256 || value.chars().any(char::is_control) {
        return Err(PrivacyError::Unsupported);
    }
    Ok(())
}

pub(super) fn validate_original(category: Category, value: &str) -> Result<()> {
    match category {
        Category::Email => validate_email(value),
        Category::Telephone => validate_phone(value),
        Category::Identifier(_) => validate_identifier(value),
    }
}

fn validate_email(value: &str) -> Result<()> {
    if !value.is_ascii() || value.len() > 254 {
        return Err(PrivacyError::Unsupported);
    }
    let Some((local, domain)) = value.split_once('@') else {
        return Err(PrivacyError::Unsupported);
    };
    if local.is_empty()
        || local.len() > 64
        || local.starts_with('.')
        || local.ends_with('.')
        || local.contains("..")
        || !local
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".!#$%&'*+-/=?^_`{|}~".contains(&b))
        || domain.is_empty()
        || domain.len() > 253
        || !domain.contains('.')
    {
        return Err(PrivacyError::Unsupported);
    }
    for label in domain.split('.') {
        if label.is_empty()
            || label.len() > 63
            || label.starts_with('-')
            || label.ends_with('-')
            || !label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return Err(PrivacyError::Unsupported);
        }
    }
    Ok(())
}

fn validate_phone(value: &str) -> Result<()> {
    let bytes = value.as_bytes();
    let digits = bytes.iter().filter(|b| b.is_ascii_digit()).count();
    if !value.is_ascii() || value.len() > 32 || !(7..=15).contains(&digits) {
        return Err(PrivacyError::Unsupported);
    }
    let mut group = None;
    let mut had_group = false;
    for (index, &byte) in bytes.iter().enumerate() {
        let previous = index.checked_sub(1).map(|i| bytes[i]);
        let next = bytes.get(index + 1).copied();
        match byte {
            b'0'..=b'9' => {}
            b'+' if index == 0 && next.is_some_and(|b| b.is_ascii_digit()) => {}
            b'(' if !had_group
                && (index == 0 || previous == Some(b' '))
                && next.is_some_and(|b| b.is_ascii_digit()) =>
            {
                had_group = true;
                group = Some(index);
            }
            b')' if group.is_some() && previous.is_some_and(|b| b.is_ascii_digit()) => {
                let start = group.take().ok_or(PrivacyError::Unsupported)?;
                if !(1..=5).contains(&(index - start - 1))
                    || !bytes[start + 1..index].iter().all(u8::is_ascii_digit)
                    || next.is_some_and(|b| !b" -.".contains(&b))
                {
                    return Err(PrivacyError::Unsupported);
                }
            }
            b' ' | b'-' | b'.'
                if group.is_none()
                    && previous.is_some_and(|b| b.is_ascii_digit() || b == b')')
                    && next.is_some_and(|b| b.is_ascii_digit() || (byte == b' ' && b == b'(')) => {}
            _ => return Err(PrivacyError::Unsupported),
        }
    }
    if group.is_some()
        || !bytes
            .last()
            .is_some_and(|b| b.is_ascii_digit() || *b == b')')
    {
        return Err(PrivacyError::Unsupported);
    }
    Ok(())
}

pub(super) fn generate(
    keys: &Keys,
    scope: &[u8; 32],
    category: Category,
    value: &str,
    counter: u32,
) -> String {
    let identity = keys.identity(scope, category, value);
    let entropy = keys.alias_entropy(scope, &identity, counter);
    let suffix = || {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut result = String::with_capacity(32);
        for byte in &entropy[..16] {
            result.push(HEX[(byte >> 4) as usize] as char);
            result.push(HEX[(byte & 15) as usize] as char);
        }
        result
    };
    match category {
        Category::Email => format!("pv1-{}@privacy.invalid", suffix()),
        Category::Telephone => {
            let mut digit = 0;
            value
                .bytes()
                .map(|byte| {
                    if byte.is_ascii_digit() {
                        let alias = b'0' + entropy[digit] % 10;
                        digit += 1;
                        alias as char
                    } else {
                        byte as char
                    }
                })
                .collect()
        }
        Category::Identifier(kind) => {
            let prefix = match kind {
                super::IdentifierKind::Customer => 'c',
                super::IdentifierKind::Account => 'a',
                super::IdentifierKind::Project => 'p',
            };
            format!("pv1-{prefix}-{}", suffix())
        }
    }
}
