//! These use an actual isolated local vault and host-supplied synthetic keys.
//! No normal-session activation, provider call, user data or real filesystem
//! operation is inferred from a successfully restored provisional content view.

use super::*;
use crate::privacy_vault::{
    DeclaredIdentifiers, DetectorReport, DetectorSpan, HostKey, HostKeySource, HostNamespace,
    IdentifierKind, Limits, MaskedText, Vault, VaultLocation,
};

struct SyntheticKey;
impl HostKeySource for SyntheticKey {
    fn acquire_privacy_key(&self) -> crate::privacy_vault::Result<HostKey> {
        Ok(HostKey::supplied_by_host(Zeroizing::new([0x6b; 32])))
    }
}

fn namespace() -> HostNamespace {
    HostNamespace::supplied_by_host("owned-host", "owned-project", "owned-thread", "v1", "data")
        .unwrap()
}

async fn fixture(
    values: &[(&str, Category)],
) -> (tempfile::TempDir, Vault, MappingSession, MaskedText) {
    let root = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let vault = Vault::open(
        VaultLocation::under_host_config_root(&std::fs::canonicalize(root.path()).unwrap())
            .unwrap(),
        &SyntheticKey,
        Limits::default(),
    )
    .await
    .unwrap();
    let session = vault.session(namespace()).unwrap();
    let source = values
        .iter()
        .map(|(text, _)| *text)
        .collect::<Vec<_>>()
        .join("\n");
    let input = session.capture_text(&source).unwrap();
    let mut spans = Vec::new();
    let mut identifiers = Vec::new();
    for (index, &(value, category)) in values.iter().enumerate() {
        if values[..index].iter().any(|(prior, _)| *prior == value) {
            continue;
        }
        for (start, _) in source.match_indices(value) {
            spans.push(DetectorSpan {
                start,
                end: start + value.len(),
                category,
            });
        }
        if let Category::Identifier(kind) = category {
            identifiers.push((kind, value));
        }
    }
    let digest = input.input_sha256_for_host_detector().unwrap();
    let masked = session
        .mask(
            input,
            DetectorReport {
                input_sha256: digest,
                spans,
            },
            DeclaredIdentifiers::supplied_by_host(&identifiers).unwrap(),
        )
        .await
        .unwrap();
    (root, vault, session, masked)
}

fn alias(masked: &MaskedText, category: Category) -> String {
    let records = masked.resolution_records_for_host_fields().unwrap();
    for index in 0..records.len().unwrap() {
        let found = records
            .with_host_field(index, |field| {
                (field.category() == category).then(|| field.known_alias().to_owned())
            })
            .unwrap();
        if let Some(found) = found {
            return found;
        }
    }
    panic!("synthetic fixture has no requested alias")
}

#[tokio::test]
async fn complete_display_restores_multiple_repeated_values_without_mutating_masked_history() {
    let (_root, _vault, session, masked) = fixture(&[
        ("alice@example.test", Category::Email),
        ("+886 912-345-678", Category::Telephone),
        (
            "CUSTOMER-4821",
            Category::Identifier(IdentifierKind::Customer),
        ),
    ])
    .await;
    let email = alias(&masked, Category::Email);
    let phone = alias(&masked, Category::Telephone);
    let customer = alias(&masked, Category::Identifier(IdentifierKind::Customer));
    let response =
        format!("Contact: {email}; phone: {phone}; customer: {customer}. Again: {email}.");
    let before = masked.text_for_later_model_request().unwrap().to_owned();
    let records = masked.resolution_records_for_host_fields().unwrap();
    let restorer = Restorer::from_records(&session, records).unwrap();
    let displayed = restorer.display(&response).unwrap();
    displayed.with_host_display_text(|text| {
        assert_eq!(text, "Contact: alice@example.test; phone: +886 912-345-678; customer: CUSTOMER-4821. Again: alice@example.test.");
    }).unwrap();
    assert_eq!(masked.text_for_later_model_request().unwrap(), before);
    assert!(!response.contains("alice@example.test"));
    assert!(!format!("{restorer:?} {displayed:?}").contains("alice@example.test"));
}

