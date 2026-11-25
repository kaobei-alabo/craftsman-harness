//! Standalone cases for the JSON string index. Every document is a literal
//! in this file; nothing is read, written, sent or run.

use super::*;
use PathStep::{Index, Key};
use StringRole::{Key as KeyRole, Value as ValueRole};

fn ranges(document: &str) -> JsonRanges<'_> {
    JsonRanges::index(document.as_bytes()).expect("the document is admitted")
}

fn refused(document: &[u8]) -> JsonError {
    JsonRanges::index(document).expect_err("the document is refused")
}

/// Role, token as written, decoded text and path of every string.
type Listed<'a> = Vec<(StringRole, &'a str, &'a str, Vec<PathStep<'a>>)>;

fn listed<'a>(document: &'a str, ranges: &'a JsonRanges<'a>) -> Listed<'a> {
    let mut all = Vec::new();
    for (number, string) in ranges.strings().iter().enumerate() {
        assert_eq!(
            string.content(),
            string.token().start + 1..string.token().end - 1
        );
        all.push((
            string.role(),
            &document[string.token()],
            ranges.decoded(number).unwrap(),
            ranges.path(number).unwrap(),
        ));
    }
    all
}

/// What one located range says, in the order of `Within`'s accessors.
type Located = (
    usize,
    Range<usize>,
    Range<usize>,
    Range<usize>,
    Range<usize>,
    bool,
);

fn facts(within: &Within) -> Located {
    (
        within.string(),
        within.range(),
        within.before(),
        within.after(),
        within.decoded(),
        within.literal(),
    )
}

/// The range of the only occurrence of `part` in `document`.
fn only(document: &str, part: &str) -> Range<usize> {
    let start = document.find(part).expect("the part occurs");
    assert_eq!(document.rfind(part), Some(start), "{part} occurs once");
    start..start + part.len()
}

/// Documents that are admitted. None has a repeated key.
const ADMITTED: &[&str] = &[
    r#""x""#,
    " \t\r\n\"a\\u0062\" \n",
    "0",
    "-0",
    "-12.5e+3",
    "1E5",
    "0.0e-0",
    "123456789012345678901234567890",
    "1e-999",
    "true",
    "false",
    "null",
    "[]",
    "{}",
    " [ ] ",
    "[[],{}]",
    r#"{ "name" : "Ada" , "tags":["x","y\n",{"k":"v","n":[1,"deep"]}], "n":-1.5e3, "ok":true, "none":null, "empty":"" }"#,
    r#"{"a":{"a":1},"b":{"a":2}}"#,
    r#"[{"a":1},{"a":2}]"#,
    r#"{"a":1,"A":2,"a ":3}"#,
    "{\"\u{e9}\":1,\"e\\u0301\":2}",
    r#"["a\n\u00e9\ud83d\ude00\"\\\/\b\f\r\t\u0000\u007f"]"#,
    "[\"\u{e9}\u{1f600}\u{7f}\"]",
    r#"{"":""}"#,
];

