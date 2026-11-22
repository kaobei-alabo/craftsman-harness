//! Admission tests over an actual vault, an actual send guard and fixed host
//! reply rules. No detector gadget runs here; the source-built detector is
//! exercised by the marked native fixture in the gadget tests.

use super::*;
use crate::privacy_hook::tests::{EMAIL, LiteralSource, PHONE};
use crate::privacy_hook::{MAX_TEXT_BYTES, ReplyRefusal};
use crate::privacy_vault::{HostKey, HostKeySource, Limits, VaultLocation};

struct SyntheticKey;

impl HostKeySource for SyntheticKey {
    fn acquire_privacy_key(&self) -> crate::privacy_vault::Result<HostKey> {
        Ok(HostKey::supplied_by_host(Zeroizing::new([7; 32])))
    }
}

async fn vault(root: &tempfile::TempDir) -> Vault {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    Vault::open(
        VaultLocation::under_host_config_root(&std::fs::canonicalize(root.path()).unwrap())
            .unwrap(),
        &SyntheticKey,
        Limits::default(),
    )
    .await
    .unwrap()
}

fn rules() -> Arc<LiteralSource> {
    LiteralSource::new(&[(EMAIL, "email"), (PHONE, "phone")])
}

async fn session(vault: &Vault, source: &Arc<LiteralSource>) -> PrivacySession {
    let workspace = std::env::temp_dir();
    PrivacySession::open(
        vault,
        ConversationScope::fresh(&workspace).unwrap(),
        Detector::from_test_source(source.clone()),
    )
    .await
    .unwrap()
}

async fn mappings(session: &PrivacySession) -> usize {
    session
        .with_scope_records(|_, records| records.len().unwrap())
        .await
        .unwrap()
}

fn body(text: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({"messages": [{"role": "user", "content": text}]}))
        .unwrap()
}

#[tokio::test]
async fn reported_values_are_saved_and_guarded_before_the_text_is_released() {
    let root = tempfile::tempdir().unwrap();
    let vault = vault(&root).await;
    let source = rules();
    let session = session(&vault, &source).await;
    let guard = session.send_guard();
    assert_eq!(mappings(&session).await, 0);
    // Before any admission the guard knows no original.
    assert!(guard.seal(body(EMAIL), &[]).is_ok());

    let raw = format!("中文🦀 write to {EMAIL} or call {PHONE}; again {EMAIL}.");
    let admitted = session.admit_raw(&raw).await.unwrap();
    let text = admitted.text().unwrap().to_owned();
    assert_eq!(admitted.replacements(), 3);
    assert!(admitted.admitted_by(&session));
    assert!(!text.contains(EMAIL) && !text.contains(PHONE));
    assert!(text.starts_with("中文🦀 write to pv1-") && text.ends_with('.'));
    assert_eq!(text.matches("@privacy.invalid").count(), 2);
    assert_eq!(format!("{admitted:?}"), "AdmittedText([private])");
    assert_eq!(source.calls(), 1);

    // Two mappings exist in the scope, with the placeholders the text shows.
    assert_eq!(mappings(&session).await, 2);
    let placeholders = session
        .with_scope_records(|_, records| {
            (0..records.len().unwrap())
                .map(|index| {
                    records
                        .with_host_field(index, |field| field.known_alias().to_owned())
                        .unwrap()
                })
                .collect::<Vec<_>>()
        })
        .await
        .unwrap();
    assert!(
        placeholders
            .iter()
            .all(|placeholder| text.contains(placeholder))
    );

    // The guard admitted both originals: it now refuses either one and still
    // seals the admitted text.
    assert_eq!(
        guard.seal(body(EMAIL), &[]).err(),
        Some(SendRefusal::KnownOriginal)
    );
    assert_eq!(
        guard.seal(body(&raw), &[]).err(),
        Some(SendRefusal::KnownOriginal)
    );
    assert!(guard.seal(body(&text), &[]).is_ok());

    // An empty text needs no detector and no mapping.
    let empty = session.admit_raw("").await.unwrap();
    assert_eq!((empty.text().unwrap(), empty.replacements()), ("", 0));
    // Text without a reported value is released unchanged.
    let plain = session.admit_raw("nothing private here").await.unwrap();
    assert_eq!(plain.text().unwrap(), "nothing private here");
    assert_eq!((source.calls(), mappings(&session).await), (2, 2));
}

