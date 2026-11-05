use super::*;
fn redactor(values: &[&[u8]]) -> Redactor {
    Redactor::from_values(values.iter().map(|v| v.to_vec()).collect()).unwrap()
}
fn body(line: &Line<'_, '_>) -> String {
    line.chunks().map(Result::unwrap).collect()
}
#[test]
fn original_positions_and_multiline_coverage_survive_every_page_selection() {
    let r = redactor(&[b"first\nsecond\nthird"]);
    let text = "before\nfirst\nsecond\nthird\nafter\n";
    let view = r.masked_text(text).unwrap();
    for page in 0..5 {
        let line = view.lines().nth(page).unwrap();
        assert_eq!(line.number(), page + 1);
        assert_eq!(
            body(&line),
            ["before", "[REDACTED]", "[REDACTED]", "[REDACTED]", "after"][page]
        );
    }
}
#[test]
fn line_semantics_match_rust_original_lines_without_renumbering_or_lossy_decoding() {
    let r = redactor(&[]);
    for input in [
        "",
        "\n",
        "a\n",
        "a\n\n",
        "\r",
        "\r\n",
        "a\r\nb\r",
        "中文\r\n🙂\n",
    ] {
        let view = r.masked_text(input).unwrap();
        let lines = view.lines().collect::<Vec<_>>();
        assert_eq!(
            lines.iter().map(body).collect::<Vec<_>>(),
            input.lines().collect::<Vec<_>>()
        );
        assert_eq!(
            lines.iter().map(Line::number).collect::<Vec<_>>(),
            (1..=lines.len()).collect::<Vec<_>>()
        );
    }
}
#[test]
fn overlaps_and_adjacency_use_original_union_not_replacement_rescanning() {
    let r = redactor(&[b"abcde", b"defgh", b"ij"]);
    let view = r.masked_text("before abcdefghij after").unwrap();
    assert_eq!(
        body(&view.lines().next().unwrap()),
        "before [REDACTED] after"
    );
}
#[test]
fn grep_decisions_use_original_content_and_not_the_masked_view() {
    let r = redactor(&[b"needle"]);
    let view = r.masked_text("raw needle line").unwrap();
    let line = view.lines().next().unwrap();
    assert!(line.matches("needle"));
    assert!(!line.matches("[REDACTED]"));
    assert_eq!(body(&line), "raw [REDACTED] line");
}
#[test]
fn bounded_chunks_handle_utf8_and_keys_across_chunk_and_bitmap_boundaries() {
    let r = redactor(&["钥匙🙂".as_bytes()]);
    for padding in [0, 61, 62, 63, 64, 65, 8188, 8191, 8192] {
        let input = format!("{}钥匙🙂{}", "q".repeat(padding), "中".repeat(4000));
        let view = r.masked_text(&input).unwrap();
        let line = view.lines().next().unwrap();
        for chunk in line.chunks() {
            assert!(chunk.unwrap().len() <= CHUNK);
        }
        assert_eq!(
            body(&line),
            format!("{}[REDACTED]{}", "q".repeat(padding), "中".repeat(4000))
        );
    }
}
#[test]
fn pathological_overlapping_seeds_do_not_allocate_per_match_or_expand_a_whole_file() {
    let r = redactor(&[&[b'q'; 1000]]);
    let input = "q".repeat(256 * 1024);
    let view = r.masked_text(&input).unwrap();
    assert_eq!(view.covered.len(), input.len().div_ceil(64));
    assert_eq!(body(&view.lines().next().unwrap()), "[REDACTED]");
}
#[test]
fn partial_scalar_masks_fail_closed_and_oversize_sources_are_rejected() {
    let r = redactor(&[&[0xa9]]);
    let view = r.masked_text("é").unwrap();
    assert_eq!(
        view.lines()
            .next()
            .unwrap()
            .chunks()
            .next()
            .unwrap()
            .unwrap_err(),
        RedactionError::Utf8
    );
    let r = redactor(&[]);
    assert!(matches!(
        r.masked_text(&"q".repeat(MAX_SOURCE_BYTES + 1)),
        Err(RedactionError::Record)
    ));
}

#[test]
fn single_line_mask_views_match_original_union_redaction_for_generated_inputs() {
    let r = redactor(&[b"aba", b"bab", b"aab", b"secret"]);
    for number in 0..1024u32 {
        let input = (0..10)
            .map(|bit| if number & (1 << bit) == 0 { 'a' } else { 'b' })
            .collect::<String>();
        let input = format!("{input} secret {input}");
        let view = r.masked_text(&input).unwrap();
        assert_eq!(body(&view.lines().next().unwrap()), r.text(&input).unwrap());
    }
}
