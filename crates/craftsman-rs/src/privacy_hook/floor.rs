//! A narrow host floor for the two supported categories.
//!
//! These are the reviewed host rules of the detector fixture, moved into
//! production unchanged in meaning. They find email addresses and telephone
//! numbers in a few fixed layouts. The host uses them for one purpose: to
//! refuse a detector reply that leaves one of these matches unreported.
//!
//! This is not a detector and not a coverage claim. It knows nothing about
//! names, addresses, other identifiers, encoded values or secrets, and a
//! value it does not match can still be private.

use crate::privacy_vault::Category;

fn mailbox(value: &str) -> bool {
    let parts: Vec<_> = value.split('@').collect();
    if parts.len() != 2 || value.len() > 254 {
        return false;
    }
    let local = parts[0];
    let domain = parts[1];
    if !(1..=64).contains(&local.len())
        || local.starts_with('.')
        || local.ends_with('.')
        || local.contains("..")
        || local
            .chars()
            .any(|c| !c.is_ascii_alphanumeric() && !"._%+-".contains(c))
        || domain.len() > 253
    {
        return false;
    }
    let labels: Vec<_> = domain.split('.').collect();
    if labels.len() < 2 {
        return false;
    }
    for label in &labels {
        if !(1..=63).contains(&label.len())
            || !label.starts_with(|c: char| c.is_ascii_alphanumeric())
            || !label.ends_with(|c: char| c.is_ascii_alphanumeric())
            || label
                .chars()
                .any(|c| !c.is_ascii_alphanumeric() && c != '-')
        {
            return false;
        }
    }
    let last = labels.last().unwrap();
    last.len() >= 2 && last.chars().all(|c| c.is_ascii_alphabetic())
}

fn telephone(value: &str) -> bool {
    // Contiguous international layout is not proof of a valid assigned number.
    if let Some(digits) = value.strip_prefix('+') {
        if (8..=15).contains(&digits.len())
            && !digits.starts_with('0')
            && digits.bytes().all(|b| b.is_ascii_digit())
        {
            return true;
        }
    }
    let layout: String = value
        .chars()
        .map(|c| if c.is_ascii_digit() { 'D' } else { c })
        .collect();
    let digits: Vec<_> = value.bytes().filter(u8::is_ascii_digit).collect();
    let nanp_offset = match layout.as_str() {
        "+D (DDD) DDD-DDDD" | "+D DDD-DDD-DDDD" | "+D DDD DDD DDDD" if value.starts_with("+1 ") => {
            Some(1)
        }
        "(DDD) DDD-DDDD" | "DDD-DDD-DDDD" | "DDD.DDD.DDDD" => Some(0),
        _ => None,
    };
    if let Some(offset) = nanp_offset {
        return (b'2'..=b'9').contains(&digits[offset])
            && (b'2'..=b'9').contains(&digits[offset + 3]);
    }
    match layout.as_str() {
        "+DD DD DDDD DDDD" => value.starts_with("+44 20 "),
        "+DDD DDD DDD DDD" => value.starts_with("+886 9"),
        "+DDD-D-DDDD-DDDD" => value.starts_with("+886-") && (b'2'..=b'8').contains(&digits[3]),
        "DDDD-DDD-DDD" | "DDDD DDD DDD" => value.starts_with("09"),
        "DD-DDDD-DDDD" => value.starts_with("02-"),
        _ => false,
    }
}

fn boundary(text: &str, start: usize, end: usize, is_phone: bool) -> bool {
    let excluded = |c: char| c.is_alphanumeric() || "_@+".contains(c);
    if text[..start].chars().last().is_some_and(excluded)
        || text[end..].chars().next().is_some_and(excluded)
    {
        return false;
    }
    if is_phone {
        let left: Vec<_> = text[..start].chars().rev().take(2).collect();
        let right: Vec<_> = text[end..].chars().take(2).collect();
        if (left.len() == 2 && "-.".contains(left[0]) && left[1].is_ascii_digit())
            || (right.len() == 2 && "-.".contains(right[0]) && right[1].is_ascii_digit())
        {
            return false;
        }
    }
    true
}

/// Every floor match in `text`, in order and without overlap, as byte ranges.
pub(super) fn matches(text: &str) -> Vec<(usize, usize, Category)> {
    let bytes = text.as_bytes();
    let mut spans = Vec::new();
    let mut accepted_end = 0;
    for (start, _) in text.char_indices() {
        if start < accepted_end {
            continue;
        }
        let address_char = |b: u8| b.is_ascii_alphanumeric() || b"._%+-@".contains(&b);
        if (start == 0 || !address_char(bytes[start - 1])) && address_char(bytes[start]) {
            let length = bytes[start..]
                .iter()
                .take_while(|b| address_char(**b))
                .count();
            let candidate = &text[start..start + length];
            let candidate = candidate.strip_suffix('.').unwrap_or(candidate);
            let end = start + candidate.len();
            if mailbox(candidate) && boundary(text, start, end, false) {
                spans.push((start, end, Category::Email));
                accepted_end = end;
                continue;
            }
        }
        // Every supported layout begins with '+', '(' or a digit.
        if !(bytes[start] == b'+' || bytes[start] == b'(' || bytes[start].is_ascii_digit()) {
            continue;
        }
        // Examine only possible complete layout lengths; no quadratic scan.
        let mut best = None;
        for length in 9..=17 {
            let Some(end) = start.checked_add(length) else {
                continue;
            };
            if end <= text.len()
                && text.is_char_boundary(end)
                && telephone(&text[start..end])
                && boundary(text, start, end, true)
            {
                best = Some(end);
            }
        }
        if let Some(end) = best {
            spans.push((start, end, Category::Telephone));
            accepted_end = end;
        }
    }
    spans
}