#[tokio::test]
async fn a_repeated_value_reuses_its_placeholder_and_adds_no_mapping() {
    let root = tempfile::tempdir().unwrap();
    let vault = vault(&root).await;
    let source = rules();
    let session = session(&vault, &source).await;
    let first = session.admit_raw(&format!("a {EMAIL}")).await.unwrap();
    let again = session
        .admit_raw(&format!("b {EMAIL} c {PHONE}"))
        .await
        .unwrap();
    let first = first.text().unwrap().strip_prefix("a ").unwrap().to_owned();
    let again = again.text().unwrap().to_owned();
    assert!(again.starts_with(&format!("b {first} c ")));
    assert_eq!(mappings(&session).await, 2);
    let third = session
        .admit_raw(&format!("{PHONE} {EMAIL}"))
        .await
        .unwrap();
    assert!(third.text().unwrap().ends_with(&first));
    assert_eq!(mappings(&session).await, 2);
}

#[tokio::test]
async fn raw_text_with_an_issued_placeholder_is_refused_before_any_effect() {
    let root = tempfile::tempdir().unwrap();
    let vault = vault(&root).await;
    let source = rules();
    let session = session(&vault, &source).await;
    let admitted = session.admit_raw(&format!("mail {EMAIL}")).await.unwrap();
    let placeholder = admitted
        .text()
        .unwrap()
        .strip_prefix("mail ")
        .unwrap()
        .to_owned();
    let calls = source.calls();

    // Admitted text is not raw text: sending it back through is refused, with
    // or without a new value beside it. It is never restored and re-masked.
    for raw in [
        admitted.text().unwrap().to_owned(),
        format!("forward {placeholder} to {PHONE}"),
    ] {
        assert_eq!(
            session.admit_raw(&raw).await.err(),
            Some(AdmitError::Refused(AdmitRefusal::IssuedPlaceholder))
        );
    }
    // No detector call, no mapping, and the session is still usable.
    assert_eq!((source.calls(), mappings(&session).await), (calls, 1));
    assert!(session.check().is_ok());
    assert!(session.admit_raw(&format!("call {PHONE}")).await.is_ok());
    assert_eq!(mappings(&session).await, 2);
}

#[tokio::test]
async fn content_the_vault_would_refuse_is_refused_first_and_nothing_is_saved() {
    let root = tempfile::tempdir().unwrap();
    let vault = vault(&root).await;
    let source = rules();
    let session = session(&vault, &source).await;
    assert!(session.admit_raw(&format!("mail {EMAIL}")).await.is_ok());

    // The detector stops reporting a value the scope already knows.
    source.set_rules(&[(PHONE, "phone")]);
    for raw in [format!("again {EMAIL}"), format!("{PHONE} and {EMAIL}")] {
        assert_eq!(
            session.admit_raw(&raw).await.err(),
            Some(AdmitError::Refused(AdmitRefusal::UncoveredValue))
        );
    }
    // It reports one occurrence of a new value and misses another.
    source.first_only(true);
    assert_eq!(
        session
            .admit_raw(&format!("{PHONE} then {PHONE}"))
            .await
            .err(),
        Some(AdmitError::Refused(AdmitRefusal::UncoveredValue))
    );
    source.first_only(false);
    // It reports text that is not one supported value.
    for literal in ["call +886 912 345 678", "+886 912 345 678;"] {
        source.set_rules(&[(literal, "phone")]);
        assert_eq!(
            session.admit_raw(&format!("call {PHONE}; x")).await.err(),
            Some(AdmitError::Detector(HookError::Reply(
                ReplyRefusal::Category
            )))
        );
    }

    // Whole-text bounds, decided without a detector call.
    let calls = source.calls();
    assert_eq!(
        session
            .admit_raw(&"x".repeat(MAX_TEXT_BYTES + 1))
            .await
            .err(),
        Some(AdmitError::Refused(AdmitRefusal::TooLarge))
    );
    assert_eq!(source.calls(), calls);
    // A line that cannot be split safely, and a result that could not fit.
    assert_eq!(
        session.admit_raw(&"x".repeat(70 * 1024)).await.err(),
        Some(AdmitError::Refused(AdmitRefusal::TooLarge))
    );
    source.set_rules(&[(EMAIL, "email")]);
    let dense = format!(
        "{}\n{}",
        "y".repeat(60 * 1024),
        format!("{EMAIL}\n").repeat(100)
    );
    let padded = format!(
        "{dense}{}",
        "z\n".repeat((MAX_TEXT_BYTES - dense.len()) / 2)
    );
    assert_eq!(
        session.admit_raw(&padded).await.err(),
        Some(AdmitError::Refused(AdmitRefusal::TooLarge))
    );

    // None of these saved a mapping or ended the session.
    assert_eq!(mappings(&session).await, 1);
    source.set_rules(&[(EMAIL, "email"), (PHONE, "phone")]);
    assert!(session.admit_raw(&format!("{EMAIL} {PHONE}")).await.is_ok());
    assert_eq!(mappings(&session).await, 2);
}

