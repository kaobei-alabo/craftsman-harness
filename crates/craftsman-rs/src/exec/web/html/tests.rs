use super::*;
use crate::exec::redaction::Redactor;
fn rendered(input: &str) -> Result<String, Fault> {
    let r = Redactor::from_values(vec![]).unwrap();
    let mut output = Output::new(&r);
    let mut html = Html::new();
    assert!(html.push(input, &mut output)?);
    let complete = html.finish(&mut output)?;
    Ok(output.finish(complete)?.text)
}
#[test]
fn bounded_html_state_strips_script_style_tags_and_matches_plain_whitespace_view() {
    let input =
        "  <h1>Hi</h1><script>ignored()</script><style>css</style><p>A &amp; B</p>\n\n\n  C  ";
    assert_eq!(rendered(input).unwrap(), "Hi\nA & B\n\n  C");
    assert_eq!(
        rendered("a<span title='x > y'>b</span>c<br>next").unwrap(),
        "abc\nnext"
    );
}
#[test]
fn decoded_angle_brackets_are_text_not_new_tags() {
    assert_eq!(
        rendered("&lt;script&gt;text&lt;/script&gt;").unwrap(),
        "<script>text</script>"
    );
    assert_eq!(
        rendered("&#x1f642; &#39; &unknown;").unwrap(),
        "🙂 ' &unknown;"
    );
}
#[test]
fn tags_entities_and_whitespace_have_fixed_pending_bounds() {
    assert_eq!(
        rendered(&format!("<a {}>", "q".repeat(TAG_LIMIT))),
        Err(Fault::Limit)
    );
    assert_eq!(
        rendered(&format!("&{};", "q".repeat(ENTITY_LIMIT))),
        Err(Fault::Limit)
    );
    assert_eq!(
        rendered(&format!("a{}b", " ".repeat(SPACE_LIMIT + 1))),
        Err(Fault::Limit)
    );
    assert_eq!(rendered("a<unfinished"), Err(Fault::Syntax));
    assert_eq!(rendered("&#x110000;"), Err(Fault::Syntax));
}
#[test]
fn complete_eof_and_abort_do_not_share_partial_token_finalization() {
    assert_eq!(rendered("a &amp").unwrap(), "a &amp");
    let r = Redactor::from_values(vec![]).unwrap();
    let mut out = Output::new(&r);
    let mut html = Html::new();
    html.push("a &amp", &mut out).unwrap();
    html.abort();
    let view = out.finish(false).unwrap();
    assert!(view.truncated && !view.text.contains("&amp"));
}
