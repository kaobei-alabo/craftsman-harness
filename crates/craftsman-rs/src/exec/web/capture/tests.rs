use super::*;
fn redactor(values: &[&[u8]]) -> Redactor {
    Redactor::from_values(values.iter().map(|v| v.to_vec()).collect()).unwrap()
}
#[test]
fn original_split_keys_and_utf8_are_redacted_before_publication() {
    let r = redactor(&["钥匙🙂".as_bytes(), b"secret-value"]);
    let input = "before 钥匙🙂 secret-value after";
    for split in 0..=input.len() {
        let mut capture = Capture::new(&r, false, 200_000);
        capture.push(&input.as_bytes()[..split]).unwrap();
        capture.push(&input.as_bytes()[split..]).unwrap();
        assert_eq!(capture.observed(), input.len() as u64);
        let view = capture.finish(true).unwrap();
        assert!(!view.truncated);
        assert_eq!(view.text, "before [REDACTED] [REDACTED] after");
    }
}
#[test]
fn original_read_cut_never_finishes_a_secret_prefix_or_incomplete_utf8() {
    let r = redactor(&[b"credential-value"]);
    let input = format!("{}credential-value after", "q".repeat(40));
    let mut capture = Capture::new(&r, false, 48);
    assert!(!capture.push(input.as_bytes()).unwrap());
    assert_eq!(capture.observed(), input.len() as u64); // actual received chunk, not admitted prefix
    let view = capture.finish(false).unwrap();
    assert!(view.truncated && !view.text.contains("credent"));
    let r = redactor(&[]);
    let mut capture = Capture::new(&r, false, 4);
    assert!(!capture.push("a中文".as_bytes()).unwrap());
    assert!(!capture.finish(false).unwrap().text.contains('\u{fffd}'));
}
#[test]
fn invalid_original_utf8_fails_before_secret_lookahead_delays_emission() {
    let r = redactor(&[b"long-credential-value"]);
    let mut capture = Capture::new(&r, false, 200_000);
    assert_eq!(capture.push(&[255]), Err(Fault::Utf8));
    assert_eq!(capture.observed(), 1);
}
#[test]
fn raw_replacement_collisions_fail_before_html_or_output_cuts() {
    let r = redactor(&[b"secret", b"x[REDACTED]"]);
    let mut capture = Capture::new(&r, true, 200_000);
    assert_eq!(
        capture.push(b"xsecret trailing enough padding"),
        Err(Fault::Rendered)
    );
}
#[test]
fn html_joining_and_entity_decoding_get_another_pre_cut_redaction_pass() {
    let r = redactor(&[b"secret"]);
    for input in [
        "before se<span>c</span>ret after",
        "before s&#101;cr&#x65;t after",
    ] {
        for split in 0..=input.len() {
            let mut capture = Capture::new(&r, true, 200_000);
            capture.push(&input.as_bytes()[..split]).unwrap();
            capture.push(&input.as_bytes()[split..]).unwrap();
            let view = capture.finish(true).unwrap();
            assert_eq!(view.text, "before [REDACTED] after");
        }
    }
}
#[test]
fn complete_json_input_preserves_grammar_until_structured_redaction() {
    let r = redactor(&[b"secret"]);
    let input = br#"{"title":"se\u0063ret","number":7}"#;
    for split in 0..=input.len() {
        let mut collector = JsonInput::new(&r, 512 * 1024);
        collector.push(&input[..split]).unwrap();
        collector.push(&input[split..]).unwrap();
        assert_eq!(collector.observed(), input.len() as u64);
        let value = collector.finish().unwrap();
        assert_eq!(value["title"], "secret");
        assert_eq!(value["number"], 7);
        assert_eq!(r.json(&value).unwrap()["title"], "[REDACTED]");
    }
}
#[test]
fn oversize_partial_or_non_utf8_json_is_rejected_not_published_as_a_prefix() {
    let r = redactor(&[b"secret"]);
    let mut collector = JsonInput::new(&r, 4);
    assert_eq!(collector.push(b"\"secret\""), Err(Fault::Limit));
    let mut collector = JsonInput::new(&r, 100);
    collector.push(b"{\"a\":").unwrap();
    assert!(matches!(collector.finish(), Err(Fault::Syntax)));
    let mut collector = JsonInput::new(&r, 100);
    assert_eq!(collector.push(&[255]), Err(Fault::Utf8));
}
#[test]
fn model_text_budget_is_checked_before_any_other_middleware_cut() {
    let r = redactor(&[b"secret"]);
    let mut capture = Capture::new(&r, false, 200_000);
    assert!(!capture.push(&vec![b'q'; 150_000]).unwrap());
    let view = capture.finish(false).unwrap();
    assert!(view.truncated && view.text.len() <= 100_000);
}