#[tokio::test]
async fn a_detector_result_failure_keeps_the_session_and_a_source_failure_ends_it() {
    let root = tempfile::tempdir().unwrap();
    let vault = vault(&root).await;
    let source = rules();
    let session = session(&vault, &source).await;
    let guard = session.send_guard();
    let admitted = session.admit_raw(&format!("mail {EMAIL}")).await.unwrap();

    source.fail(Some(HookError::Execution));
    assert_eq!(
        session.admit_raw(&format!("call {PHONE}")).await.err(),
        Some(AdmitError::Detector(HookError::Execution))
    );
    assert!(session.check().is_ok() && admitted.text().is_ok());
    assert_eq!(mappings(&session).await, 1);

    source.fail(Some(HookError::Source));
    assert_eq!(
        session.admit_raw(&format!("call {PHONE}")).await.err(),
        Some(AdmitError::Stopped(SessionStop::Detector))
    );
    // Stopped: no admission, no readable admitted text, no send.
    source.fail(None);
    assert_eq!(
        session.admit_raw("anything").await.err(),
        Some(AdmitError::Stopped(SessionStop::Closed))
    );
    assert_eq!(session.check(), Err(SessionStop::Closed));
    assert_eq!(admitted.text(), Err(SessionStop::Closed));
    assert_eq!(
        guard.seal(body("clean"), &[]).err(),
        Some(SendRefusal::Closed)
    );
    assert!(session.with_scope_records(|_, _| ()).await.is_err());
}

#[tokio::test]
async fn a_vault_or_guard_failure_releases_nothing_and_ends_the_session() {
    let root = tempfile::tempdir().unwrap();
    let vault = vault(&root).await;

    // The guard closes after the vault has committed the mapping.
    let source = rules();
    let labels = ("a".repeat(64), "b".repeat(32));
    let scope = ConversationScope::labelled(&labels.0, &labels.1).unwrap();
    let first = PrivacySession::open(&vault, scope, Detector::from_test_source(source.clone()))
        .await
        .unwrap();
    let guard = first.send_guard();
    first.after_commit({
        let guard = guard.clone();
        move || guard.close()
    });
    assert_eq!(
        first.admit_raw(&format!("mail {EMAIL}")).await.err(),
        Some(AdmitError::Stopped(SessionStop::Guard(SendRefusal::Closed)))
    );
    assert_eq!(first.check(), Err(SessionStop::Closed));
    // The mapping was saved, yet no admitted text exists for it.
    let scope = ConversationScope::labelled(&labels.0, &labels.1).unwrap();
    let reopened = vault.session(scope.namespace).unwrap();
    assert_eq!(reopened.scope_records().await.unwrap().len().unwrap(), 1);
    reopened.close();

    // The vault closes while the detector is running.
    let source = rules();
    let second = session(&vault, &source).await;
    let admitted = second.admit_raw(&format!("call {PHONE}")).await.unwrap();
    source.during({
        let session = second.inner.session.clone();
        move || session.close()
    });
    assert_eq!(
        second.admit_raw(&format!("mail {EMAIL}")).await.err(),
        Some(AdmitError::Stopped(SessionStop::Closed))
    );
    assert_eq!(admitted.text(), Err(SessionStop::Closed));

    // The whole vault closes: a later session cannot be opened or used.
    let source = rules();
    let third = session(&vault, &source).await;
    vault.close();
    assert_eq!(
        third.admit_raw(&format!("mail {EMAIL}")).await.err(),
        Some(AdmitError::Stopped(SessionStop::Closed))
    );
    assert_eq!(source.calls(), 0);
    assert!(matches!(
        PrivacySession::open(
            &vault,
            ConversationScope::fresh(&std::env::temp_dir()).unwrap(),
            Detector::from_test_source(source.clone()),
        )
        .await
        .err(),
        Some(SessionStop::Vault(_))
    ));
}

