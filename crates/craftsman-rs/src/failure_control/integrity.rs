//! Bounded structural notes for an existing protected-content review.
//!
//! The caller supplies borrowed bytes from its retained snapshot authority.
//! These notes are not a semantic verdict, an approval grant or evidence that
//! tests ran. No-note and unavailable analyses leave the original review intact.

const MAX_PATH_BYTES: usize = 4096;
const MAX_FILE_BYTES: usize = 64 * 1024;
const MAX_TOKENS: usize = 16 * 1024;
const MAX_RAW_DELIMITER: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Reason {
    PathLimit,
    FileLimit,
    TokenLimit,
    NonUtf8,
    UnsupportedLanguage,
    UnterminatedString,
    UnterminatedComment,
    FormattedString,
    TemplateInterpolation,
    RegexOrDivision,
    UnrecognisedEscape,
    RawDelimiterLimit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Coverage {
    /// Recognised lexical structures only, never complete semantic coverage.
    StructuralOnly,
    Limited(Reason),
    Unavailable(Reason),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReviewNote {
    ProtectedFileRemoved,
    SkipDirectivesAdded { count: u32 },
    FocusDirectivesAdded { count: u32 },
    AssertionSitesReduced { before: u32, after: u32 },
    TestSitesReduced { before: u32, after: u32 },
    LiteralFalseGuardsAdded { count: u32 },
}

pub(crate) struct Analysis {
    notes: Vec<ReviewNote>,
    coverage: Coverage,
}

impl Analysis {
    pub(crate) fn notes(&self) -> &[ReviewNote] {
        &self.notes
    }

    pub(crate) fn coverage(&self) -> Coverage {
        self.coverage
    }

    /// Fixed note text and bounded counts only. Do not use this text as a
    /// decision or parse it back into evidence/paths/approval authority.
    pub(crate) fn render_notes(&self) -> String {
        let mut output =
            String::from("Structural protected-test review notes (signals, not a verdict):\n");
        for note in &self.notes {
            let text = match *note {
                ReviewNote::ProtectedFileRemoved => "- A protected file was removed.\n".into(),
                ReviewNote::SkipDirectivesAdded { count } => {
                    format!("- Recognisable skip/ignore directives increased by {count}.\n")
                }
                ReviewNote::FocusDirectivesAdded { count } => {
                    format!("- Recognisable focused-test directives increased by {count}.\n")
                }
                ReviewNote::AssertionSitesReduced { before, after } => {
                    format!("- Recognisable assertion sites decreased from {before} to {after}.\n")
                }
                ReviewNote::TestSitesReduced { before, after } => {
                    format!("- Recognisable test sites decreased from {before} to {after}.\n")
                }
                ReviewNote::LiteralFalseGuardsAdded { count } => {
                    format!("- Literal-false branch guards increased by {count}.\n")
                }
            };
            output.push_str(&text);
        }
        if self.notes.is_empty() {
            output.push_str("- No recognised structural changes were flagged. This does not establish test integrity.\n");
        }
        match self.coverage {
            Coverage::StructuralOnly => output.push_str(
                "Analysis covers recognised lexical structures; aliases, dynamic discovery, macros and execution reachability need human review.\n",
            ),
            Coverage::Limited(reason) => {
                output.push_str("Analysis is limited: ");
                output.push_str(reason_text(reason));
                output.push('\n');
            }
            Coverage::Unavailable(reason) => {
                output.push_str("Structural analysis is unavailable: ");
                output.push_str(reason_text(reason));
                output.push('\n');
            }
        }
        output.push_str("Review the original before/after contents. These notes do not replace content-bound approval, verification or independent grading.\n");
        output
    }
}

fn reason_text(reason: Reason) -> &'static str {
    match reason {
        Reason::PathLimit => "the selected path exceeds the analysis bound.",
        Reason::FileLimit => "selected content exceeds the analysis byte bound.",
        Reason::TokenLimit => "selected content exceeds the analysis token bound.",
        Reason::NonUtf8 => "selected content is not UTF-8 text.",
        Reason::UnsupportedLanguage => "the file language is unsupported.",
        Reason::UnterminatedString => "a string delimiter could not be resolved.",
        Reason::UnterminatedComment => "a comment delimiter could not be resolved.",
        Reason::FormattedString => "formatted-string expressions are not analysed.",
        Reason::TemplateInterpolation => "template interpolation has ambiguous executable content.",
        Reason::RegexOrDivision => "slash syntax may be a regular expression or division.",
        Reason::UnrecognisedEscape => "an escape outside a supported literal is ambiguous.",
        Reason::RawDelimiterLimit => "a raw-string delimiter exceeds the analysis bound.",
    }
}

#[derive(Clone, Copy)]
enum Language {
    Py,
    JavaScript,
    Rust,
}

impl Language {
    fn from_path(path: &str) -> Option<Self> {
        let extension = std::path::Path::new(path).extension()?.to_str()?;
        if extension.eq_ignore_ascii_case("py") {
            Some(Self::Py)
        } else if ["js", "cjs", "mjs", "jsx", "ts", "tsx"]
            .iter()
            .any(|candidate| extension.eq_ignore_ascii_case(candidate))
        {
            Some(Self::JavaScript)
        } else if extension.eq_ignore_ascii_case("rs") {
            Some(Self::Rust)
        } else {
            None
        }
    }
}

/// `before`/`after` must come from the same original bounded protected snapshot
/// pair. The path is used only to choose supported lexical handling, never IO.
pub(crate) fn analyze(path: &str, before: Option<&[u8]>, after: Option<&[u8]>) -> Analysis {
    let removed = before.is_some() && after.is_none();
    let mut notes = if removed {
        vec![ReviewNote::ProtectedFileRemoved]
    } else {
        Vec::new()
    };
    let unavailable = if path.len() > MAX_PATH_BYTES {
        Some(Reason::PathLimit)
    } else if before.is_some_and(|bytes| bytes.len() > MAX_FILE_BYTES)
        || after.is_some_and(|bytes| bytes.len() > MAX_FILE_BYTES)
    {
        Some(Reason::FileLimit)
    } else {
        None
    };
    if let Some(reason) = unavailable {
        return Analysis {
            notes,
            coverage: Coverage::Unavailable(reason),
        };
    }
    let Some(language) = Language::from_path(path) else {
        return Analysis {
            notes,
            coverage: Coverage::Limited(Reason::UnsupportedLanguage),
        };
    };
    let (Ok(before), Ok(after)) = (source_text(before), source_text(after)) else {
        return Analysis {
            notes,
            coverage: Coverage::Unavailable(Reason::NonUtf8),
        };
    };
    let old = match lex(before, language) {
        Ok(old) => old,
        Err(reason) => {
            return Analysis {
                notes,
                coverage: Coverage::Unavailable(reason),
            };
        }
    };
    let new = match lex(after, language) {
        Ok(new) => new,
        Err(reason) => {
            return Analysis {
                notes,
                coverage: Coverage::Unavailable(reason),
            };
        }
    };
    let old = counts(&old.tokens, language);
    let new = counts(&new.tokens, language);
    if new.skips > old.skips {
        notes.push(ReviewNote::SkipDirectivesAdded {
            count: new.skips - old.skips,
        });
    }
    if new.focus > old.focus {
        notes.push(ReviewNote::FocusDirectivesAdded {
            count: new.focus - old.focus,
        });
    }
    if new.assertions < old.assertions {
        notes.push(ReviewNote::AssertionSitesReduced {
            before: old.assertions,
            after: new.assertions,
        });
    }
    if new.tests < old.tests {
        notes.push(ReviewNote::TestSitesReduced {
            before: old.tests,
            after: new.tests,
        });
    }
    if new.false_guards > old.false_guards {
        notes.push(ReviewNote::LiteralFalseGuardsAdded {
            count: new.false_guards - old.false_guards,
        });
    }
    Analysis {
        notes,
        coverage: Coverage::StructuralOnly,
    }
}

fn source_text(bytes: Option<&[u8]>) -> Result<&str, std::str::Utf8Error> {
    bytes
        .map(std::str::from_utf8)
        .transpose()
        .map(|text| text.unwrap_or(""))
}

#[derive(Clone, Copy)]
enum Token<'a> {
    Word(&'a str),
    Number(&'a str),
    Punct(u8),
    Literal,
}

struct Lexed<'a> {
    tokens: Vec<Token<'a>>,
}

fn lex(source: &str, language: Language) -> Result<Lexed<'_>, Reason> {
    let bytes = source.as_bytes();
    let mut position = 0;
    let mut tokens = Vec::new();
    while position < bytes.len() {
        let current = bytes[position];
        if current.is_ascii_whitespace() {
            position += 1;
            continue;
        }
        if matches!(language, Language::Py) && current == b'#' {
            position = line_end(bytes, position);
            continue;
        }
        if !matches!(language, Language::Py) && bytes[position..].starts_with(b"//") {
            position = line_end(bytes, position);
            continue;
        }
        if !matches!(language, Language::Py) && bytes[position..].starts_with(b"/*") {
            position = block_comment(bytes, position, matches!(language, Language::Rust))?;
            continue;
        }
        if matches!(language, Language::JavaScript) && current == b'/' {
            return Err(Reason::RegexOrDivision);
        }
        if matches!(language, Language::JavaScript) && current == b'`' {
            position = template_literal(bytes, position)?;
            tokens.push(Token::Literal);
        } else if matches!(language, Language::Rust) && rust_raw_start(bytes, position).is_some() {
            let (quote, hashes) = rust_raw_start(bytes, position).unwrap();
            position = rust_raw_literal(bytes, quote, hashes)?;
            tokens.push(Token::Literal);
        } else if current == b'"' || current == b'\'' {
            if matches!(language, Language::Rust) && current == b'\'' {
                if let Some(end) = rust_character(source, position) {
                    position = end;
                    tokens.push(Token::Literal);
                } else {
                    position += 1;
                    tokens.push(Token::Punct(b'\''));
                }
            } else {
                position = quoted_literal(bytes, position, matches!(language, Language::Py))?;
                tokens.push(Token::Literal);
            }
        } else if current == b'\\' {
            if matches!(language, Language::Py)
                && bytes
                    .get(position + 1)
                    .is_some_and(|next| *next == b'\n' || *next == b'\r')
            {
                position += 2;
                continue;
            }
            return Err(Reason::UnrecognisedEscape);
        } else if current.is_ascii_digit() {
            let start = position;
            position += 1;
            while bytes
                .get(position)
                .is_some_and(|next| next.is_ascii_alphanumeric() || matches!(*next, b'.' | b'_'))
            {
                position += 1;
            }
            tokens.push(Token::Number(&source[start..position]));
        } else if identifier_start(source[position..].chars().next().unwrap(), language) {
            let start = position;
            while position < bytes.len() {
                let character = source[position..].chars().next().unwrap();
                if !identifier_continue(character, language) {
                    break;
                }
                position += character.len_utf8();
            }
            let word = &source[start..position];
            if matches!(language, Language::Py)
                && python_string_prefix(word)
                && bytes
                    .get(position)
                    .is_some_and(|next| matches!(*next, b'\'' | b'"'))
            {
                if word.bytes().any(|byte| byte == b'f' || byte == b'F') {
                    return Err(Reason::FormattedString);
                }
                position = quoted_literal(bytes, position, true)?;
                tokens.push(Token::Literal);
            } else {
                tokens.push(Token::Word(word));
            }
        } else {
            position += 1;
            tokens.push(Token::Punct(current));
        }
        if tokens.len() > MAX_TOKENS {
            return Err(Reason::TokenLimit);
        }
    }
    Ok(Lexed { tokens })
}