#[tokio::test]
async fn display_does_not_guess_foreign_altered_or_embedded_aliases() {
    let (_root, _vault, session, masked) =
        fixture(&[("alice@example.test", Category::Email)]).await;
    let known = alias(&masked, Category::Email);
    let unknown = "pv1-00000000000000000000000000000000@privacy.invalid";
    assert_ne!(known, unknown);
    let response = format!("Unknown: {unknown}; changed: x{known}; extended: {known}.other");
    let restorer = Restorer::from_records(
        &session,
        masked.resolution_records_for_host_fields().unwrap(),
    )
    .unwrap();
    restorer
        .display(&response)
        .unwrap()
        .with_host_display_text(|text| assert_eq!(text, response))
        .unwrap();
}

#[tokio::test]
async fn scalar_safe_plain_file_data_restores_known_fields_as_a_provisional_candidate() {
    let (_root, _vault, session, masked) =
        fixture(&[("alice@example.test", Category::Email)]).await;
    let known = alias(&masked, Category::Email);
    let input = format!("匠： {known}\r\n🛠️ unchanged\n");
    let records = masked.resolution_records_for_host_fields().unwrap();
    let restorer = Restorer::from_records(&session, records).unwrap();
    let candidate = restorer
        .file_data(FileDataFormat::PlainText, &input)
        .unwrap();
    candidate
        .with_host_candidate_content(|text| {
            assert_eq!(text, "匠： alice@example.test\r\n🛠️ unchanged\n");
        })
        .unwrap();
    assert!(!input.contains("alice@example.test"));
    assert_eq!(format!("{candidate:?}"), "FileDataView([private])");
}

#[tokio::test]
async fn unresolved_marked_file_alias_refuses_whole_and_closes_its_own_scope() {
    let (_root, _vault, session, masked) =
        fixture(&[("alice@example.test", Category::Email)]).await;
    let known = alias(&masked, Category::Email);
    let restorer = Restorer::from_records(
        &session,
        masked.resolution_records_for_host_fields().unwrap(),
    )
    .unwrap();
    let input = format!("{known}; pv1-unknown");
    assert_eq!(
        restorer
            .file_data(FileDataFormat::PlainText, &input)
            .unwrap_err(),
        RestoreError::UnknownAlias
    );
    assert!(session.capture_text("no later raw fallback").is_err());
    assert_eq!(input, format!("{known}; pv1-unknown"));
}

#[tokio::test]
async fn equal_supplied_namespace_cannot_rebind_records_from_another_actual_session() {
    let (_root, vault, session, masked) = fixture(&[("alice@example.test", Category::Email)]).await;
    let foreign = vault.session(namespace()).unwrap();
    let records = masked.resolution_records_for_host_fields().unwrap();
    assert_eq!(
        Restorer::from_records(&foreign, records).unwrap_err(),
        RestoreError::Scope
    );
    assert!(foreign.capture_text("refused foreign operation").is_err());
    assert!(
        session
            .capture_text("original scope remains distinct")
            .is_ok()
    );
}

#[tokio::test]
async fn closed_scope_refuses_retained_display_and_file_views_before_callback() {
    let (_root, _vault, session, masked) =
        fixture(&[("alice@example.test", Category::Email)]).await;
    let known = alias(&masked, Category::Email);
    let restorer = Restorer::from_records(
        &session,
        masked.resolution_records_for_host_fields().unwrap(),
    )
    .unwrap();
    let display = restorer.display(&known).unwrap();
    let file = restorer
        .file_data(FileDataFormat::PlainText, &known)
        .unwrap();
    session.close();
    let called = std::cell::Cell::new(false);
    assert_eq!(
        display.with_host_display_text(|_| called.set(true)),
        Err(RestoreError::Scope)
    );
    assert_eq!(
        file.with_host_candidate_content(|_| called.set(true)),
        Err(RestoreError::Scope)
    );
    assert!(!called.get());
}

#[tokio::test]
async fn callback_that_closes_scope_is_not_reported_as_a_successful_view_return() {
    let (_root, _vault, session, masked) =
        fixture(&[("alice@example.test", Category::Email)]).await;
    let known = alias(&masked, Category::Email);
    let restorer = Restorer::from_records(
        &session,
        masked.resolution_records_for_host_fields().unwrap(),
    )
    .unwrap();
    let display = restorer.display(&known).unwrap();
    let called = std::cell::Cell::new(0);
    assert_eq!(
        display.with_host_display_text(|text| {
            assert_eq!(text, "alice@example.test");
            called.set(called.get() + 1);
            session.close();
        }),
        Err(RestoreError::Scope)
    );
    assert_eq!(called.get(), 1); // Actual callback work is not rolled back.
}