/// Documents that are refused, with the fault and where it is found.
fn refusals() -> Vec<(&'static [u8], JsonFault, usize)> {
    use JsonFault::*;
    vec![
        // Nothing, or not one value.
        (b"", Structure, 0),
        (b" \n\t", Structure, 3),
        (b"{} x", Trailing, 3),
        (b"1 2", Trailing, 2),
        (b"\"a\"\"b\"", Trailing, 3),
        (b"[1]]", Trailing, 3),
        (b"null,", Trailing, 4),
        // Structure.
        (b"[1,]", Structure, 3),
        (b"{\"a\":1,}", Structure, 7),
        (b"{\"a\" 1}", Structure, 5),
        (b"{\"a\":}", Structure, 5),
        (b"{a:1}", Structure, 1),
        (b"{1:2}", Structure, 1),
        (b"[1 2]", Structure, 3),
        (b"{\"a\":1 \"b\":2}", Structure, 7),
        (b"[", Structure, 1),
        (b"{\"a\":[1", Structure, 7),
        (b"\"abc", Structure, 4),
        (b"\"abc\\", Structure, 5),
        (b"\"\\u12", Structure, 5),
        (b"tru", Structure, 0),
        (b"True", Structure, 0),
        (b"nulls", Structure, 0),
        (b"NaN", Structure, 0),
        (b"'a'", Structure, 0),
        (b"// c\n1", Structure, 0),
        (b"[1,/*c*/2]", Structure, 3),
        (b".5", Structure, 0),
        (b"+1", Structure, 0),
        (b"\xef\xbb\xbf{}", Structure, 0),
        (b"{\"a\":\xc3\xa9}", Structure, 5),
        // Numbers.
        (b"01", Number, 0),
        (b"-", Number, 0),
        (b"[-]", Number, 1),
        (b"1.", Number, 0),
        (b"1e", Number, 0),
        (b"1e+", Number, 0),
        (b"0x10", Number, 0),
        (b"1.2.3", Number, 0),
        (b"[1-2]", Number, 1),
        (b"-a", Number, 0),
        (b"1e999", NumberRange, 0),
        (b"[-1e400]", NumberRange, 1),
        // Escapes.
        (b"\"\\x\"", Escape, 1),
        (b"\"\\u12G4\"", Escape, 1),
        (b"\"\\u12\"", Escape, 1),
        (b"\"\\U0041\"", Escape, 1),
        (b"\"a\\'b\"", Escape, 2),
        (b"\"\\ud83d\\uZZZZ\"", Escape, 7),
        // Surrogates.
        (b"\"\\ud83d\"", Surrogate, 1),
        (b"\"\\ude00\"", Surrogate, 1),
        (b"\"\\ud83dx\"", Surrogate, 1),
        (b"\"\\ud83d\\u0041\"", Surrogate, 1),
        (b"\"\\ude00\\ud83d\"", Surrogate, 1),
        (b"\"\\ud83d\\n\"", Surrogate, 1),
        // Control characters as they are.
        (b"\"a\nb\"", Control, 2),
        (b"\"\x00\"", Control, 1),
        (b"\"\t\"", Control, 1),
        (b"[\"\x1f\"]", Control, 2),
        // Bytes that are not UTF-8.
        (b"\"\xff\"", Utf8, 1),
        (b"\"\xc3\"", Utf8, 1),
        (b"\"\xed\xa0\x80\"", Utf8, 1),
        (b"\"\xc0\xaf\"", Utf8, 1),
        (b"[1,\xe2\x82]", Utf8, 3),
        // A repeated key.
        (b"{\"a\":1,\"a\":2}", DuplicateKey { first: 1 }, 7),
    ]
}