fn identifier_start(character: char, language: Language) -> bool {
    character.is_ascii_alphabetic()
        || character == '_'
        || character >= '\u{80}'
        || matches!(language, Language::JavaScript) && character == '$'
}

fn identifier_continue(character: char, language: Language) -> bool {
    identifier_start(character, language) || character.is_ascii_digit()
}

fn python_string_prefix(word: &str) -> bool {
    ["r", "u", "b", "f", "br", "rb", "fr", "rf"]
        .iter()
        .any(|prefix| word.eq_ignore_ascii_case(prefix))
}

fn line_end(bytes: &[u8], start: usize) -> usize {
    bytes[start..]
        .iter()
        .position(|byte| *byte == b'\n')
        .map_or(bytes.len(), |offset| start + offset + 1)
}

fn block_comment(bytes: &[u8], start: usize, nested: bool) -> Result<usize, Reason> {
    let mut position = start + 2;
    let mut depth = 1usize;
    while position < bytes.len() {
        if bytes[position..].starts_with(b"*/") {
            depth -= 1;
            position += 2;
            if depth == 0 {
                return Ok(position);
            }
        } else if nested && bytes[position..].starts_with(b"/*") {
            depth += 1;
            position += 2;
        } else {
            position += 1;
        }
    }
    Err(Reason::UnterminatedComment)
}

