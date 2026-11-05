use super::capture::{CaptureError, Captured, Origin};
use super::*;
use crate::config::Config;
use crate::shell::ShellSettings;
use serde_json::json;
use std::collections::BTreeMap;

fn redactor(seeds: &[&[u8]]) -> Redactor {
    Redactor::from_values(seeds.iter().map(|s| s.to_vec()).collect()).unwrap()
}
fn reference(input: &[u8], seeds: &[&[u8]]) -> Vec<u8> {
    let mut covered = vec![false; input.len()];
    for seed in seeds {
        for start in 0..input.len() {
            if input[start..].starts_with(seed) {
                covered[start..start + seed.len()].fill(true);
            }
        }
    }
    let mut output = Vec::new();
    let mut inside = false;
    for (byte, hidden) in input.iter().zip(covered) {
        if hidden {
            if !inside {
                output.extend_from_slice(MARKER);
            }
            inside = true;
        } else {
            output.push(*byte);
            inside = false;
        }
    }
    output
}
fn capture(cfg: &Config, values: &[(&str, &str)]) -> Captured {
    let env: BTreeMap<_, _> = values
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    Captured::with_lookup(cfg, &ShellSettings::from_config(cfg).secret_vars, |name| {
        Ok(env.get(name).cloned())
    })
    .unwrap()
}

