use super::*;
use crate::exec::permissions::{InvocationPermissions, OnUnapproved, Selectors};
use serde_json::json;
use std::time::Duration;
use tokio::io::AsyncWriteExt;

fn redactor(keys: &[&[u8]]) -> Redactor {
    Redactor::from_values(keys.iter().map(|k| k.to_vec()).collect()).unwrap()
}
fn gate() -> Arc<InvocationGate> {
    Arc::new(InvocationGate::new(Arc::new(
        InvocationPermissions::from_explicit(
            Selectors {
                on_unapproved: OnUnapproved::Fail,
                ..Default::default()
            },
            &[],
        )
        .unwrap(),
    )))
}
fn capture(keys: &[&[u8]]) -> RedactedCapture {
    RedactedCapture::new(
        &redactor(keys),
        gate(),
        Arc::new(CallBudget::new(Duration::from_secs(30))),
    )
}

#[test]
fn every_split_and_single_byte_writes_redact_before_retention() {
    let input = b"ordinary secret-value abcdefgh end";
    let keys = &[b"secret-value".as_slice(), b"abcde", b"defgh"];
    for split in 0..=input.len() {
        let c = capture(keys);
        c.append(&input[..split]).unwrap();
        c.append(&input[split..]).unwrap();
        assert_eq!(
            c.finish().unwrap(),
            ("ordinary [REDACTED] [REDACTED] end".into(), false)
        );
    }
    let c = capture(keys);
    for byte in input {
        c.append(&[*byte]).unwrap();
    }
    assert_eq!(c.finish().unwrap().0, "ordinary [REDACTED] [REDACTED] end");
}

#[test]
fn secrets_at_every_position_around_log_cap_never_leave_an_original_prefix() {
    let key = b"credential-value";
    for offset in -20isize..=20 {
        let c = capture(&[key]);
        let n = (MAX_LOG_BYTES as isize + offset) as usize;
        for chunk in vec![b'q'; n].chunks(MAX_WRITE) {
            c.append(chunk).unwrap();
        }
        // Split exactly where a legacy original-byte cap would cut the key.
        for b in key {
            c.append(&[*b]).unwrap();
        }
        c.append(b" after").unwrap();
        let (text, truncated) = c.finish().unwrap();
        assert!(text.len() <= MAX_LOG_BYTES);
        assert!(!text.contains("credent"), "offset {offset}");
        let full = format!("{}[REDACTED] after", "q".repeat(n));
        assert_eq!(text, full[..full.len().min(MAX_LOG_BYTES)]);
        assert_eq!(truncated, full.len() > MAX_LOG_BYTES, "offset {offset}");
    }
}

#[test]
fn output_limit_counts_redacted_expansion_not_original_byte_count() {
    let c = capture(&[b"x"]);
    let input = b"xq".repeat(MAX_LOG_BYTES / 2);
    c.append(&input).unwrap();
    let (text, truncated) = c.finish().unwrap();
    assert_eq!(text.len(), MAX_LOG_BYTES);
    assert!(truncated && !text.contains('x'));
    assert!(text.starts_with("[REDACTED]q"));
}

#[test]
fn utf8_splits_and_control_bytes_remain_exact_without_lossy_replacement() {
    let c = capture(&["钥匙😀".as_bytes()]);
    for b in "\u{feff}\0\r\n中文钥匙😀🙂end".as_bytes() {
        c.append(&[*b]).unwrap();
    }
    assert_eq!(
        c.finish().unwrap(),
        ("\u{feff}\0\r\n中文[REDACTED]🙂end".into(), false)
    );
}

#[test]
fn retention_cut_omits_only_an_incomplete_final_utf8_scalar() {
    for byte_in_scalar in 1..=3 {
        let c = capture(&[]);
        c.append(&vec![b'q'; MAX_LOG_BYTES - byte_in_scalar])
            .unwrap();
        c.append("😀tail".as_bytes()).unwrap();
        let (text, truncated) = c.finish().unwrap();
        assert_eq!(text.len(), MAX_LOG_BYTES - byte_in_scalar);
        assert!(truncated && !text.contains('\u{fffd}'));
    }
}

#[test]
fn invalid_utf8_is_a_fixed_failure_not_lossy_text() {
    let c = capture(&[]);
    c.append(b"synthetic-invalid-input-prefix\xff bytes")
        .unwrap();
    let e = c.finish().err().unwrap();
    assert_eq!(e.kind, ErrorKind::InvalidGadget);
    assert!(!e.message.contains("synthetic-invalid-input-prefix"));
    assert!(c.truncated());
    assert!(c.append(b"later").is_err());
}