fn quoted_literal(bytes: &[u8], start: usize, triple_allowed: bool) -> Result<usize, Reason> {
    let quote = bytes[start];
    let triple = triple_allowed
        && bytes
            .get(start..start + 3)
            .is_some_and(|part| part == [quote; 3]);
    let delimiter = if triple { 3 } else { 1 };
    let mut position = start + delimiter;
    while position < bytes.len() {
        if bytes[position] == b'\\' {
            position = position.saturating_add(2);
        } else if triple
            && bytes
                .get(position..position + 3)
                .is_some_and(|part| part == [quote; 3])
        {
            return Ok(position + 3);
        } else if !triple && bytes[position] == quote {
            return Ok(position + 1);
        } else if !triple && matches!(bytes[position], b'\n' | b'\r') {
            return Err(Reason::UnterminatedString);
        } else {
            position += 1;
        }
    }
    Err(Reason::UnterminatedString)
}

fn template_literal(bytes: &[u8], start: usize) -> Result<usize, Reason> {
    let mut position = start + 1;
    while position < bytes.len() {
        if bytes[position] == b'\\' {
            position = position.saturating_add(2);
        } else if bytes[position..].starts_with(b"${") {
            return Err(Reason::TemplateInterpolation);
        } else if bytes[position] == b'`' {
            return Ok(position + 1);
        } else {
            position += 1;
        }
    }
    Err(Reason::UnterminatedString)
}