#[tokio::test]
async fn json_string_values_escape_restored_identifiers_and_preserve_other_original_bytes() {
    let original = "client \"alpha\\beta\"";
    let category = Category::Identifier(IdentifierKind::Customer);
    let (_root, _vault, session, masked) = fixture(&[(original, category)]).await;
    let known = alias(&masked, category);
    let input =
        format!("{{\r\n  \"name\": \"{known}\", \"n\": 1.2300e+02, \"other\": \"\\u5320\"\r\n}}\n");
    let restorer = Restorer::from_records(
        &session,
        masked.resolution_records_for_host_fields().unwrap(),
    )
    .unwrap();
    let candidate = restorer
        .file_data(FileDataFormat::JsonDocument, &input)
        .unwrap();
    candidate
        .with_host_candidate_content(|text| {
            let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
            assert_eq!(parsed["name"], original);
            assert_eq!(parsed["other"], "匠");
            assert!(text.contains("\"n\": 1.2300e+02, \"other\": \"\\u5320\"\r\n"));
            assert!(text.starts_with("{\r\n  \"name\": "));
            assert!(text.ends_with("\r\n}\n"));
        })
        .unwrap();
}

#[tokio::test]
async fn json_keys_duplicate_escaped_keys_and_malformed_documents_refuse_without_repair() {
    for case in [
        "known-key",
        "duplicate",
        "escaped-duplicate",
        "malformed",
        "unknown",
    ] {
        let (_root, _vault, session, masked) =
            fixture(&[("alice@example.test", Category::Email)]).await;
        let known = alias(&masked, Category::Email);
        let input = match case {
            "known-key" => format!("{{\"{known}\": \"value\"}}"),
            "duplicate" => format!("{{\"a\": \"{known}\", \"a\": \"other\"}}"),
            "escaped-duplicate" => format!("{{\"a\": \"{known}\", \"\\u0061\": \"other\"}}"),
            "malformed" => format!("{{\"a\": \"{known}\""),
            "unknown" => "{\"a\": \"pv1-unknown\"}".into(),
            _ => unreachable!(),
        };
        let restorer = Restorer::from_records(
            &session,
            masked.resolution_records_for_host_fields().unwrap(),
        )
        .unwrap();
        let error = restorer
            .file_data(FileDataFormat::JsonDocument, &input)
            .unwrap_err();
        assert!(matches!(
            error,
            RestoreError::Input | RestoreError::UnknownAlias
        ));
        assert!(session.capture_text("no later raw fallback").is_err());
        assert!(!error.to_string().contains(&known));
        assert!(!error.to_string().contains("alice@example.test"));
    }
}

#[tokio::test]
async fn exact_text_limit_and_json_depth_limit_preserve_empty_and_unchanged_data() {
    let (_root, _vault, session, masked) = fixture(&[]).await;
    let restorer = Restorer::from_records(
        &session,
        masked.resolution_records_for_host_fields().unwrap(),
    )
    .unwrap();
    restorer
        .display("")
        .unwrap()
        .with_host_display_text(|text| assert!(text.is_empty()))
        .unwrap();
    let full = "z".repeat(MAX_TEXT);
    restorer
        .file_data(FileDataFormat::PlainText, &full)
        .unwrap()
        .with_host_candidate_content(|text| assert_eq!(text, full))
        .unwrap();
    let depth = format!(
        "{}0{}",
        "[".repeat(MAX_JSON_DEPTH),
        "]".repeat(MAX_JSON_DEPTH)
    );
    restorer
        .file_data(FileDataFormat::JsonDocument, &depth)
        .unwrap()
        .with_host_candidate_content(|text| assert_eq!(text, depth))
        .unwrap();
    let too_deep = format!("[{depth}]");
    assert_eq!(
        restorer
            .file_data(FileDataFormat::JsonDocument, &too_deep)
            .unwrap_err(),
        RestoreError::Bounds
    );
}

#[tokio::test]
async fn oversized_whole_text_and_replacement_count_refuse_without_partial_output() {
    for count_case in [false, true] {
        let (_root, _vault, session, masked) =
            fixture(&[("ID12", Category::Identifier(IdentifierKind::Account))]).await;
        let known = alias(&masked, Category::Identifier(IdentifierKind::Account));
        let restorer = Restorer::from_records(
            &session,
            masked.resolution_records_for_host_fields().unwrap(),
        )
        .unwrap();
        let text = if count_case {
            format!("{known}\n").repeat(MAX_REPLACEMENTS + 1)
        } else {
            "z".repeat(MAX_TEXT + 1)
        };
        assert_eq!(restorer.display(&text).unwrap_err(), RestoreError::Bounds);
        assert!(session.capture_text("operation remains closed").is_err());
    }
}