#[test]
fn every_string_is_listed_with_its_role_span_decoded_text_and_path() {
    let document = ADMITTED[16];
    let ranges = ranges(document);
    let tags = Key("tags");
    assert_eq!(
        listed(document, &ranges),
        vec![
            (KeyRole, r#""name""#, "name", vec![]),
            (ValueRole, r#""Ada""#, "Ada", vec![Key("name")]),
            (KeyRole, r#""tags""#, "tags", vec![]),
            (ValueRole, r#""x""#, "x", vec![tags, Index(0)]),
            (ValueRole, r#""y\n""#, "y\n", vec![tags, Index(1)]),
            (KeyRole, r#""k""#, "k", vec![tags, Index(2)]),
            (ValueRole, r#""v""#, "v", vec![tags, Index(2), Key("k")]),
            (KeyRole, r#""n""#, "n", vec![tags, Index(2)]),
            (
                ValueRole,
                r#""deep""#,
                "deep",
                vec![tags, Index(2), Key("n"), Index(1)]
            ),
            (KeyRole, r#""n""#, "n", vec![]),
            (KeyRole, r#""ok""#, "ok", vec![]),
            (KeyRole, r#""none""#, "none", vec![]),
            (KeyRole, r#""empty""#, "empty", vec![]),
            (ValueRole, r#""""#, "", vec![Key("empty")]),
        ]
    );
    // Spans are the bytes as written, white space and all.
    assert_eq!(ranges.strings()[0].token(), only(document, r#""name""#));
    assert_eq!(ranges.strings()[1].token(), only(document, r#""Ada""#));
    assert_eq!(ranges.strings()[4].token(), only(document, r#""y\n""#));
    assert_eq!(ranges.strings()[4].content().len(), 3);
    let escaped: Vec<bool> = ranges.strings().iter().map(JsonString::escaped).collect();
    assert_eq!(escaped.iter().filter(|escaped| **escaped).count(), 1);
    assert!(escaped[4]);
    // Nothing beyond the list.
    assert_eq!(ranges.decoded(14), None);
    assert_eq!(ranges.path(14), None);
    // The index prints no document text.
    assert_eq!(
        format!("{ranges:?}"),
        format!("JsonRanges {{ bytes: {}, strings: 14 }}", document.len())
    );
}

#[test]
fn a_primitive_or_an_empty_container_is_a_whole_document() {
    let string = ranges(ADMITTED[0]);
    assert_eq!(
        listed(ADMITTED[0], &string),
        vec![(ValueRole, r#""x""#, "x", vec![])]
    );
    assert_eq!(
        facts(&string.locate(1..2).unwrap()),
        (0, 1..2, 1..1, 2..2, 0..1, true)
    );
    // White space around the document's one value is not part of it.
    let spaced = ranges(ADMITTED[1]);
    assert_eq!(
        listed(ADMITTED[1], &spaced),
        vec![(ValueRole, r#""a\u0062""#, "ab", vec![])]
    );
    assert_eq!(spaced.strings()[0].token(), 4..13);
    for document in &ADMITTED[2..16] {
        assert!(ranges(document).strings().is_empty(), "{document}");
        assert_eq!(
            ranges(document).locate(0..document.len()),
            Err(Outside::Structure)
        );
    }
}

#[test]
fn a_key_is_never_a_value() {
    // An edit of `old` in this file would rename a key.
    let document = r#"{"old":"v"}"#;
    let ranges = ranges(document);
    assert_eq!(
        ranges.locate_all("old"),
        Err(Refused {
            occurrence: 0,
            range: 2..5,
            outside: Outside::Key { string: 0 },
        })
    );
    assert_eq!(ranges.locate(3..4), Err(Outside::Key { string: 0 }));
    assert_eq!(ranges.locate(3..3), Err(Outside::Key { string: 0 }));
    // The same text as a value is one.
    let found = ranges.locate_all("v").unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(facts(&found[0]), (1, 8..9, 8..8, 9..9, 0..1, true));
    // With a quote it is no longer inside the value, and with more than
    // that it is no longer inside one string.
    let outside = |text: &str| ranges.locate_all(text).unwrap_err().outside;
    assert_eq!(outside(r#""v""#), Outside::Delimiter { string: 1 });
    assert_eq!(outside(r#""v"#), Outside::Delimiter { string: 1 });
    assert_eq!(outside(r#"v""#), Outside::Delimiter { string: 1 });
    assert_eq!(outside(r#""old""#), Outside::Delimiter { string: 0 });
    assert_eq!(outside(r#":"v"#), Outside::Spans);
    assert_eq!(outside(r#"old":"v"#), Outside::Spans);
    assert_eq!(outside(r#"v"}"#), Outside::Spans);
    assert_eq!(outside(":"), Outside::Structure);
    assert_eq!(outside("{"), Outside::Structure);
    assert_eq!(outside(""), Outside::Empty);
    // A key and a value with the same text are still a key and a value.
    let same = JsonRanges::index(br#"{"v":"v"}"#).unwrap();
    assert_eq!(
        same.locate_all("v"),
        Err(Refused {
            occurrence: 0,
            range: 2..3,
            outside: Outside::Key { string: 0 },
        })
    );
    assert_eq!(
        facts(&same.locate(6..7).unwrap()),
        (1, 6..7, 6..6, 7..7, 0..1, true)
    );
}

#[test]
fn every_occurrence_of_a_text_must_lie_in_a_value() {
    let document = r#"{"a":"x","b":["x","x x"],"c":{"d":"ax"}}"#;
    let ranges = ranges(document);
    let found: Vec<Located> = ranges.locate_all("x").unwrap().iter().map(facts).collect();
    assert_eq!(
        found,
        vec![
            (1, 6..7, 6..6, 7..7, 0..1, true),
            (3, 15..16, 15..15, 16..16, 0..1, true),
            (4, 19..20, 19..19, 20..22, 0..1, true),
            (4, 21..22, 19..21, 22..22, 2..3, true),
            (7, 36..37, 35..36, 37..37, 1..2, true),
        ]
    );
    for (start, ..) in document.match_indices('x') {
        assert!(found.iter().any(|located| located.1.start == start));
    }
    // A text that is not there has no occurrence, and that is not a refusal.
    assert_eq!(ranges.locate_all("q"), Ok(vec![]));
    // Occurrences do not overlap, as a replacement finds them.
    let overlapping = JsonRanges::index(br#"["aaa","aa"]"#).unwrap();
    let found: Vec<Range<usize>> = overlapping
        .locate_all("aa")
        .unwrap()
        .iter()
        .map(Within::range)
        .collect();
    assert_eq!(found, vec![2..4, 8..10]);

    // One occurrence that is a key refuses the text, wherever it comes.
    let later_key = JsonRanges::index(br#"{"a":"x","x":1}"#).unwrap();
    assert_eq!(
        later_key.locate_all("x"),
        Err(Refused {
            occurrence: 1,
            range: 10..11,
            outside: Outside::Key { string: 2 },
        })
    );
    // So does one that is a number, or the punctuation between two values.
    let number = JsonRanges::index(br#"{"a":"x1","b":1}"#).unwrap();
    assert_eq!(
        number.locate_all("1"),
        Err(Refused {
            occurrence: 1,
            range: 14..15,
            outside: Outside::Structure,
        })
    );
    let between = JsonRanges::index(br#"["a","b","a,b"]"#).unwrap();
    assert_eq!(facts(&between.locate(10..13).unwrap()).0, 2);
    assert_eq!(
        between.locate_all("\",\""),
        Err(Refused {
            occurrence: 0,
            range: 3..6,
            outside: Outside::Spans,
        })
    );
}

#[test]
fn a_repeated_key_is_refused_however_it_is_escaped() {
    // The second key of each pair decodes to the first.
    for (first, second) in [
        (r#""a""#, r#""a""#),
        (r#""a""#, r#""\u0061""#),
        (r#""\u0061""#, r#""a""#),
        ("\"\u{e9}\"", r#""\u00e9""#),
        (r#""\u00E9""#, r#""\u00e9""#),
        ("\"\u{1f600}\"", r#""\ud83d\ude00""#),
        (r#""a/b""#, r#""a\/b""#),
        (r#""line\nbreak""#, r#""line\u000abreak""#),
        (r#""""#, r#""""#),
    ] {
        let document = format!(r#"{{"b":0,{first}:1,"c":[{{"a":3}}],{second}:2}}"#);
        let at = document.rfind(second).unwrap();
        assert_eq!(
            refused(document.as_bytes()),
            JsonError {
                fault: JsonFault::DuplicateKey { first: 7 },
                at,
            },
            "{document}"
        );
        // Inside another object the pair is refused the same way.
        let nested = format!(r#"[{{"o":{{{first}:1,{second}:2}}}}]"#);
        assert_eq!(
            refused(nested.as_bytes()).fault,
            JsonFault::DuplicateKey { first: 7 },
            "{nested}"
        );
    }
    // The same key in different objects is not repeated, and keys that only
    // look alike are different keys: nothing is folded or normalized.
    for document in &ADMITTED[17..21] {
        assert!(JsonRanges::index(document.as_bytes()).is_ok(), "{document}");
    }
    let composed = ranges(ADMITTED[20]);
    assert_eq!(composed.decoded(0), Some("\u{e9}"));
    assert_eq!(composed.decoded(1), Some("e\u{301}"));
}

#[test]
fn escapes_and_long_characters_keep_written_and_decoded_offsets_apart() {
    // One value, unit by unit: as written, and what it decodes to.
    let units = [
        ("a", "a"),
        (r"\n", "\n"),
        (r"\u00e9", "\u{e9}"),
        ("\u{e9}", "\u{e9}"),
        ("\u{1f600}", "\u{1f600}"),
        (r"\ud83d\ude00", "\u{1f600}"),
        (r#"\""#, "\""),
        (r"\\", "\\"),
        ("/", "/"),
        (r"\/", "/"),
        ("z", "z"),
    ];
    let written: String = units.iter().map(|unit| unit.0).collect();
    let decoded: String = units.iter().map(|unit| unit.1).collect();
    let document = format!(r#"{{"k":["{written}"]}}"#);
    let ranges = ranges(&document);
    let content = ranges.strings()[1].content();
    assert_eq!(&document[content.clone()], written);
    assert_eq!(ranges.decoded(1), Some(decoded.as_str()));
    assert_eq!(ranges.path(1), Some(vec![Key("k"), Index(0)]));
    assert!(ranges.strings()[1].escaped() && !ranges.strings()[0].escaped());

    // Where each unit begins, as written and decoded; the last entry is the
    // end of the content.
    let mut starts = vec![(content.start, 0)];
    for (raw, text) in units {
        let (at, decoded_at) = starts[starts.len() - 1];
        starts.push((at + raw.len(), decoded_at + text.len()));
    }
    assert_eq!(starts[units.len()], (content.end, decoded.len()));

    // Every range from one unit boundary to another lies in the value, an
    // empty one included, with both sets of offsets exact.
    for from in 0..starts.len() {
        for to in from..starts.len() {
            let range = starts[from].0..starts[to].0;
            let within = ranges.locate(range.clone()).unwrap();
            let literal = units[from..to].iter().all(|unit| unit.0 == unit.1);
            assert_eq!(
                facts(&within),
                (
                    1,
                    range.clone(),
                    content.start..range.start,
                    range.end..content.end,
                    starts[from].1..starts[to].1,
                    literal
                )
            );
            assert_eq!(&decoded[within.decoded()], {
                let text: String = units[from..to].iter().map(|unit| unit.1).collect();
                text
            });
        }
    }
    // Every other offset is inside an escape or inside one character, and a
    // range that begins or ends there is refused with that offset.
    let mut split = 0;
    for at in content.clone() {
        if starts.iter().any(|start| start.0 == at) {
            continue;
        }
        split += 1;
        assert_eq!(
            ranges.locate(content.start..at),
            Err(Outside::Splits { at })
        );
        assert_eq!(ranges.locate(at..content.end), Err(Outside::Splits { at }));
        assert_eq!(ranges.locate(at..at), Err(Outside::Splits { at }));
    }
    assert_eq!(split, written.len() - units.len());
    // A text that occurs only inside escapes is found and refused: the `n`
    // of `\n`, the digits of `\u00e9`, one half of a surrogate pair.
    for (part, at) in [("n", 2), ("00e9", 5), (r"\ud83d", 21), ("ude00", 22)] {
        let refused = ranges.locate_all(part).unwrap_err();
        assert_eq!(refused.occurrence, 0, "{part}");
        assert_eq!(
            refused.outside,
            Outside::Splits {
                at: content.start + at
            }
        );
    }
    // A backslash as written is half of an escape, never a value's text.
    assert!(matches!(
        ranges.locate_all(r"\").unwrap_err().outside,
        Outside::Splits { .. }
    ));
    // The escape as a whole is a unit of the value, and it is not literal.
    let whole = ranges.locate_all(r"\u00e9").unwrap();
    assert_eq!(whole.len(), 1);
    assert!(!whole[0].literal());
    assert_eq!(&decoded[whole[0].decoded()], "\u{e9}");

    // Text that only decodes to something was not written as that
    // something: it has no occurrence there, and its own range says that an
    // escape stands in it.
    let escaped = JsonRanges::index(br#"["A\u002d1","A-1"]"#).unwrap();
    assert_eq!(
        (escaped.decoded(0), escaped.decoded(1)),
        (Some("A-1"), Some("A-1"))
    );
    let found = escaped.locate_all("A-1").unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!((found[0].string(), found[0].literal()), (1, true));
    let written = escaped.locate(escaped.strings()[0].content()).unwrap();
    assert_eq!((written.decoded(), written.literal()), (0..3, false));
}

#[test]
fn a_range_that_is_partly_or_not_at_all_in_a_value_is_refused() {
    let document = r#"{"k":"ab", "n":12, "t":true, "l":["cd","ef"], "e":""}"#;
    let ranges = ranges(document);
    let at = |part: &str| ranges.locate(only(document, part));
    // In one value.
    assert_eq!(facts(&at("ab").unwrap()), (1, 6..8, 6..6, 8..8, 0..2, true));
    assert_eq!(facts(&at("cd").unwrap()).0, 5);
    assert_eq!(
        facts(&ranges.locate(7..8).unwrap()),
        (1, 7..8, 6..7, 8..8, 1..2, true)
    );
    // One string token, with a quote of it.
    for part in [r#""ab"#, r#"ab""#, r#""ab""#, r#"b""#] {
        assert_eq!(at(part), Err(Outside::Delimiter { string: 1 }), "{part}");
    }
    assert_eq!(at(r#""k""#), Err(Outside::Delimiter { string: 0 }));
    assert_eq!(at(r#""""#), Err(Outside::Delimiter { string: 8 }));
    // More than one string, or a string and what stands beside it.
    for part in [
        r#"d","e"#,
        r#""cd","ef""#,
        r#"b", "#,
        r#":"a"#,
        r#"k":"ab"#,
        r#"f"]"#,
    ] {
        assert_eq!(at(part), Err(Outside::Spans), "{part}");
    }
    assert_eq!(ranges.locate(0..document.len()), Err(Outside::Spans));
    // No string at all.
    for part in ["12", "true", "12, ", ":12", "{", "[", "]", "}"] {
        assert_eq!(at(part), Err(Outside::Structure), "{part}");
    }
    // A key.
    assert_eq!(at("k"), Err(Outside::Key { string: 0 }));
    assert_eq!(at("n"), Err(Outside::Key { string: 2 }));
    // Not a range of this document.
    let end = document.len();
    assert_eq!(ranges.locate(end..end + 1), Err(Outside::Document));
    assert_eq!(ranges.locate(6..end + 1), Err(Outside::Document));
    let (later, earlier) = (8, 6);
    assert_eq!(ranges.locate(later..earlier), Err(Outside::Document));
    // A position is in a value from just after its opening quote to just
    // before its closing one, and nowhere else.
    for (position, expected) in [
        (6, Ok((1, 6..6, 6..6, 6..8, 0..0, true))),
        (7, Ok((1, 7..7, 6..7, 7..8, 1..1, true))),
        (8, Ok((1, 8..8, 6..8, 8..8, 2..2, true))),
        (5, Err(Outside::Structure)),
        (9, Err(Outside::Structure)),
        (0, Err(Outside::Structure)),
        (end, Err(Outside::Structure)),
        (2, Err(Outside::Key { string: 0 })),
        (3, Err(Outside::Key { string: 0 })),
    ] {
        let found = ranges.locate(position..position);
        assert_eq!(found.map(|within| facts(&within)), expected);
    }
    let empty = only(document, r#""""#).start + 1;
    assert_eq!(
        facts(&ranges.locate(empty..empty).unwrap()),
        (8, empty..empty, empty..empty, empty..empty, 0..0, true)
    );
}

#[test]
fn an_invalid_document_is_refused_with_its_fault_and_offset() {
    for (document, expected, at) in refusals() {
        assert_eq!(
            refused(document),
            JsonError {
                fault: expected,
                at,
            },
            "{}",
            String::from_utf8_lossy(document)
        );
    }
}

#[test]
fn the_bounds_are_those_a_restored_file_already_has() {
    assert_eq!((MAX_DOCUMENT, MAX_DEPTH, MAX_STRINGS), (262_144, 64, 4096));
    // Bytes.
    let most = format!("\"{}\"", "a".repeat(MAX_DOCUMENT - 2));
    let ranges = ranges(&most);
    assert_eq!(ranges.strings()[0].token(), 0..MAX_DOCUMENT);
    assert_eq!(ranges.decoded(0).map(str::len), Some(MAX_DOCUMENT - 2));
    let size = JsonError {
        fault: JsonFault::Size,
        at: MAX_DOCUMENT,
    };
    assert_eq!(refused(format!("{most} ").as_bytes()), size);
    // The bound comes first, whatever the bytes are.
    assert_eq!(refused(&vec![0xff; MAX_DOCUMENT + 1]), size);

    // Nesting, of arrays and of objects.
    let arrays = |depth: usize| format!("{}{}", "[".repeat(depth), "]".repeat(depth));
    assert!(JsonRanges::index(arrays(MAX_DEPTH).as_bytes()).is_ok());
    assert_eq!(
        refused(arrays(MAX_DEPTH + 1).as_bytes()),
        JsonError {
            fault: JsonFault::Depth,
            at: MAX_DEPTH,
        }
    );
    let objects = |depth: usize| format!("{}1{}", r#"{"a":"#.repeat(depth), "}".repeat(depth));
    let deepest = objects(MAX_DEPTH);
    let ranges = JsonRanges::index(deepest.as_bytes()).unwrap();
    assert_eq!(ranges.strings().len(), MAX_DEPTH);
    assert_eq!(
        ranges.path(MAX_DEPTH - 1),
        Some(vec![Key("a"); MAX_DEPTH - 1])
    );
    assert_eq!(
        refused(objects(MAX_DEPTH + 1).as_bytes()),
        JsonError {
            fault: JsonFault::Depth,
            at: 5 * MAX_DEPTH,
        }
    );

    // Strings, values and keys counted together.
    let values = |count: usize| format!("[{}0]", r#""s","#.repeat(count));
    assert_eq!(
        JsonRanges::index(values(MAX_STRINGS).as_bytes())
            .unwrap()
            .strings()
            .len(),
        MAX_STRINGS
    );
    assert_eq!(
        refused(values(MAX_STRINGS + 1).as_bytes()),
        JsonError {
            fault: JsonFault::Strings,
            at: 1 + 4 * MAX_STRINGS,
        }
    );
    let members = |count: usize| {
        let mut document = String::from("{");
        for number in 0..count {
            document.push_str(&format!(r#""{number:04}":"v","#));
        }
        document.push_str(r#""last":0}"#);
        document
    };
    let most = members(MAX_STRINGS / 2 - 1);
    let ranges = JsonRanges::index(most.as_bytes()).unwrap();
    assert_eq!(ranges.strings().len(), MAX_STRINGS - 1);
    assert_eq!(ranges.path(MAX_STRINGS - 3), Some(vec![Key("2046")]));
    assert_eq!(
        refused(members(MAX_STRINGS / 2).as_bytes()),
        JsonError {
            fault: JsonFault::Strings,
            at: 1 + 11 * (MAX_STRINGS / 2),
        }
    );
}

/// How many strings the parsed value has, keys included.
fn strings_of(value: &serde_json::Value) -> usize {
    match value {
        serde_json::Value::String(_) => 1,
        serde_json::Value::Array(elements) => elements.iter().map(strings_of).sum(),
        serde_json::Value::Object(members) => {
            members.len() + members.values().map(strings_of).sum::<usize>()
        }
        _ => 0,
    }
}

#[test]
fn admission_and_decoding_agree_with_the_json_parser_already_in_use() {
    for document in ADMITTED {
        let ranges = ranges(document);
        let parsed: serde_json::Value = serde_json::from_str(document).expect(document);
        assert_eq!(ranges.strings().len(), strings_of(&parsed), "{document}");
        for (number, string) in ranges.strings().iter().enumerate() {
            // The token alone decodes to the same text.
            let alone: String = serde_json::from_str(&document[string.token()]).unwrap();
            assert_eq!(ranges.decoded(number), Some(alone.as_str()), "{document}");
            // And the path leads to that string in the parsed document.
            let mut value = &parsed;
            for step in ranges.path(number).unwrap() {
                value = match step {
                    Key(key) => &value[key],
                    Index(position) => &value[position],
                };
            }
            match string.role() {
                ValueRole => assert_eq!(value.as_str(), Some(alone.as_str()), "{document}"),
                KeyRole => assert!(value.get(alone.as_str()).is_some(), "{document}"),
            }
        }
    }
    // What is refused here is refused there too. A repeated key is the one
    // exception: that parser keeps the last of the two.
    for (document, expected, _) in refusals() {
        let parsed = serde_json::from_slice::<serde_json::Value>(document);
        assert_eq!(
            parsed.is_ok(),
            matches!(expected, JsonFault::DuplicateKey { .. }),
            "{}",
            String::from_utf8_lossy(document)
        );
    }
}

/// What `str::replace` makes of `before`, and where each replacement stands
/// in the result.
fn replaced(before: &str, old: &str, new: &str) -> (String, Vec<Range<usize>>) {
    let mut at = Vec::new();
    for (number, (start, _)) in before.match_indices(old).enumerate() {
        let start = start + number * new.len() - number * old.len();
        at.push(start..start + new.len());
    }
    (before.replace(old, new), at)
}

#[test]
fn a_replacement_is_located_in_the_document_before_it_and_after_it() {
    // A value's text replaced by text that needs escapes, three times.
    let before = r#"{"a":"x","b":["x","say x!"],"n":1}"#;
    let new = r#"Ada \"Countess\" Lovelace"#;
    let (after, at) = replaced(before, "x", new);
    let (old_ranges, new_ranges) = (ranges(before), ranges(&after));
    let found = old_ranges.locate_all("x").unwrap();
    assert_eq!((found.len(), at.len()), (3, 3));
    assert_eq!(old_ranges.strings().len(), new_ranges.strings().len());
    for (was, range) in found.iter().zip(at) {
        let now = new_ranges.locate(range.clone()).unwrap();
        assert_eq!(now.string(), was.string());
        assert_eq!(now.range(), range);
        assert!(was.literal() && !now.literal());
        let value = new_ranges.decoded(now.string()).unwrap();
        assert_eq!(&value[now.decoded()], r#"Ada "Countess" Lovelace"#);
        // What stood before and after it in the value is what stands there
        // now, byte for byte, at its new offsets.
        assert_eq!(&before[was.before()], &after[now.before()]);
        assert_eq!(&before[was.after()], &after[now.after()]);
    }
    // A removal leaves a position inside the same value.
    let (after, at) = replaced(before, " x", "");
    let (new_ranges, was) = (ranges(&after), old_ranges.locate_all(" x").unwrap());
    let now = new_ranges.locate(at[0].clone()).unwrap();
    assert_eq!((now.string(), now.range().len()), (was[0].string(), 0));
    assert_eq!(new_ranges.decoded(now.string()), Some("say!"));

    // The key of `{"old":"v"}`: refused before, and a key after as well.
    let (after, at) = replaced(r#"{"old":"v"}"#, "old", "new");
    assert!(ranges(r#"{"old":"v"}"#).locate_all("old").is_err());
    assert_eq!(
        ranges(&after).locate(at[0].clone()),
        Err(Outside::Key { string: 0 })
    );

    // Text that is in a value before, and whose replacement is not in one
    // value after: it closes the string and writes a member of its own.
    let before = r#"{"a":"x","b":"y"}"#;
    assert_eq!(
        ranges(before).locate_all("x").map(|found| found.len()),
        Ok(1)
    );
    let (after, at) = replaced(before, "x", r#"","c":""#);
    assert_eq!(after, r#"{"a":"","c":"","b":"y"}"#);
    assert_eq!(ranges(&after).locate(at[0].clone()), Err(Outside::Spans));
    // A quote alone leaves no document at all.
    let (after, _) = replaced(before, "x", "\"");
    assert_eq!(refused(after.as_bytes()).fault, JsonFault::Structure);
    // A replacement that ends as the beginning of an escape takes the next
    // character of the value into it.
    for (before, new, after) in [
        (r#"["xn"]"#, r"\", r#"["\n"]"#),
        (r#"["x9"]"#, r"\u00e", r#"["\u00e9"]"#),
    ] {
        let (made, at) = replaced(before, "x", new);
        assert_eq!(made, after);
        assert_eq!(
            ranges(after).locate(at[0].clone()),
            Err(Outside::Splits { at: at[0].end })
        );
    }
    // And half of a surrogate pair is no text of a value before.
    assert_eq!(
        ranges(r#"["\ud83d\ude00"]"#)
            .locate_all(r"\ud83d")
            .unwrap_err()
            .outside,
        Outside::Splits { at: 8 }
    );
}