fn rust_raw_start(bytes: &[u8], start: usize) -> Option<(usize, usize)> {
    let prefix = if bytes[start..].starts_with(b"br") || bytes[start..].starts_with(b"cr") {
        2
    } else if bytes[start..].starts_with(b"r") {
        1
    } else {
        return None;
    };
    let mut quote = start + prefix;
    while bytes.get(quote) == Some(&b'#') {
        quote += 1;
    }
    (bytes.get(quote) == Some(&b'"')).then_some((quote, quote - start - prefix))
}

fn rust_raw_literal(bytes: &[u8], quote: usize, hashes: usize) -> Result<usize, Reason> {
    if hashes > MAX_RAW_DELIMITER {
        return Err(Reason::RawDelimiterLimit);
    }
    let mut position = quote + 1;
    while position < bytes.len() {
        if bytes[position] == b'"'
            && bytes
                .get(position + 1..position + 1 + hashes)
                .is_some_and(|suffix| suffix.iter().all(|byte| *byte == b'#'))
        {
            return Ok(position + 1 + hashes);
        }
        position += 1;
    }
    Err(Reason::UnterminatedString)
}

fn rust_character(source: &str, start: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    let next = *bytes.get(start + 1)?;
    let end = if next != b'\\' {
        start + 1 + source[start + 1..].chars().next()?.len_utf8()
    } else {
        match *bytes.get(start + 2)? {
            b'x' => start + 5,
            b'u' if bytes.get(start + 3) == Some(&b'{') => {
                let offset = bytes
                    .get(start + 4..)?
                    .iter()
                    .take(32)
                    .position(|byte| *byte == b'}')?;
                start + 5 + offset
            }
            _ => start + 3,
        }
    };
    (bytes.get(end) == Some(&b'\'')).then_some(end + 1)
}

#[derive(Default)]
struct Counts {
    assertions: u32,
    tests: u32,
    skips: u32,
    focus: u32,
    false_guards: u32,
}

fn word(tokens: &[Token<'_>], index: usize, expected: &str) -> bool {
    matches!(tokens.get(index), Some(Token::Word(value)) if *value == expected)
}

fn punctuation(tokens: &[Token<'_>], index: usize, expected: u8) -> bool {
    matches!(tokens.get(index), Some(Token::Punct(value)) if *value == expected)
}

fn number_zero(tokens: &[Token<'_>], index: usize) -> bool {
    matches!(tokens.get(index), Some(Token::Number("0" | "0.0")))
}

fn attribute_name<'a>(tokens: &[Token<'a>], start: usize) -> Option<&'a str> {
    if !punctuation(tokens, start, b'#') || !punctuation(tokens, start + 1, b'[') {
        return None;
    }
    let mut index = start + 2;
    let mut last = None;
    while let Some(token) = tokens.get(index) {
        match token {
            Token::Word(value) => last = Some(*value),
            Token::Punct(b':') => {}
            _ => return last,
        }
        index += 1;
    }
    None
}

