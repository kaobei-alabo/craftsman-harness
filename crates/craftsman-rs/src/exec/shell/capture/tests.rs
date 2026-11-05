use super::*;
fn capture(keys: &[&[u8]], head: usize, tail: usize) -> Capture {
    Capture::new(
        &Redactor::from_values(keys.iter().map(|s| s.to_vec()).collect()).unwrap(),
        head,
        tail,
    )
}
#[test]
fn split_keys_overlap_and_original_counts_precede_head_tail_cuts() {
    let keys = &[b"secret-value".as_slice(), b"abcde", b"defgh"];
    let input = b"before secret-value abcdefgh after";
    for split in 0..=input.len() {
        let mut c = capture(keys, 64, 64);
        c.push(&input[..split]).unwrap();
        c.push(&input[split..]).unwrap();
        c.eof();
        let v = c.finish(true);
        assert_eq!(v.raw_bytes, input.len() as u64);
        assert_eq!(v.head_text().unwrap(), "before [REDACTED] [REDACTED] after");
        assert!(!v.omitted && v.fault.is_none());
    }
}
#[test]
fn head_tail_cuts_never_retain_a_raw_key_crossing_the_head_boundary() {
    for prefix in 0..60 {
        let mut c = capture(&[b"credential-value"], 40, 40);
        c.push(&vec![b'q'; prefix]).unwrap();
        for b in b"credential-value" {
            c.push(&[*b]).unwrap();
        }
        c.push(&vec![b'z'; 70]).unwrap();
        c.eof();
        let v = c.finish(true);
        let body = v.body(60, "stdout").unwrap().0;
        assert!(!body.contains("credent"));
        assert_eq!(v.raw_bytes, (prefix + 16 + 70) as u64);
    }
}
#[test]
fn forced_read_cap_timeout_and_error_never_finish_a_secret_prefix() {
    for normal in [false, true] {
        let mut c = capture(&[b"credential-value"], 100, 100);
        c.push(b"credential-").unwrap();
        let v = c.finish(normal);
        assert!(v.head_text().unwrap().is_empty());
        assert!(v.omitted);
        assert_eq!(v.raw_bytes, 11);
    }
}
#[test]
fn utf8_is_not_lossy_and_safe_tail_start_is_trimmed_at_a_scalar_boundary() {
    let mut c = capture(&["钥匙😀".as_bytes()], 7, 7);
    for b in "中文钥匙😀🙂tail".as_bytes() {
        c.push(&[*b]).unwrap();
    }
    c.eof();
    let v = c.finish(true);
    assert!(v.fault.is_none());
    let body = v.body(600, "stdout").unwrap().0;
    assert!(!body.contains('\u{fffd}') && !body.contains("钥匙"));
    let mut c = capture(&[], 10, 10);
    assert_eq!(c.push(b"q\xff"), Err(Fault::Utf8));
    assert!(c.finish(true).fault.is_some());
}
#[test]
fn replacement_boundary_collisions_fail_and_do_not_preserve_partial_data() {
    let mut c = capture(&[b"secret", b"x[REDACTED]"], 10, 10);
    assert_eq!(c.push(b"xsecret trailing padding"), Err(Fault::Rendered));
    let v = c.finish(true);
    assert!(v.head.is_empty() && v.tail.is_empty() && v.fault.is_some());
}
#[test]
fn retained_head_and_tail_are_bounded_independently_of_stream_size() {
    let mut c = capture(&[b"secret-value"], 50, 50);
    for _ in 0..1000 {
        c.push(&[b'q'; 8192]).unwrap();
    }
    c.eof();
    let v = c.finish(true);
    assert_eq!(v.head.len(), 50);
    assert_eq!(v.tail.len(), 50);
    assert_eq!(v.raw_bytes, 8192000);
}

#[test]
fn invalid_original_bytes_fail_before_secret_lookahead_can_delay_emission() {
    let mut c = capture(&[b"long-secret-value"], 100, 100);
    assert_eq!(c.push(&[0xff]), Err(Fault::Utf8));
    let view = c.finish(false);
    assert_eq!(view.raw_bytes, 1);
    assert!(view.fault.is_some());
}