#[test]
fn generated_marker_boundary_collision_is_rejected_even_at_retention_cut() {
    for padding in [0, MAX_LOG_BYTES - 3, MAX_LOG_BYTES - 1] {
        let c = capture(&[b"secret", b"x[REDACTED]"]);
        c.append(&vec![b'q'; padding]).unwrap();
        c.append(b"xsec").unwrap();
        c.append(b"ret rest").unwrap_err();
        assert!(c.finish().is_err());
        assert!(c.snapshot_for_tests().0.is_empty());
    }
}

#[test]
fn abort_never_flushes_delayed_secret_prefix_or_permits_later_writes() {
    let c = capture(&[b"credential-value"]);
    c.append(b"credential-").unwrap();
    c.abort();
    assert_eq!(c.snapshot_for_tests(), (String::new(), true));
    assert!(c.finish().is_err() && c.append(b"value").is_err());
}

#[test]
fn cap_drains_without_retaining_more_data_or_unbounded_matcher_work() {
    let c = capture(&[b"secret-value"]);
    c.append(&vec![b'q'; MAX_WRITE]).unwrap();
    c.append(&vec![b'q'; MAX_WRITE]).unwrap();
    for _ in 0..100 {
        c.append(&vec![b'q'; MAX_WRITE]).unwrap();
    }
    let b = c.buffer.lock().unwrap();
    assert_eq!(b.retained.len(), MAX_LOG_BYTES);
    assert!(b.discarding && b.truncated);
    drop(b);
    assert_eq!(c.finish().unwrap().0.len(), MAX_LOG_BYTES);
}

#[test]
fn oversize_append_fails_before_retaining_any_record_or_prefix() {
    let c = capture(&[b"secret-value"]);
    let e = c.append(&vec![b'q'; MAX_WRITE + 1]).err().unwrap();
    assert_eq!(e.kind, ErrorKind::OutputLimit);
    assert!(c.snapshot_for_tests().0.is_empty() && c.truncated());
    assert!(c.finish().is_err());
}

#[test]
fn imported_record_overflow_is_a_fixed_latched_capture_failure() {
    let c = capture(&[b"secret-value"]);
    c.reject_oversized_record();
    assert_eq!(c.finish().err().unwrap().kind, ErrorKind::OutputLimit);
    assert!(!c.gate.approval_required());
}

#[tokio::test]
async fn stdout_stderr_flush_and_handle_shutdown_share_order_not_finalization() {
    let c = capture(&[b"secret-value"]);
    let mut stdout = Pin::from(c.async_stream());
    let mut stderr = Pin::from(c.async_stream());
    stdout.write_all(b"before sec").await.unwrap();
    stdout.flush().await.unwrap();
    stdout.shutdown().await.unwrap();
    assert!(stdout.write_all(b"closed").await.is_err());
    stderr.write_all(b"ret-value after").await.unwrap();
    stderr.flush().await.unwrap();
    assert_eq!(
        c.finish().unwrap(),
        ("before [REDACTED] after".into(), false)
    );
    assert!(stderr.write_all(b"late").await.is_err());
    assert!(!c.is_terminal());
}

#[tokio::test]
async fn direct_wasip2_writes_are_captured_synchronously_with_independent_permits() {
    let c = capture(&[b"secret-value"]);
    let mut a = c.p2_stream();
    let mut b = c.p2_stream();
    assert_eq!(a.check_write().unwrap(), WASI_PERMIT);
    assert_eq!(b.check_write().unwrap(), WASI_PERMIT);
    a.write(Bytes::from_static(b"sec")).unwrap();
    a.flush().unwrap();
    a.cancel().await;
    b.write(Bytes::from_static(b"ret-value tail")).unwrap();
    assert!(a.check_write().is_err());
    assert_eq!(c.finish().unwrap(), ("[REDACTED] tail".into(), false));
}

#[test]
fn oversized_or_unpermitted_wasip2_write_is_a_fixed_trap_without_capture() {
    let c = capture(&[]);
    let mut writer = c.p2_stream();
    assert!(matches!(
        writer.write(Bytes::from_static(b"without permit")),
        Err(StreamError::Trap(_))
    ));
    writer.check_write().unwrap();
    assert!(matches!(
        writer.write(Bytes::from(vec![b'q'; WASI_PERMIT + 1])),
        Err(StreamError::Trap(_))
    ));
    assert_eq!(c.finish().unwrap(), (String::new(), false));
}