#[tokio::test]
async fn telephone_matches_do_not_restore_embedded_or_extended_digit_tokens() {
    let (_root, _vault, session, masked) =
        fixture(&[("+886 912-345-678", Category::Telephone)]).await;
    let known = alias(&masked, Category::Telephone);
    let text = format!("Changed: {known}.9; prefixed: 9{known}; suffix: {known}1");
    let restorer = Restorer::from_records(
        &session,
        masked.resolution_records_for_host_fields().unwrap(),
    )
    .unwrap();
    restorer
        .display(&text)
        .unwrap()
        .with_host_display_text(|value| assert_eq!(value, text))
        .unwrap();
}

#[tokio::test]
async fn json_replacement_ceiling_is_shared_by_every_value_in_the_document() {
    let (_root, _vault, session, masked) = fixture(&[("1234567", Category::Telephone)]).await;
    let known = alias(&masked, Category::Telephone);
    let half = format!("{known};").repeat(MAX_REPLACEMENTS / 2 + 1);
    let text = serde_json::json!({"a": half, "b": half}).to_string();
    assert!(text.len() < MAX_TEXT);
    let restorer = Restorer::from_records(
        &session,
        masked.resolution_records_for_host_fields().unwrap(),
    )
    .unwrap();
    assert_eq!(
        restorer
            .file_data(FileDataFormat::JsonDocument, &text)
            .unwrap_err(),
        RestoreError::Bounds,
    );
    assert!(
        session
            .capture_text("no partial document or raw fallback")
            .is_err()
    );
}

#[tokio::test]
async fn reviewed_parenthesized_phone_continuations_preserve_the_complete_display_token() {
    for prefix in [false, true] {
        let (_root, _vault, session, masked) = fixture(&[("1234567", Category::Telephone)]).await;
        let known = alias(&masked, Category::Telephone);
        let text = if prefix {
            format!("(1) {known}")
        } else {
            format!("{known} (1)")
        };
        let restorer = Restorer::from_records(
            &session,
            masked.resolution_records_for_host_fields().unwrap(),
        )
        .unwrap();
        restorer
            .display(&text)
            .unwrap()
            .with_host_display_text(|display| assert_eq!(display, text))
            .unwrap();
    }
}

#[tokio::test]
async fn reviewed_parenthesized_phone_continuations_refuse_plain_and_json_candidates() {
    for format in [FileDataFormat::PlainText, FileDataFormat::JsonDocument] {
        for prefix in [false, true] {
            let (_root, _vault, session, masked) =
                fixture(&[("1234567", Category::Telephone)]).await;
            let known = alias(&masked, Category::Telephone);
            let text = if prefix {
                format!("(1) {known}")
            } else {
                format!("{known} (1)")
            };
            let input = match format {
                FileDataFormat::PlainText => text,
                FileDataFormat::JsonDocument => serde_json::json!({"phone":text}).to_string(),
            };
            let restorer = Restorer::from_records(
                &session,
                masked.resolution_records_for_host_fields().unwrap(),
            )
            .unwrap();
            assert_eq!(
                restorer.file_data(format, &input).unwrap_err(),
                RestoreError::UnknownAlias
            );
        }
    }
}

#[tokio::test]
async fn reviewed_unicode_mark_and_joiner_continuations_preserve_display_without_normalization() {
    for (original, category) in [
        ("alice@example.test", Category::Email),
        ("1234567", Category::Telephone),
        (
            "CUSTOMER-4821",
            Category::Identifier(IdentifierKind::Customer),
        ),
    ] {
        let (_root, _vault, session, masked) = fixture(&[(original, category)]).await;
        let known = alias(&masked, category);
        let restorer = Restorer::from_records(
            &session,
            masked.resolution_records_for_host_fields().unwrap(),
        )
        .unwrap();
        for continuation in [
            '\u{0301}', '\u{200c}', '\u{200d}', '\u{2060}', '\u{fe0f}', '\u{feff}',
        ] {
            for text in [
                format!("{known}{continuation}"),
                format!("x{continuation}{known}"),
            ] {
                restorer
                    .display(&text)
                    .unwrap()
                    .with_host_display_text(|display| assert_eq!(display, text))
                    .unwrap();
            }
        }
    }
}

