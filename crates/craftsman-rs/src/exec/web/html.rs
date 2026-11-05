//! Private bounded incremental HTML-to-text view, not a DOM or sanitizer.
//! Input has already passed original-byte redaction/verification. Decoded/joined
//! text enters ANOTHER redacting/verifying publisher before its output cap.

use super::capture::Fault;
use crate::exec::redaction::publication::Output;
const TAG_LIMIT: usize = 4096;
const ENTITY_LIMIT: usize = 64;
const SPACE_LIMIT: usize = 4096;

enum State {
    Text,
    Tag { text: String, quote: Option<char> },
    Entity(String),
    Skip { close: &'static str, matched: usize },
}
pub(super) struct Html {
    state: State,
    whitespace: String,
    newlines: usize,
    content: bool,
}
impl Html {
    pub(super) fn new() -> Self {
        Self {
            state: State::Text,
            whitespace: String::new(),
            newlines: 0,
            content: false,
        }
    }
    pub(super) fn push(&mut self, text: &str, output: &mut Output) -> Result<bool, Fault> {
        for ch in text.chars() {
            if !self.character(ch, output)? {
                return Ok(false);
            }
        }
        Ok(true)
    }
    fn character(&mut self, ch: char, output: &mut Output) -> Result<bool, Fault> {
        let state = std::mem::replace(&mut self.state, State::Text);
        match state {
            State::Text => match ch {
                '<' => {
                    self.state = State::Tag {
                        text: "<".into(),
                        quote: None,
                    };
                    Ok(true)
                }
                '&' => {
                    self.state = State::Entity("&".into());
                    Ok(true)
                }
                _ => self.text_char(ch, output),
            },
            State::Tag {
                mut text,
                mut quote,
            } => {
                if text.len() + ch.len_utf8() > TAG_LIMIT {
                    return Err(Fault::Limit);
                }
                text.push(ch);
                if let Some(open) = quote {
                    if ch == open {
                        quote = None;
                    }
                } else if matches!(ch, '\'' | '"') {
                    quote = Some(ch);
                } else if ch == '>' {
                    return self.tag(&text, output);
                }
                self.state = State::Tag { text, quote };
                Ok(true)
            }
            State::Entity(mut entity) => {
                if entity.len() + ch.len_utf8() > ENTITY_LIMIT {
                    return Err(Fault::Limit);
                }
                if ch == ';' {
                    entity.push(ch);
                    if let Some(decoded) = decode(&entity)? {
                        // Do not reinterpret decoded angle brackets as tags.
                        return self.text_char(decoded, output);
                    }
                    return self.literal(&entity, output);
                }
                if ch.is_whitespace() || matches!(ch, '<' | '&') {
                    if !self.literal(&entity, output)? {
                        return Ok(false);
                    }
                    return self.character(ch, output);
                }
                entity.push(ch);
                self.state = State::Entity(entity);
                Ok(true)
            }
            State::Skip { close, mut matched } => {
                let byte = ch.to_ascii_lowercase();
                if byte.is_ascii() && byte as u8 == close.as_bytes()[matched] {
                    matched += 1;
                } else {
                    matched = usize::from(byte == '<');
                }
                if matched != close.len() {
                    self.state = State::Skip { close, matched };
                }
                Ok(true)
            }
        }
    }
    fn tag(&mut self, raw: &str, output: &mut Output) -> Result<bool, Fault> {
        let inside = raw
            .strip_prefix('<')
            .and_then(|s| s.strip_suffix('>'))
            .ok_or(Fault::Syntax)?
            .trim();
        let closing = inside.starts_with('/');
        let body = inside.strip_prefix('/').unwrap_or(inside).trim_start();
        let name = body
            .split(|c: char| c.is_whitespace() || c == '/')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        if !closing && !inside.ends_with('/') {
            match name.as_str() {
                "script" => {
                    self.state = State::Skip {
                        close: "</script>",
                        matched: 0,
                    };
                    return Ok(true);
                }
                "style" => {
                    self.state = State::Skip {
                        close: "</style>",
                        matched: 0,
                    };
                    return Ok(true);
                }
                _ => {}
            }
        }
        if name == "br"
            || (closing
                && (matches!(name.as_str(), "p" | "div" | "li" | "tr")
                    || matches!(name.as_str(), "h1" | "h2" | "h3" | "h4" | "h5" | "h6")))
        {
            return self.text_char('\n', output);
        }
        Ok(true)
    }
    fn literal(&mut self, text: &str, output: &mut Output) -> Result<bool, Fault> {
        for ch in text.chars() {
            if !self.text_char(ch, output)? {
                return Ok(false);
            }
        }
        Ok(true)
    }
    /// Bounded trailing-space/blank-line/global-trim state; never accumulate a
    /// whole line, script body or expanded document while deciding what to omit.
    fn text_char(&mut self, ch: char, output: &mut Output) -> Result<bool, Fault> {
        if ch == '\n' {
            self.whitespace.clear();
            if self.content {
                self.newlines = (self.newlines + 1).min(2);
            }
            return Ok(true);
        }
        if ch.is_whitespace() {
            if self.whitespace.len() + ch.len_utf8() > SPACE_LIMIT {
                return Err(Fault::Limit);
            }
            self.whitespace.push(ch);
            return Ok(true);
        }
        if self.content {
            for _ in 0..self.newlines {
                if !output.frame("\n")? {
                    return Ok(false);
                }
            }
            if !output.frame(&self.whitespace)? {
                return Ok(false);
            }
        }
        self.whitespace.clear();
        self.newlines = 0;
        self.content = true;
        let mut scalar = [0; 4];
        output
            .frame(ch.encode_utf8(&mut scalar))
            .map_err(Into::into)
    }
    pub(super) fn finish(&mut self, output: &mut Output) -> Result<bool, Fault> {
        match std::mem::replace(&mut self.state, State::Text) {
            State::Tag { .. } => return Err(Fault::Syntax),
            State::Entity(entity) => {
                if !self.literal(&entity, output)? {
                    return Ok(false);
                }
            }
            State::Text | State::Skip { .. } => {}
        }
        self.whitespace.clear();
        self.newlines = 0;
        Ok(true)
    }
    pub(super) fn abort(&mut self) {
        self.state = State::Text;
        self.whitespace.clear();
        self.newlines = 0;
    }
}
fn decode(entity: &str) -> Result<Option<char>, Fault> {
    let named = match entity {
        "&nbsp;" => Some(' '),
        "&amp;" => Some('&'),
        "&lt;" => Some('<'),
        "&gt;" => Some('>'),
        "&quot;" => Some('"'),
        "&apos;" => Some('\''),
        _ => None,
    };
    if named.is_some() {
        return Ok(named);
    }
    let Some(number) = entity.strip_prefix("&#").and_then(|s| s.strip_suffix(';')) else {
        return Ok(None);
    };
    let value = if let Some(hex) = number
        .strip_prefix('x')
        .or_else(|| number.strip_prefix('X'))
    {
        u32::from_str_radix(hex, 16)
    } else {
        number.parse()
    }
    .map_err(|_| Fault::Syntax)?;
    char::from_u32(value).map(Some).ok_or(Fault::Syntax)
}

#[cfg(test)]
mod tests;