#[tokio::test]
async fn already_granted_async_and_wasip2_handles_stop_on_same_typed_latch() {
    let c = capture(&[b"secret-value"]);
    let mut async_writer = Pin::from(c.async_stream());
    let mut p2 = c.p2_stream();
    p2.check_write().unwrap();
    c.gate
        .observe_error_kind(craftsman_harness::ToolErrorKind::NeedsApproval);
    assert!(p2.write(Bytes::from_static(b"must-not-write")).is_err());
    assert!(async_writer.write_all(b"must-not-write").await.is_err());
    assert_eq!(c.finish().err().unwrap().kind, ErrorKind::NeedsApproval);
    assert!(c.snapshot_for_tests().0.is_empty());
}

#[tokio::test]
async fn expired_budget_closes_async_and_wasip2_without_capture() {
    let c = capture(&[b"secret-value"]);
    let mut a = Pin::from(c.async_stream());
    let mut b = c.p2_stream();
    b.check_write().unwrap();
    c.set_budget_for_tests(Arc::new(CallBudget::new(Duration::ZERO)));
    assert!(a.write_all(b"secret-value").await.is_err());
    assert!(b.write(Bytes::from_static(b"secret-value")).is_err());
    assert_eq!(c.finish().err().unwrap().kind, ErrorKind::Timeout);
    assert!(c.snapshot_for_tests().0.is_empty());
}

#[test]
fn typed_approval_stop_wins_over_prior_redaction_fault_without_string_heuristics() {
    let c = capture(&[b"secret", b"x[REDACTED]"]);
    c.append(b"xsecret trailing padding").unwrap_err();
    assert!(!c.gate.approval_required());
    c.gate
        .observe_error_kind(craftsman_harness::ToolErrorKind::NeedsApproval);
    assert_eq!(c.finish().err().unwrap().kind, ErrorKind::NeedsApproval);
}

#[test]
fn finish_is_single_use_and_independent_captures_do_not_share_pending_data() {
    let a = capture(&[b"secret-value"]);
    let b = capture(&[b"secret-value"]);
    a.append(b"sec").unwrap();
    b.append(b"ordinary").unwrap();
    a.append(b"ret-value").unwrap();
    assert_eq!(a.finish().unwrap().0, "[REDACTED]");
    assert_eq!(b.finish().unwrap().0, "ordinary");
    assert!(a.finish().is_err());
}

#[test]
fn json_redacts_keys_and_values_before_quote_newline_and_unicode_encoding() {
    let key = "secret\"\n钥匙";
    let raw = serde_json::to_string(&json!({key:[key,true,42,null]})).unwrap();
    let out = super::json(&redactor(&[key.as_bytes()]), &raw).unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&out).unwrap(),
        json!({"[REDACTED]":["[REDACTED]",true,42,null]})
    );
}

#[test]
fn json_colliding_redacted_names_fail_fixed_instead_of_overwriting_data() {
    let e = super::json(
        &redactor(&[b"key-one", b"key-two"]),
        r#"{"key-one":1,"key-two":2}"#,
    )
    .err()
    .unwrap();
    assert_eq!(e.kind, ErrorKind::InvalidGadget);
    assert!(!e.message.contains("key-one") && !e.message.contains("key-two"));
}

#[test]
fn json_scalar_syntax_and_generated_boundary_credentials_fail_not_corrupt_types() {
    for (keys, raw) in [
        (vec![b"12345".as_slice()], r#"{"count":12345}"#),
        (vec![b"true".as_slice()], r#"{"flag":true}"#),
        (vec![b"null".as_slice()], r#"{"value":null}"#),
        (vec![b":".as_slice()], r#"{"value":0}"#),
        (
            vec![b"secret".as_slice(), b"x[REDACTED]"],
            r#"{"value":"xsecret"}"#,
        ),
    ] {
        let e = super::json(&redactor(&keys), raw).err().unwrap();
        assert_eq!(e.kind, ErrorKind::InvalidGadget);
        assert!(e.message.starts_with("private guest"));
    }
}

#[test]
fn json_input_depth_work_expansion_and_encoded_output_limits_return_no_partial_record() {
    let r = redactor(&[b"x"]);
    for raw in [
        "x".repeat(craftsman_gadget::MAX_JSON_BYTES + 1),
        format!("{}0{}", "[".repeat(66), "]".repeat(66)),
        serde_json::to_string(&json!("xq".repeat(100_000))).unwrap(),
        serde_json::to_string(&json!("\0".repeat(200_000))).unwrap(),
    ] {
        assert!(super::json(&r, &raw).is_err());
    }
    assert_eq!(
        super::json(&r, "invalid SECRET JSON").err().unwrap().kind,
        ErrorKind::InvalidGadget
    );
}