#[tokio::test]
async fn each_conversation_has_its_own_scope_and_admitted_text_identity() {
    let root = tempfile::tempdir().unwrap();
    let vault = vault(&root).await;
    let source = rules();
    let one = session(&vault, &source).await;
    let two = session(&vault, &source).await;
    let raw = format!("mail {EMAIL}");
    let first = one.admit_raw(&raw).await.unwrap();
    let second = two.admit_raw(&raw).await.unwrap();
    // The same value in two conversations gets two placeholders.
    assert_ne!(first.text().unwrap(), second.text().unwrap());
    assert!(first.admitted_by(&one) && !first.admitted_by(&two));
    assert!(second.admitted_by(&two) && !second.admitted_by(&one));
    assert_eq!((mappings(&one).await, mappings(&two).await), (1, 1));
    // Each guard knows only its own conversation's originals.
    assert!(one.send_guard().seal(body(PHONE), &[]).is_ok());
    assert_eq!(
        two.send_guard().seal(body(EMAIL), &[]).err(),
        Some(SendRefusal::KnownOriginal)
    );
    // Stopping one conversation leaves the other usable.
    one.stop();
    assert_eq!(first.text(), Err(SessionStop::Closed));
    assert!(two.admit_raw(&format!("call {PHONE}")).await.is_ok());
    // A relative workspace cannot name a scope.
    assert_eq!(
        ConversationScope::fresh(Path::new("relative")).err(),
        Some(SessionStop::Scope)
    );
    drop(two);
    assert_eq!(second.text(), Err(SessionStop::Closed));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn admissions_started_together_run_one_at_a_time() {
    let root = tempfile::tempdir().unwrap();
    let vault = vault(&root).await;
    let source = rules();
    let session = session(&vault, &source).await;
    let (first, second) = (format!("mail {EMAIL}"), format!("call {PHONE}"));
    let (one, two, three) = tokio::join!(
        session.admit_raw(&first),
        session.admit_raw(&second),
        session.admit_raw(&first),
    );
    // The vault has one operation slot and closes a session that finds it
    // busy; all three complete because the owner serializes them.
    assert!(one.is_ok() && two.is_ok() && three.is_ok());
    assert_eq!(one.unwrap().text().unwrap(), three.unwrap().text().unwrap());
    assert_eq!(mappings(&session).await, 2);
}

#[tokio::test]
async fn the_scope_capacity_is_a_refusal_before_any_effect() {
    let root = tempfile::tempdir().unwrap();
    let vault = vault(&root).await;
    let emails: Vec<String> = (0..SCOPE_MAPPINGS + 1)
        .map(|index| format!("u{index:03}@example.invalid"))
        .collect();
    let rules: Vec<(&str, &'static str)> = emails
        .iter()
        .map(|email| (email.as_str(), "email"))
        .collect();
    let source = LiteralSource::new(&rules);
    let session = session(&vault, &source).await;
    for batch in emails[..SCOPE_MAPPINGS].chunks(128) {
        let admitted = session.admit_raw(&batch.join("\n")).await.unwrap();
        assert_eq!(admitted.replacements(), batch.len());
    }
    assert_eq!(mappings(&session).await, SCOPE_MAPPINGS);
    // One more new value does not fit. Nothing is pruned, raised or saved.
    assert_eq!(
        session
            .admit_raw(&format!("new {}", emails[SCOPE_MAPPINGS]))
            .await
            .err(),
        Some(AdmitError::Refused(AdmitRefusal::ScopeFull))
    );
    assert_eq!(mappings(&session).await, SCOPE_MAPPINGS);
    // Known values are still admitted, and the guard holds every original.
    let known = session
        .admit_raw(&format!("old {}", emails[7]))
        .await
        .unwrap();
    assert!(!known.text().unwrap().contains(&emails[7]));
    assert_eq!(
        session.send_guard().seal(body(&emails[511]), &[]).err(),
        Some(SendRefusal::KnownOriginal)
    );
}