#[test]
fn whole_binary_and_text_redaction_preserve_nonsecret_bytes() {
    let r = redactor(&[b"secret-value", b"second-key"]);
    assert_eq!(
        r.text("before secret-value and second-key after").unwrap(),
        "before [REDACTED] and [REDACTED] after"
    );
    assert_eq!(
        r.bytes(b"\0\xff secret-value\r\n").unwrap(),
        b"\0\xff [REDACTED]\r\n"
    );
    assert_eq!(
        r.text("no credential: 😀 中文").unwrap(),
        "no credential: 😀 中文"
    );
}
#[test]
fn every_single_split_and_one_byte_chunks_match_whole_redaction() {
    let seeds = &[b"ababa".as_slice(), b"babac", b"key-value"];
    let r = redactor(seeds);
    let input = b"prefix ababac/key-value and ababa suffix";
    let expected = reference(input, seeds);
    for split in 0..=input.len() {
        let mut s = r.byte_stream();
        let mut out = s.push(&input[..split]).unwrap();
        out.extend(s.push(&input[split..]).unwrap());
        out.extend(s.finish().unwrap());
        assert_eq!(out, expected, "split {split}");
    }
    let mut s = r.byte_stream();
    let mut out = Vec::new();
    for b in input {
        out.extend(s.push(&[*b]).unwrap());
    }
    out.extend(s.finish().unwrap());
    assert_eq!(out, expected);
}
#[test]
fn intersecting_and_adjacent_matches_redact_union_not_longest_only() {
    for (seeds, input) in [
        (&[b"abcde".as_slice(), b"defgh"][..], b"abcdefgh".as_slice()),
        (
            &[b"key".as_slice(), b"key-key"][..],
            b"keykey-key".as_slice(),
        ),
        (
            &[b"abc".as_slice(), b"bcdef", b"efgh"][..],
            b"abcdefgh".as_slice(),
        ),
    ] {
        assert_eq!(redactor(seeds).bytes(input).unwrap(), MARKER);
    }
}
#[test]
fn repeated_prefix_patterns_match_reference_for_generated_partitioned_inputs() {
    let seeds = &[b"aab".as_slice(), b"abab", b"baa", b"ababa"];
    let r = redactor(seeds);
    for bits in 0..4096usize {
        let input: Vec<_> = (0..12)
            .map(|index| if bits & (1 << index) == 0 { b'a' } else { b'b' })
            .collect();
        let expected = reference(&input, seeds);
        for size in [1, 2, 3, 5, 12] {
            let mut stream = r.byte_stream();
            let mut out = Vec::new();
            for chunk in input.chunks(size) {
                out.extend(stream.push(chunk).unwrap());
            }
            out.extend(stream.finish().unwrap());
            assert_eq!(out, expected);
        }
    }
}
#[test]
fn incomplete_secret_prefix_is_released_only_on_finish() {
    let r = redactor(&[b"long-secret"]);
    let mut s = r.byte_stream();
    assert!(s.push(b"long-sec").unwrap().is_empty());
    assert_eq!(s.finish().unwrap(), b"long-sec");
    assert_eq!(s.push(b"rest").err().unwrap(), RedactionError::Closed);
    assert_eq!(s.finish().err().unwrap(), RedactionError::Closed);
}
#[test]
fn seed_finishing_at_stream_end_is_redacted_and_pending_memory_is_bounded() {
    let r = redactor(&[b"abcab"]);
    let mut s = r.byte_stream();
    let mut out = Vec::new();
    for _ in 0..1000 {
        out.extend(s.push(b"abcab").unwrap());
        assert!(s.pending.len() < r.0.longest);
        assert!(s.ranges.len() <= r.0.longest);
    }
    out.extend(s.finish().unwrap());
    assert_eq!(out, MARKER);
}
#[test]
fn text_stream_never_emits_partial_utf8_and_redacts_unicode_across_chunks() {
    let r = redactor(&["钥匙😀".as_bytes()]);
    let mut s = r.text_stream();
    let mut out = String::new();
    for chunk in ["中文钥", "匙", "😀x", "🙂y"] {
        out.push_str(&s.push(chunk).unwrap());
        assert!(s.pending.len() <= 3);
    }
    out.push_str(&s.finish().unwrap());
    assert_eq!(out, "中文[REDACTED]x🙂y");
}
#[test]
fn binary_seed_that_corrupts_text_fails_without_lossy_replacement() {
    let r = redactor(&[&[0xe4]]);
    let mut s = r.text_stream();
    assert_eq!(s.push("中").err().unwrap(), RedactionError::Utf8);
    assert_eq!(s.push("later").err().unwrap(), RedactionError::Closed);
}
#[test]
fn utf8_holdback_boundary_is_correct_for_every_seed_length() {
    for len in 1..=30 {
        let key = "k".repeat(len);
        let r = redactor(&[key.as_bytes()]);
        let mut s = r.text_stream();
        let mut out = String::new();
        for chunk in ["😀", "é", "中", "hello", key.as_str(), "🙂"] {
            out.push_str(&s.push(chunk).unwrap());
        }
        out.push_str(&s.finish().unwrap());
        assert_eq!(out, "😀é中hello[REDACTED]🙂");
    }
}
#[test]
fn empty_seed_set_preserves_arbitrary_bytes_with_no_delay() {
    let r = Redactor::from_values(vec![]).unwrap();
    let mut s = r.byte_stream();
    assert_eq!(s.push(&[0, 255, 13, 10]).unwrap(), [0, 255, 13, 10]);
    assert!(s.finish().unwrap().is_empty());
}
#[test]
fn duplicate_seeds_are_coalesced_and_constructor_limits_fail_fixed() {
    let r = Redactor::from_values(vec![b"long-key".to_vec(); MAX_INPUT_SEEDS]).unwrap();
    assert_eq!(r.0.patterns.len(), 1);
    for (values, error) in [
        (vec![vec![]], RedactionError::Seed),
        (vec![vec![b'x'; MAX_SEED_BYTES + 1]], RedactionError::Seed),
        (
            vec![b"key".to_vec(); MAX_INPUT_SEEDS + 1],
            RedactionError::Seeds,
        ),
        (
            (0..MAX_SEEDS + 1)
                .map(|i| format!("key-{i}").into_bytes())
                .collect(),
            RedactionError::Seeds,
        ),
    ] {
        assert_eq!(Redactor::from_values(values).err().unwrap(), error);
    }
    let values = (0..5)
        .map(|i| {
            let mut v = vec![b'x'; MAX_SEED_BYTES];
            v[0] = b'0' + i;
            v
        })
        .collect();
    assert_eq!(
        Redactor::from_values(values).err().unwrap(),
        RedactionError::Seeds
    );
}
#[test]
fn marker_conflicting_credentials_fail_closed_including_single_byte_values() {
    for key in [b"R".as_slice(), b"[REDACTED]", b"REDACT"] {
        assert_eq!(
            Redactor::from_values(vec![key.to_vec()]).err().unwrap(),
            RedactionError::Marker
        );
    }
    assert_eq!(
        redactor(&[b"x"]).text("axbxx").unwrap(),
        "a[REDACTED]b[REDACTED]"
    );
}
#[test]
fn chunk_limit_is_checked_before_mutation_and_finish_is_single_use() {
    let r = redactor(&[b"key"]);
    let mut s = r.byte_stream();
    assert_eq!(
        s.push(&vec![b'x'; MAX_CHUNK + 1]).err().unwrap(),
        RedactionError::Chunk
    );
    assert_eq!(s.consumed, 0);
    let mut out = s.push(b"key").unwrap();
    out.extend(s.finish().unwrap());
    assert_eq!(out, MARKER);
    assert_eq!(s.finish().err().unwrap(), RedactionError::Closed);
}
#[test]
fn whole_record_input_and_expanded_output_are_bounded_without_truncation() {
    let r = redactor(&[b"x"]);
    assert_eq!(
        r.bytes(&vec![b'a'; MAX_RECORD + 1]).err().unwrap(),
        RedactionError::Record
    );
    let input = b"xa".repeat(MAX_RECORD / 2);
    assert_eq!(r.bytes(&input).err().unwrap(), RedactionError::Record);
}
#[test]
fn structured_values_and_keys_are_redacted_before_json_escaping() {
    let key = "secret\"\n中文";
    let r = redactor(&[key.as_bytes()]);
    let input = json!({key:{"nested":[key,0,true,null]},"ordinary":"value"});
    let out = r.json(&input).unwrap();
    assert_eq!(
        out,
        json!({"[REDACTED]":{"nested":["[REDACTED]",0,true,null]},"ordinary":"value"})
    );
    let serialized = serde_json::to_string(&out).unwrap();
    assert!(!serialized.contains("secret"));
}
#[test]
fn redacted_structured_member_collisions_are_errors_not_silent_overwrite() {
    let r = redactor(&[b"key-one", b"key-two"]);
    assert_eq!(
        r.json(&json!({"key-one":1,"key-two":2})).err().unwrap(),
        RedactionError::KeyCollision
    );
    assert_eq!(
        r.json(&json!({"key-one":1,"[REDACTED]":2})).err().unwrap(),
        RedactionError::KeyCollision
    );
}
#[test]
fn structured_depth_work_and_encoded_output_bounds_fail_closed() {
    let r = redactor(&[b"secret"]);
    let mut deep = json!(0);
    for _ in 0..MAX_DEPTH + 1 {
        deep = Value::Array(vec![deep]);
    }
    assert_eq!(r.json(&deep).err().unwrap(), RedactionError::Record);
    assert_eq!(
        r.json(&Value::Array(vec![Value::Null; MAX_NODES + 1]))
            .err()
            .unwrap(),
        RedactionError::Record
    );
    assert_eq!(
        r.json(&json!("\0".repeat(MAX_RECORD / 3))).err().unwrap(),
        RedactionError::Record
    );
}
#[test]
fn seed_order_does_not_change_overlap_or_chunk_results() {
    let a = redactor(&[b"abcde", b"defgh", b"bc"]);
    let b = redactor(&[b"bc", b"defgh", b"abcde"]);
    assert_eq!(
        a.bytes(b"abcde-fgh abcdefgh bc").unwrap(),
        b.bytes(b"abcde-fgh abcdefgh bc").unwrap()
    );
}
#[test]
fn parallel_streams_share_only_immutable_seeds_not_pending_or_coverage() {
    let r = redactor(&[b"secret"]);
    let mut a = r.byte_stream();
    let mut b = r.byte_stream();
    let mut first = a.push(b"sec").unwrap();
    let mut second = b.push(b"ordinary").unwrap();
    first.extend(a.push(b"ret").unwrap());
    first.extend(a.finish().unwrap());
    second.extend(b.finish().unwrap());
    assert_eq!(first, MARKER);
    assert_eq!(second, b"ordinary");
}
#[test]
fn position_overflow_is_rejected_before_consuming_input() {
    let r = redactor(&[b"key"]);
    let mut s = r.byte_stream();
    s.consumed = u64::MAX;
    assert_eq!(s.push(b"x").err().unwrap(), RedactionError::Position);
    assert!(s.pending.is_empty());
}
#[test]
fn all_error_diagnostics_are_fixed_and_never_echo_source_values() {
    for e in [
        RedactionError::Seed,
        RedactionError::Seeds,
        RedactionError::Marker,
        RedactionError::Chunk,
        RedactionError::Record,
        RedactionError::Closed,
        RedactionError::Utf8,
        RedactionError::KeyCollision,
        RedactionError::Position,
    ] {
        assert!(!e.to_string().contains("synthetic-key-value"));
    }
    assert!(!CaptureError::Credential.to_string().contains("${"));
}
#[test]
fn known_provider_platform_header_search_and_disabled_sources_are_captured() {
    let cfg:Config=serde_json::from_value(json!({"model":"model","api_key":"prefix-${PROVIDER}","platform":{"api_token":"${PLATFORM}"},
        "search_api_key":"search-key-value","model_kwargs":{"default_headers":{"Authorization":"header-key-value"}},
        "mcp_servers":[{"headers":{"Authorization":"Bearer ${DISABLED}"},"env":{"AUTH":"literal-disabled-key"}}]})).unwrap();
    let captured = capture(
        &cfg,
        &[
            ("PROVIDER", "provider-key-value"),
            ("PLATFORM", "platform-key-value"),
            ("DISABLED", "disabled-env-value"),
        ],
    );
    assert_eq!(captured.route().api_key, "prefix-provider-key-value");
    for key in [
        "prefix-provider-key-value",
        "provider-key-value",
        "platform-key-value",
        "search-key-value",
        "header-key-value",
        "Bearer disabled-env-value",
        "literal-disabled-key",
    ] {
        assert_eq!(
            captured.redactor().text(key).unwrap(),
            "[REDACTED]",
            "known configured source not redacted"
        );
    }
    for origin in [
        Origin::Provider,
        Origin::Platform,
        Origin::Environment,
        Origin::Search,
        Origin::ProviderHeader,
        Origin::DisabledMcp,
    ] {
        assert!(captured.origin_count(origin) > 0);
    }
}
#[test]
fn capture_lookup_visits_only_known_names_once_and_does_not_scan_environment() {
    let cfg: Config =
        serde_json::from_value(json!({"model":"model","api_key":"${KNOWN}-${KNOWN}"})).unwrap();
    let mut visited = BTreeMap::new();
    let c = Captured::with_lookup(
        &cfg,
        &ShellSettings::from_config(&cfg).secret_vars,
        |name| {
            *visited.entry(name.to_string()).or_insert(0) += 1;
            Ok(if name == "KNOWN" {
                Some("known-key-value".into())
            } else {
                None
            })
        },
    )
    .unwrap();
    assert_eq!(c.route().api_key, "known-key-value-known-key-value");
    assert_eq!(
        visited.keys().cloned().collect::<Vec<_>>(),
        vec!["CRAFTSMAN_PLATFORM_TOKEN", "KNOWN"]
    );
    assert!(visited.values().all(|v| *v == 1));
}
#[test]
fn resolved_placeholder_values_are_not_recursively_expanded_or_later_reloaded() {
    let cfg: Config = serde_json::from_value(
        json!({"api_key":"${KEY}","model":"model","base_url":"http://explicit-route"}),
    )
    .unwrap();
    let c = capture(&cfg, &[("KEY", "literal-${NEVER_LOOK_UP}")]);
    assert_eq!(c.route().api_key, "literal-${NEVER_LOOK_UP}");
    assert_eq!(c.route().base_url, "http://explicit-route");
    assert_eq!(
        c.redactor().text("literal-${NEVER_LOOK_UP}").unwrap(),
        "[REDACTED]"
    );
}
#[test]
fn missing_active_provider_and_invalid_templates_fail_without_echoing_names_or_key() {
    for key in ["${MISSING_SECRET_NAME}", "synthetic-key-${", "${bad-name}"] {
        let cfg: Config = serde_json::from_value(json!({"model":"model","api_key":key})).unwrap();
        let e = Captured::with_lookup(&cfg, &ShellSettings::from_config(&cfg).secret_vars, |_| {
            Ok(None)
        })
        .err()
        .unwrap();
        assert_eq!(e, CaptureError::Credential);
        assert!(!e.to_string().contains(key));
    }
}
#[test]
fn unconfigured_disabled_services_can_have_missing_sources_without_provider_activation() {
    let cfg: Config = serde_json::from_value(
        json!({"model":"model","api_key":"provider-literal","platform":{"api_token":"${UNSET}"},
        "mcp_servers":[{"headers":{"AUTH":"${OTHER_UNSET}"}}]}),
    )
    .unwrap();
    let c = capture(&cfg, &[]);
    assert_eq!(c.route().api_key, "provider-literal");
    assert_eq!(c.redactor().text("provider-literal").unwrap(), "[REDACTED]");
}
#[test]
fn configured_nonconventional_credential_name_and_platform_fallback_are_seeded() {
    let cfg: Config = serde_json::from_value(json!({"model":"model","api_key":"${LANG}"})).unwrap();
    let c = capture(
        &cfg,
        &[
            ("LANG", "synthetic-credential-not-a-locale"),
            ("CRAFTSMAN_PLATFORM_TOKEN", " platform-key-value "),
        ],
    );
    assert_eq!(
        c.redactor()
            .text("synthetic-credential-not-a-locale")
            .unwrap(),
        "[REDACTED]"
    );
    assert_eq!(
        c.redactor().text("platform-key-value").unwrap(),
        "[REDACTED]"
    );
}
#[test]
fn credential_capture_bounds_and_marker_conflicts_are_fixed_failures() {
    let cfg: Config = serde_json::from_value(json!({"model":"model","api_key":"${KEY}"})).unwrap();
    assert_eq!(
        Captured::with_lookup(&cfg, &vec!["x".into(); 65], |_| Ok(Some(
            "x".repeat(MAX_SEED_BYTES + 1)
        )))
        .err()
        .unwrap(),
        CaptureError::Bound
    );
    let cfg: Config = serde_json::from_value(json!({"model":"model","api_key":"R"})).unwrap();
    assert_eq!(capture_error(&cfg), CaptureError::Redactor);
}
fn capture_error(cfg: &Config) -> CaptureError {
    Captured::with_lookup(cfg, &[], |_| Ok(None)).err().unwrap()
}

