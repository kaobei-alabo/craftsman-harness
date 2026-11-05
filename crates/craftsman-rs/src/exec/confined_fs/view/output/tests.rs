use super::*;
fn redactor(values: &[&[u8]]) -> Redactor {
    Redactor::from_values(values.iter().map(|v| v.to_vec()).collect()).unwrap()
}
#[test]
fn whole_frames_and_scalar_metadata_literals_are_redacted_before_retention() {
    let r = redactor(&[b"secret-value", b"7"]);
    let mut out = Output::new(&r);
    assert!(out.frame("file:7:secret-value").unwrap());
    let view = out.finish(true).unwrap();
    assert_eq!(view.text, "file:[REDACTED]:[REDACTED]");
    assert!(!view.truncated);
}
#[test]
fn forced_cut_never_finishes_a_delayed_credential_prefix() {
    let r = redactor(&[b"credential-value"]);
    let mut out = Output::new(&r);
    out.frame("credential-").unwrap();
    let view = out.finish(false).unwrap();
    assert!(view.truncated && !view.text.contains("credential-"));
}
#[test]
fn bounded_utf8_head_and_notice_are_safe_and_do_not_expand_whole_records() {
    let r = redactor(&[b"output truncated"]);
    let mut out = Output::new(&r);
    let mut complete = true;
    for _ in 0..100 {
        if !out.frame(&"中文🙂".repeat(1000)).unwrap() {
            complete = false;
            break;
        }
    }
    let view = out.finish(complete).unwrap();
    assert!(view.truncated && view.text.len() <= BUDGET);
    assert!(!view.text.contains('\u{fffd}') && !view.text.contains("output truncated"));
}
#[test]
fn replacement_boundary_and_invalid_utf8_are_failures_not_silent_rewriting() {
    let r = redactor(&[b"secret", b"x[REDACTED]"]);
    let mut out = Output::new(&r);
    assert_eq!(
        out.frame("xsecret and sufficient padding"),
        Err(Fault::Rendered)
    );
    let r = redactor(&[b"long-secret-value"]);
    let mut out = Output::new(&r);
    assert_eq!(out.push(&[0xff]), Err(Fault::Utf8));
}
#[test]
fn complete_names_are_verified_without_truncated_aliases() {
    let r = redactor(&[b"secret"]);
    assert_eq!(name(&r, "secret.txt").unwrap(), "[REDACTED].txt");
    let r = redactor(&[b"secret", b"x[REDACTED]"]);
    assert_eq!(name(&r, "xsecret.txt"), Err(Fault::Rendered));
}