#[tokio::test]
async fn reviewed_unicode_mark_and_joiner_continuations_refuse_plain_and_escaped_json_candidates() {
    for (original, category) in [
        ("alice@example.test", Category::Email),
        ("1234567", Category::Telephone),
        (
            "CUSTOMER-4821",
            Category::Identifier(IdentifierKind::Customer),
        ),
    ] {
        for continuation in [
            '\u{0301}', '\u{200c}', '\u{200d}', '\u{2060}', '\u{fe0f}', '\u{feff}',
        ] {
            for prefix in [false, true] {
                for format in [FileDataFormat::PlainText, FileDataFormat::JsonDocument] {
                    let (_root, _vault, session, masked) = fixture(&[(original, category)]).await;
                    let known = alias(&masked, category);
                    let plain = if prefix {
                        format!("x{continuation}{known}")
                    } else {
                        format!("{known}{continuation}")
                    };
                    let escaped = format!("\\u{:04x}", continuation as u32);
                    let input = match format {
                        FileDataFormat::PlainText => plain,
                        FileDataFormat::JsonDocument if prefix => {
                            format!("{{\"value\":\"x{escaped}{known}\"}}")
                        }
                        FileDataFormat::JsonDocument => {
                            format!("{{\"value\":\"{known}{escaped}\"}}")
                        }
                    };
                    let restorer = Restorer::from_records(
                        &session,
                        masked.resolution_records_for_host_fields().unwrap(),
                    )
                    .unwrap();
                    assert_eq!(
                        restorer.file_data(format, &input).unwrap_err(),
                        RestoreError::UnknownAlias
                    );
                }
            }
        }
    }
}

// The complete Unicode Connector_Punctuation (Pc) set, including ASCII's
// already supported LOW LINE. The literal scalars are never normalized.
const REVIEWED_UNICODE_CONNECTORS: [char; 10] = [
    '_', '\u{203f}', '\u{2040}', '\u{2054}', '\u{fe33}', '\u{fe34}', '\u{fe4d}', '\u{fe4e}',
    '\u{fe4f}', '\u{ff3f}',
];

