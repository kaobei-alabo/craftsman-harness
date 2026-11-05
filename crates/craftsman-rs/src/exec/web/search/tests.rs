use super::*;
use serde_json::json;
fn redactor(values: &[&[u8]]) -> Redactor {
    Redactor::from_values(values.iter().map(|v| v.to_vec()).collect()).unwrap()
}
#[test]
fn strings_and_urls_are_redacted_without_using_redacted_provider_keys_as_authority() {
    let r = redactor(&[b"secret", b"web"]);
    let original = json!({"web":{"results":[{"title":"secret title","url":"https://example.com/secret","description":"a secret snippet"}]}});
    let view = render(&r, &original).unwrap();
    assert_eq!(
        view.text,
        "1. [REDACTED] title\n   https://example.com/[REDACTED]\n   a [REDACTED] snippet\n"
    );
    assert!(!view.truncated);
    assert_eq!(original["web"]["results"][0]["title"], "secret title");
}
#[test]
fn whole_normalized_snippets_are_redacted_before_scalar_cut() {
    let r = redactor(&[b"secret-value", b"joined words"]);
    let original = json!({"web":{"results":[{"title":"title","url":"url","description":format!("{}secret-value joined\n\twords tail","q".repeat(294))}]}});
    let view = render(&r, &original).unwrap();
    assert!(view.truncated && !view.text.contains("secret") && !view.text.contains("joined"));
    assert!(view.text.contains('…'));
}
#[test]
fn key_collisions_and_untrusted_scalar_or_syntax_literals_fail_closed() {
    let r = redactor(&[b"secret"]);
    assert!(matches!(
        render(&r, &json!({"web":{"results":[]},"secret":1,"[REDACTED]":2})),
        Err(Fault::Redaction)
    ));
    let r = redactor(&[b"42"]);
    assert!(matches!(
        render(&r, &json!({"web":{"results":[]},"number":42})),
        Err(Fault::Rendered)
    ));
    let r = redactor(&[b"true"]);
    assert!(matches!(
        render(&r, &json!({"web":{"results":[]},"flag":true})),
        Err(Fault::Rendered)
    ));
}
#[test]
fn replacement_encoding_collisions_fail_without_silently_corrupting_types() {
    let r = redactor(&[b"secret", b"x[REDACTED]"]);
    assert!(matches!(
        render(&r, &json!({"web":{"results":[{"title":"xsecret"}]}})),
        Err(Fault::Rendered)
    ));
}
#[test]
fn only_actual_empty_results_are_no_results_not_malformed_schema() {
    let r = redactor(&[]);
    assert_eq!(
        render(&r, &json!({"web":{"results":[]}})).unwrap().text,
        "No results."
    );
    assert!(matches!(
        render(&r, &json!({"wrong":[]})),
        Err(Fault::Syntax)
    ));
    assert!(matches!(
        render(&r, &json!({"web":{"results":[17]}})),
        Err(Fault::Syntax)
    ));
}
#[test]
fn result_selection_is_bounded_and_does_not_hide_colliding_unused_metadata() {
    let r = redactor(&[]);
    let rows = (0..11)
        .map(|i| json!({"title":format!("ROW-{i}"),"url":"url"}))
        .collect::<Vec<_>>();
    let view = render(&r, &json!({"web":{"results":rows}})).unwrap();
    assert!(view.truncated && !view.text.contains("ROW-10"));
    let r = redactor(&[b"42"]);
    let mut rows = (0..11).map(|_| json!({"title":"row"})).collect::<Vec<_>>();
    rows[10] = json!({"number":42});
    assert!(matches!(
        render(&r, &json!({"web":{"results":rows}})),
        Err(Fault::Rendered)
    ));
}