#[test]
fn disabled_or_optional_credentials_cannot_silently_skip_resource_overflow() {
    for cfg in [
        json!({"model":"model","api_key":"provider","platform":{"api_token":"x".repeat(MAX_SEED_BYTES+1)}}),
        json!({"model":"model","api_key":"provider","mcp_servers":[{"headers":{"AUTH":"x".repeat(MAX_SEED_BYTES+1)}}]}),
    ] {
        let cfg: Config = serde_json::from_value(cfg).unwrap();
        assert_eq!(capture_error(&cfg), CaptureError::Bound);
    }
    let cfg: Config =
        serde_json::from_value(json!({"model":"model","api_key":"provider"})).unwrap();
    assert_eq!(
        Captured::with_lookup(
            &cfg,
            &(0..65).map(|i| format!("KNOWN_{i}")).collect::<Vec<_>>(),
            |_| Ok(None)
        )
        .err()
        .unwrap(),
        CaptureError::Bound
    );
}

#[test]
fn bom_and_control_bytes_are_preserved_but_secret_is_redacted_before_any_cut() {
    let r = redactor(&[b"credential-value"]);
    let input = b"\xef\xbb\xbf\x1bprefix credential-value\0\r\n";
    let output = r.bytes(input).unwrap();
    assert_eq!(output, b"\xef\xbb\xbf\x1bprefix [REDACTED]\0\r\n");
    let raw_cut = &input[..18];
    assert!(raw_cut.ends_with(b"credent"));
    let safe_cut = &output[..18];
    assert!(!safe_cut.windows(7).any(|v| v == b"credent"));
    let mut stream = r.byte_stream();
    let mut streamed = stream.push(&input[..18]).unwrap();
    streamed.extend(stream.push(&input[18..]).unwrap());
    streamed.extend(stream.finish().unwrap());
    assert_eq!(streamed, output);
}

#[test]
fn match_fact_is_monotonic_without_exposing_seed_or_matcher_state() {
    let r = redactor(&[b"secret-value"]);
    let mut s = r.byte_stream();
    assert!(!s.matched());
    s.push(b"secret-").unwrap();
    assert!(!s.matched());
    s.push(b"value ordinary").unwrap();
    assert!(s.matched());
    s.finish().unwrap();
    assert!(s.matched());
}

#[test]
fn abort_discards_pending_prefix_without_finish_release_and_closes_stream() {
    let r = redactor(&[b"long-secret-value"]);
    let mut s = r.byte_stream();
    assert!(s.push(b"long-secret").unwrap().is_empty());
    assert!(s.abort());
    assert!(s.pending.is_empty() && s.ranges.is_empty());
    assert_eq!(s.finish().err(), Some(RedactionError::Closed));
    assert_eq!(s.push(b"later").err(), Some(RedactionError::Closed));
    assert!(!s.abort());
}