fn connector_embeddings(known: &str, connector: &str) -> [(&'static str, String); 4] {
    [
        ("prefix", format!("{connector}{known}")),
        ("suffix", format!("{known}{connector}")),
        ("prefix-letter", format!("x{connector}{known}")),
        ("suffix-letter", format!("{known}{connector}x")),
    ]
}

async fn connector_display_control(original: &str, category: Category) {
    let (_root, _vault, session, masked) = fixture(&[(original, category)]).await;
    let known = alias(&masked, category);
    let before = masked.text_for_later_model_request().unwrap().to_owned();
    let restorer = Restorer::from_records(
        &session,
        masked.resolution_records_for_host_fields().unwrap(),
    )
    .unwrap();
    restorer
        .display(&known)
        .unwrap()
        .with_host_display_text(|text| assert_eq!(text, original))
        .unwrap();

    // Collect every mismatch so the prior-source failure log retains all ten
    // connector forms and both ends, rather than stopping at the first scalar.
    let mut failures = Vec::new();
    for connector in REVIEWED_UNICODE_CONNECTORS {
        for (shape, embedded) in connector_embeddings(&known, &connector.to_string()) {
            let input = format!("{known}; {embedded}; {known}.");
            let expected = format!("{original}; {embedded}; {original}.");
            match restorer
                .display(&input)
                .and_then(|view| view.with_host_display_text(str::to_owned))
            {
                Ok(display) if display == expected => {}
                Ok(_) => failures.push(format!(
                    "U+{:04X} {shape}: display changed an extended token",
                    connector as u32
                )),
                Err(error) => failures.push(format!(
                    "U+{:04X} {shape}: display refused with {error:?}",
                    connector as u32
                )),
            }
        }
    }
    assert_eq!(masked.text_for_later_model_request().unwrap(), before);
    assert!(
        failures.is_empty(),
        "connector display failures for {category:?}:\n{}",
        failures.join("\n")
    );
}

async fn connector_file_control(original: &str, category: Category) {
    let mut failures = Vec::new();
    for connector in REVIEWED_UNICODE_CONNECTORS {
        for shape_index in 0..4 {
            for form in ["plain", "json-direct", "json-escaped"] {
                // Refusal closes a mapping session, so every negative candidate
                // needs its own actual vault/session and exact known alias.
                let (_root, _vault, session, masked) = fixture(&[(original, category)]).await;
                let known = alias(&masked, category);
                let escaped = format!("\\u{:04x}", connector as u32);
                let (shape, embedded) =
                    connector_embeddings(&known, &connector.to_string())[shape_index].clone();
                let escaped_embedded = connector_embeddings(&known, &escaped)[shape_index]
                    .1
                    .clone();
                let (format, control, expected, input) = match form {
                    "plain" => (
                        FileDataFormat::PlainText,
                        known.clone(),
                        original.to_owned(),
                        format!("{known}; {embedded}; {known}"),
                    ),
                    "json-direct" | "json-escaped" => {
                        let value = if form == "json-escaped" {
                            &escaped_embedded
                        } else {
                            &embedded
                        };
                        let input = format!(
                            "{{\"exact\":\"{known}\",\"embedded\":\"{value}\",\"later\":\"{known}\"}}"
                        );
                        let decoded: serde_json::Value = serde_json::from_str(&input).unwrap();
                        assert_eq!(decoded["embedded"], embedded);
                        if form == "json-escaped" {
                            assert!(input.contains(&escaped));
                        }
                        (
                            FileDataFormat::JsonDocument,
                            format!("{{\"exact\":\"{known}\",\"n\":1.2300e+02}}"),
                            format!("{{\"exact\":\"{original}\",\"n\":1.2300e+02}}"),
                            input,
                        )
                    }
                    _ => unreachable!(),
                };
                let before = input.clone();
                let masked_before = masked.text_for_later_model_request().unwrap().to_owned();
                let restorer = Restorer::from_records(
                    &session,
                    masked.resolution_records_for_host_fields().unwrap(),
                )
                .unwrap();
                restorer
                    .file_data(format, &control)
                    .unwrap()
                    .with_host_candidate_content(|text| assert_eq!(text, expected))
                    .unwrap();
                assert_eq!(
                    masked.text_for_later_model_request().unwrap(),
                    masked_before
                );
                match restorer.file_data(format, &input) {
                    Err(RestoreError::UnknownAlias) => {
                        assert!(session.capture_text("no later raw fallback").is_err());
                    }
                    Err(error) => failures.push(format!(
                        "U+{:04X} {shape} {form}: wrong refusal {error:?}",
                        connector as u32
                    )),
                    Ok(_) => failures.push(format!(
                        "U+{:04X} {shape} {form}: accepted a complete ambiguous candidate",
                        connector as u32
                    )),
                }
                assert_eq!(input, before);
            }
        }
    }
    assert!(
        failures.is_empty(),
        "connector file failures for {category:?}:\n{}",
        failures.join("\n")
    );
}

#[tokio::test]
async fn reviewed_unicode_connector_display_email() {
    connector_display_control("alice@example.test", Category::Email).await;
}

#[tokio::test]
async fn reviewed_unicode_connector_display_telephone() {
    connector_display_control("1234567", Category::Telephone).await;
}

#[tokio::test]
async fn reviewed_unicode_connector_display_customer() {
    connector_display_control(
        "CUSTOMER-4821",
        Category::Identifier(IdentifierKind::Customer),
    )
    .await;
}

#[tokio::test]
async fn reviewed_unicode_connector_display_account() {
    connector_display_control(
        "ACCOUNT-4821",
        Category::Identifier(IdentifierKind::Account),
    )
    .await;
}

#[tokio::test]
async fn reviewed_unicode_connector_display_project() {
    connector_display_control(
        "PROJECT-4821",
        Category::Identifier(IdentifierKind::Project),
    )
    .await;
}

#[tokio::test]
async fn reviewed_unicode_connector_file_email() {
    connector_file_control("alice@example.test", Category::Email).await;
}

#[tokio::test]
async fn reviewed_unicode_connector_file_telephone() {
    connector_file_control("1234567", Category::Telephone).await;
}

#[tokio::test]
async fn reviewed_unicode_connector_file_customer() {
    connector_file_control(
        "CUSTOMER-4821",
        Category::Identifier(IdentifierKind::Customer),
    )
    .await;
}

#[tokio::test]
async fn reviewed_unicode_connector_file_account() {
    connector_file_control(
        "ACCOUNT-4821",
        Category::Identifier(IdentifierKind::Account),
    )
    .await;
}

#[tokio::test]
async fn reviewed_unicode_connector_file_project() {
    connector_file_control(
        "PROJECT-4821",
        Category::Identifier(IdentifierKind::Project),
    )
    .await;
}