fn counts(tokens: &[Token<'_>], language: Language) -> Counts {
    let mut result = Counts::default();
    for index in 0..tokens.len() {
        let Some(Token::Word(value)) = tokens.get(index) else {
            if matches!(language, Language::Rust) {
                match attribute_name(tokens, index) {
                    Some("test") => result.tests += 1,
                    Some("ignore") => result.skips += 1,
                    _ => {}
                }
            }
            continue;
        };
        match language {
            Language::Py => {
                if *value == "assert"
                    || punctuation(tokens, index + 1, b'(')
                        && index > 0
                        && punctuation(tokens, index - 1, b'.')
                        && [
                            "assertTrue",
                            "assertFalse",
                            "assertEqual",
                            "assertNotEqual",
                            "assertIs",
                            "assertIsNot",
                            "assertIsNone",
                            "assertIsNotNone",
                            "assertIn",
                            "assertNotIn",
                            "assertRaises",
                            "assertAlmostEqual",
                            "assertGreater",
                            "assertLess",
                        ]
                        .contains(value)
                {
                    result.assertions += 1;
                }
                if *value == "def"
                    && matches!(tokens.get(index + 1), Some(Token::Word(name)) if name.starts_with("test_"))
                    && punctuation(tokens, index + 2, b'(')
                {
                    result.tests += 1;
                }
                if *value == "pytest"
                    && punctuation(tokens, index + 1, b'.')
                    && word(tokens, index + 2, "mark")
                    && punctuation(tokens, index + 3, b'.')
                    && (word(tokens, index + 4, "skip") || word(tokens, index + 4, "skipif"))
                    || *value == "unittest"
                        && punctuation(tokens, index + 1, b'.')
                        && ["skip", "skipIf", "skipUnless"]
                            .iter()
                            .any(|name| word(tokens, index + 2, name))
                {
                    result.skips += 1;
                }
                if ["if", "elif"].contains(value)
                    && (word(tokens, index + 1, "False") || number_zero(tokens, index + 1))
                    && punctuation(tokens, index + 2, b':')
                {
                    result.false_guards += 1;
                }
            }
            Language::JavaScript => {
                let definition = index > 0 && word(tokens, index - 1, "function");
                if !definition
                    && (*value == "expect" && punctuation(tokens, index + 1, b'(')
                        || *value == "assert"
                            && (punctuation(tokens, index + 1, b'(')
                                || punctuation(tokens, index + 1, b'.')
                                    && [
                                        "ok",
                                        "equal",
                                        "notEqual",
                                        "strictEqual",
                                        "notStrictEqual",
                                        "deepEqual",
                                        "deepStrictEqual",
                                        "notDeepEqual",
                                        "notDeepStrictEqual",
                                        "throws",
                                        "rejects",
                                        "match",
                                        "doesNotThrow",
                                    ]
                                    .iter()
                                    .any(|name| word(tokens, index + 2, name))
                                    && punctuation(tokens, index + 3, b'(')))
                {
                    result.assertions += 1;
                }
                let test_name = ["test", "it", "describe", "suite", "context"].contains(value)
                    && !definition
                    && (index == 0 || !punctuation(tokens, index - 1, b'.'));
                if test_name {
                    if punctuation(tokens, index + 1, b'(') {
                        result.tests += u32::from(["test", "it"].contains(value));
                    } else if punctuation(tokens, index + 1, b'.')
                        && punctuation(tokens, index + 3, b'(')
                    {
                        if word(tokens, index + 2, "skip") {
                            result.skips += 1;
                            result.tests += u32::from(["test", "it"].contains(value));
                        } else if word(tokens, index + 2, "only") {
                            result.focus += 1;
                            result.tests += u32::from(["test", "it"].contains(value));
                        }
                    }
                }
                if *value == "if"
                    && punctuation(tokens, index + 1, b'(')
                    && (word(tokens, index + 2, "false") || number_zero(tokens, index + 2))
                    && punctuation(tokens, index + 3, b')')
                {
                    result.false_guards += 1;
                }
            }
            Language::Rust => {
                if [
                    "assert",
                    "assert_eq",
                    "assert_ne",
                    "debug_assert",
                    "debug_assert_eq",
                    "debug_assert_ne",
                ]
                .contains(value)
                    && punctuation(tokens, index + 1, b'!')
                {
                    result.assertions += 1;
                }
                if *value == "if"
                    && word(tokens, index + 1, "false")
                    && punctuation(tokens, index + 2, b'{')
                {
                    result.false_guards += 1;
                }
            }
        }
    }
    result
}
