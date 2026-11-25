//! Admission tests over an actual vault, an actual send guard and fixed host
//! reply rules. No detector gadget runs here; the source-built detector is
//! exercised by the marked native fixture in the gadget tests.

use super::*;
use crate::privacy_hook::tests::{EMAIL, LiteralSource, PHONE};
use crate::privacy_hook::{MAX_TEXT_BYTES, ReplyRefusal};
use crate::privacy_vault::{HostKey, HostKeySource, Limits, VaultLocation};
use std::sync::atomic::AtomicUsize;

struct SyntheticKey;

impl HostKeySource for SyntheticKey {
    fn acquire_privacy_key(&self) -> crate::privacy_vault::Result<HostKey> {
        Ok(HostKey::supplied_by_host(Zeroizing::new([7; 32])))
    }
}

pub(super) async fn vault(root: &tempfile::TempDir) -> Vault {
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

pub(super) fn rules() -> Arc<LiteralSource> {
    LiteralSource::new(&[(EMAIL, "email"), (PHONE, "phone")])
}

async fn session(vault: &Vault, source: &Arc<LiteralSource>) -> PrivacySession {
    PrivacySession::open(
        vault,
        FreshConversationBinding::reserve(&std::env::temp_dir()).unwrap(),
        Detector::from_test_source(source.clone()),
    )
    .await
    .unwrap()
}

pub(super) fn draft(text: &str) -> CapturedRaw {
    CapturedRaw::from_producer(RawOrigin::TerminalDraft, text)
}

fn fact(text: &str) -> CapturedRaw {
    CapturedRaw::from_producer(RawOrigin::ToolResult, text)
}

fn body(text: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({"messages": [{"role": "user", "content": text}]}))
        .unwrap()
}

/// Counts the vault commits one session performs.
fn commits(session: &PrivacySession) -> Arc<AtomicUsize> {
    let count = Arc::new(AtomicUsize::new(0));
    session.after_commit({
        let count = count.clone();
        move || {
            count.fetch_add(1, Ordering::SeqCst);
        }
    });
    count
}

#[tokio::test]
async fn a_node_exists_only_after_the_vault_commit_and_guard_admission_of_its_capture() {
    let root = tempfile::tempdir().unwrap();
    let vault = vault(&root).await;
    let source = rules();
    let session = session(&vault, &source).await;
    let guard = session.send_guard();
    let committed = commits(&session);
    // Before any admission the guard knows no original.
    assert!(guard.seal(body(EMAIL), &[]).is_ok());

    let text = format!("中文🦀 write to {EMAIL} or call {PHONE}; again {EMAIL}.");
    let raw = draft(&text);
    let node = session.admit_raw(&raw).await.unwrap();
    let view = node.model_view().unwrap().to_owned();
    assert_eq!(node.replacements(), 3);
    assert!(node.admitted_by(&session) && node.admits(&raw));
    assert_eq!(node.origin(), RawOrigin::TerminalDraft);
    assert!(!view.contains(EMAIL) && !view.contains(PHONE));
    assert!(view.starts_with("中文🦀 write to pv1-") && view.ends_with('.'));
    assert_eq!(view.matches("@privacy.invalid").count(), 2);
    assert_eq!(
        format!("{node:?} {raw:?}"),
        "AdmittedNode([private]) CapturedRaw([private])"
    );
    assert_eq!((source.calls(), committed.load(Ordering::SeqCst)), (1, 1));
    assert_eq!(session.known_values().await, 2);
    // The node carries the vault's own records for exactly its placeholders.
    let placeholders = node
        .with_records(|_, records| {
            (0..records.len().unwrap())
                .map(|index| {
                    records
                        .with_host_field(index, |field| field.known_alias().to_owned())
                        .unwrap()
                })
                .collect::<Vec<_>>()
        })
        .unwrap();
    assert_eq!(placeholders.len(), 2);
    assert!(
        placeholders
            .iter()
            .all(|placeholder| view.contains(placeholder))
    );

    // The guard admitted both originals: it refuses either one and seals the
    // view.
    assert_eq!(
        guard.seal(body(EMAIL), &[]).err(),
        Some(SendRefusal::KnownOriginal)
    );
    assert_eq!(
        guard.seal(body(&text), &[]).err(),
        Some(SendRefusal::KnownOriginal)
    );
    assert!(guard.seal(body(&view), &[]).is_ok());

    // A capture with nothing to report, and an empty one, still go through
    // the detector and the vault's own commit before a node exists.
    for clean in ["nothing private here", ""] {
        let raw = fact(clean);
        let node = session.admit_raw(&raw).await.unwrap();
        assert_eq!(
            (node.model_view().unwrap(), node.replacements()),
            (clean, 0)
        );
        assert!(node.admits(&raw));
        assert_eq!(
            node.with_records(|_, records| records.len().unwrap())
                .unwrap(),
            0
        );
    }
    assert_eq!((source.calls(), committed.load(Ordering::SeqCst)), (3, 3));
    assert_eq!(session.known_values().await, 2);
    // Another capture is another source, even with equal bytes.
    assert!(!node.admits(&draft(&text)));
}

#[tokio::test]
async fn a_repeated_value_reuses_its_placeholder_through_a_new_commit() {
    let root = tempfile::tempdir().unwrap();
    let vault = vault(&root).await;
    let source = rules();
    let session = session(&vault, &source).await;
    let committed = commits(&session);
    let first = session
        .admit_raw(&draft(&format!("a {EMAIL}")))
        .await
        .unwrap();
    let again = session
        .admit_raw(&fact(&format!("b {EMAIL} c {PHONE}")))
        .await
        .unwrap();
    let placeholder = first
        .model_view()
        .unwrap()
        .strip_prefix("a ")
        .unwrap()
        .to_owned();
    assert!(
        again
            .model_view()
            .unwrap()
            .starts_with(&format!("b {placeholder} c "))
    );
    assert_eq!(session.known_values().await, 2);
    let third = session
        .admit_raw(&fact(&format!("{PHONE} {EMAIL}")))
        .await
        .unwrap();
    assert!(third.model_view().unwrap().ends_with(&placeholder));
    // Three captures, three commits, two mappings; each node is its own.
    assert_eq!(
        (
            committed.load(Ordering::SeqCst),
            session.known_values().await
        ),
        (3, 2)
    );
    assert_eq!(
        third
            .with_records(|_, records| records.len().unwrap())
            .unwrap(),
        2
    );
}

#[tokio::test]
async fn the_bytes_of_an_admitted_view_are_raw_again_when_they_arrive_as_a_new_capture() {
    let root = tempfile::tempdir().unwrap();
    let vault = vault(&root).await;
    let source = rules();
    let session = session(&vault, &source).await;
    let node = session
        .admit_raw(&draft(&format!("mail {EMAIL} call {PHONE}")))
        .await
        .unwrap();
    let view = node.model_view().unwrap().to_owned();
    let (mail, phone) = view
        .strip_prefix("mail ")
        .unwrap()
        .split_once(" call ")
        .map(|(mail, phone)| (mail.to_owned(), phone.to_owned()))
        .unwrap();
    let calls = source.calls();

    // The existing node can be used again. The same bytes as a new capture
    // are refused: a marked placeholder, and a telephone number that equals
    // an issued telephone placeholder. Nothing is restored and masked again.
    for text in [
        view.clone(),
        format!("forward {mail}"),
        format!("my number is {phone}"),
    ] {
        assert_eq!(
            session.admit_raw(&draft(&text)).await.err(),
            Some(AdmitError::Refused(AdmitRefusal::IssuedPlaceholder)),
            "{text}"
        );
    }
    // Decided before the detector and the vault; the session continues.
    assert_eq!((source.calls(), session.known_values().await), (calls, 2));
    assert_eq!(node.model_view().unwrap(), view);
    assert!(session.admit_raw(&draft("unrelated")).await.is_ok());

    // The same condition on an accepted fact cannot be refused: the session
    // stops, and the fact is not replaced by anything.
    let returned = fact(&format!("tool printed {mail}"));
    assert_eq!(
        session.admit_raw(&returned).await.err(),
        Some(AdmitError::Stopped(SessionStop::Projection(
            AdmitRefusal::IssuedPlaceholder
        )))
    );
    assert_eq!(returned.origin(), RawOrigin::ToolResult);
    assert_eq!(session.check(), Err(SessionStop::Closed));
    assert_eq!(node.model_view(), Err(SessionStop::Closed));
}

#[tokio::test]
async fn a_draft_the_vault_would_refuse_is_refused_first_and_the_session_continues() {
    let root = tempfile::tempdir().unwrap();
    let vault = vault(&root).await;
    let source = rules();
    let session = session(&vault, &source).await;
    let committed = commits(&session);
    assert!(
        session
            .admit_raw(&draft(&format!("mail {EMAIL}")))
            .await
            .is_ok()
    );

    // A known value occurs where neither the floor nor the detector sees an
    // address, so it would go out unreported.
    source.set_rules(&[]);
    assert_eq!(
        session
            .admit_raw(&draft(&format!("see 中{EMAIL}")))
            .await
            .err(),
        Some(AdmitError::Refused(AdmitRefusal::UncoveredValue))
    );
    source.set_rules(&[(EMAIL, "email"), (PHONE, "phone")]);

    // Whole-text bounds, decided without a detector call.
    let calls = source.calls();
    assert_eq!(
        session
            .admit_raw(&draft(&"x".repeat(MAX_TEXT_BYTES + 1)))
            .await
            .err(),
        Some(AdmitError::Refused(AdmitRefusal::TooLarge))
    );
    assert_eq!(source.calls(), calls);
    // A line that cannot be split safely, and a view that could not fit.
    assert_eq!(
        session
            .admit_raw(&draft(&"x".repeat(70 * 1024)))
            .await
            .err(),
        Some(AdmitError::Refused(AdmitRefusal::TooLarge))
    );
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
        session.admit_raw(&draft(&padded)).await.err(),
        Some(AdmitError::Refused(AdmitRefusal::TooLarge))
    );

    // None of these reached the vault or ended the session.
    assert_eq!(
        (
            committed.load(Ordering::SeqCst),
            session.known_values().await
        ),
        (1, 1)
    );
    assert!(
        session
            .admit_raw(&draft(&format!("{EMAIL} {PHONE}")))
            .await
            .is_ok()
    );
    assert_eq!(session.known_values().await, 2);
}

#[tokio::test]
async fn every_detector_failure_stops_the_session_and_releases_nothing() {
    let root = tempfile::tempdir().unwrap();
    let vault = vault(&root).await;
    let text = format!("call {PHONE} then {PHONE}");

    // No result, an unapproved source, an omission and a partial report.
    for (failure, first_only, rules, expected) in [
        (
            Some(HookError::Execution),
            false,
            true,
            HookError::Execution,
        ),
        (Some(HookError::Source), false, true, HookError::Source),
        (None, false, false, HookError::Reply(ReplyRefusal::Coverage)),
        (None, true, true, HookError::Reply(ReplyRefusal::Coverage)),
    ] {
        let source = rules_for(rules);
        let session = session(&vault, &source).await;
        let guard = session.send_guard();
        let committed = commits(&session);
        let before = session.admit_raw(&draft("before")).await.unwrap();
        source.fail(failure);
        source.first_only(first_only);
        assert_eq!(
            session.admit_raw(&draft(&text)).await.err(),
            Some(AdmitError::Stopped(SessionStop::Detector(expected)))
        );
        // Stopped: no commit for that text, no later admission, no readable
        // view, no send.
        source.fail(None);
        assert_eq!(committed.load(Ordering::SeqCst), 1);
        assert_eq!(
            session.admit_raw(&draft("anything")).await.err(),
            Some(AdmitError::Stopped(SessionStop::Closed))
        );
        assert_eq!(session.check(), Err(SessionStop::Closed));
        assert_eq!(before.model_view(), Err(SessionStop::Closed));
        assert_eq!(
            guard.seal(body("clean"), &[]).err(),
            Some(SendRefusal::Closed)
        );
    }

    // A reply that reports text which is not one supported value.
    let source = LiteralSource::new(&[("call +886 912 345 678", "phone")]);
    let session = session(&vault, &source).await;
    assert_eq!(
        session
            .admit_raw(&draft(&format!("call {PHONE}; x")))
            .await
            .err(),
        Some(AdmitError::Stopped(SessionStop::Detector(
            HookError::Reply(ReplyRefusal::Category)
        )))
    );
}

fn rules_for(report: bool) -> Arc<LiteralSource> {
    if report {
        rules()
    } else {
        LiteralSource::new(&[])
    }
}

#[tokio::test]
async fn an_accepted_fact_without_a_view_stops_the_session_and_is_not_replaced() {
    let root = tempfile::tempdir().unwrap();
    let vault = vault(&root).await;
    let source = rules();
    let session = session(&vault, &source).await;
    let committed = commits(&session);
    let earlier = session
        .admit_raw(&fact(&format!("mail {EMAIL}")))
        .await
        .unwrap();
    // A returned result that is too large to admit as one unit.
    let large = "r".repeat(MAX_TEXT_BYTES + 1);
    let returned = CapturedRaw::from_producer(RawOrigin::ToolResult, &large);
    assert!(returned.is_accepted_fact() && !draft("x").is_accepted_fact());
    assert_eq!(
        session.admit_raw(&returned).await.err(),
        Some(AdmitError::Stopped(SessionStop::Projection(
            AdmitRefusal::TooLarge
        )))
    );
    // The capture is what the producer took in; the session offers no way to
    // change or substitute it. Future work is closed.
    assert_eq!(returned.origin(), RawOrigin::ToolResult);
    assert_eq!(committed.load(Ordering::SeqCst), 1);
    assert_eq!(session.check(), Err(SessionStop::Closed));
    assert_eq!(earlier.model_view(), Err(SessionStop::Closed));
    for origin in [
        RawOrigin::HumanReply,
        RawOrigin::HostFile,
        RawOrigin::HostConfig,
        RawOrigin::HostStatic,
    ] {
        assert!(CapturedRaw::from_producer(origin, "x").is_accepted_fact());
    }
}

#[tokio::test]
async fn a_vault_or_guard_failure_releases_nothing_and_ends_the_session() {
    let root = tempfile::tempdir().unwrap();
    let vault = vault(&root).await;

    // The guard closes after the vault has committed the mapping.
    let source = rules();
    let first = session(&vault, &source).await;
    let committed = commits(&first);
    let guard = first.send_guard();
    first.after_commit({
        let (guard, committed) = (guard.clone(), committed.clone());
        move || {
            committed.fetch_add(1, Ordering::SeqCst);
            guard.close();
        }
    });
    assert_eq!(
        first
            .admit_raw(&draft(&format!("mail {EMAIL}")))
            .await
            .err(),
        Some(AdmitError::Stopped(SessionStop::Guard(SendRefusal::Closed)))
    );
    // The commit returned, yet no node exists for it.
    assert_eq!(committed.load(Ordering::SeqCst), 1);
    assert_eq!(first.check(), Err(SessionStop::Closed));

    // The scope closes while the detector is running.
    let source = rules();
    let second = session(&vault, &source).await;
    let node = second
        .admit_raw(&draft(&format!("call {PHONE}")))
        .await
        .unwrap();
    source.during({
        let session = second.inner.session.clone();
        move || session.close()
    });
    assert_eq!(
        second
            .admit_raw(&draft(&format!("mail {EMAIL}")))
            .await
            .err(),
        Some(AdmitError::Stopped(SessionStop::Closed))
    );
    assert_eq!(node.model_view(), Err(SessionStop::Closed));

    // The whole vault closes: no admission and no new session.
    let source = rules();
    let third = session(&vault, &source).await;
    vault.close();
    assert_eq!(
        third
            .admit_raw(&draft(&format!("mail {EMAIL}")))
            .await
            .err(),
        Some(AdmitError::Stopped(SessionStop::Closed))
    );
    assert_eq!(source.calls(), 0);
    assert!(matches!(
        PrivacySession::open(
            &vault,
            FreshConversationBinding::reserve(&std::env::temp_dir()).unwrap(),
            Detector::from_test_source(source.clone()),
        )
        .await
        .err(),
        Some(SessionStop::Vault(_))
    ));
}

#[tokio::test]
async fn each_reserved_conversation_has_its_own_thread_scope_and_node_identity() {
    let root = tempfile::tempdir().unwrap();
    let vault = vault(&root).await;
    let source = rules();
    // The binding carries the thread id of the application's own allocator.
    let binding = FreshConversationBinding::reserve(&std::env::temp_dir()).unwrap();
    let reserved = binding.thread_id().to_owned();
    assert!(!reserved.is_empty() && reserved != LibsqlCheckpointer::new_thread_id());
    let one = PrivacySession::open(&vault, binding, Detector::from_test_source(source.clone()))
        .await
        .unwrap();
    assert_eq!(one.thread_id(), reserved);
    let two = session(&vault, &source).await;
    assert_ne!(one.thread_id(), two.thread_id());

    let raw = draft(&format!("mail {EMAIL}"));
    let first = one.admit_raw(&raw).await.unwrap();
    let second = two.admit_raw(&raw).await.unwrap();
    // The same capture in two conversations gets two placeholders and nodes.
    assert_ne!(first.model_view().unwrap(), second.model_view().unwrap());
    assert!(first.admitted_by(&one) && !first.admitted_by(&two));
    assert!(second.admitted_by(&two) && !second.admitted_by(&one));
    assert_eq!((one.known_values().await, two.known_values().await), (1, 1));
    // Each guard knows only its own conversation's originals.
    assert!(one.send_guard().seal(body(PHONE), &[]).is_ok());
    assert_eq!(
        two.send_guard().seal(body(EMAIL), &[]).err(),
        Some(SendRefusal::KnownOriginal)
    );
    // Stopping one conversation leaves the other usable.
    one.stop();
    assert_eq!(first.model_view(), Err(SessionStop::Closed));
    assert!(
        two.admit_raw(&draft(&format!("call {PHONE}")))
            .await
            .is_ok()
    );
    // A relative or missing workspace cannot be bound.
    for workspace in ["relative", "/nonexistent-craftsman-privacy-workspace"] {
        assert_eq!(
            FreshConversationBinding::reserve(Path::new(workspace)).err(),
            Some(SessionStop::Scope)
        );
    }
    drop(two);
    assert_eq!(second.model_view(), Err(SessionStop::Closed));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn admissions_started_together_run_one_at_a_time_and_a_full_owner_turns_drafts_away() {
    let root = tempfile::tempdir().unwrap();
    let vault = vault(&root).await;
    let source = rules();
    let session = Arc::new(session(&vault, &source).await);
    let (first, second) = (
        draft(&format!("mail {EMAIL}")),
        draft(&format!("call {PHONE}")),
    );
    let (one, two, three) = tokio::join!(
        session.admit_raw(&first),
        session.admit_raw(&second),
        session.admit_raw(&first),
    );
    // The vault has one operation slot and closes a session that finds it
    // busy; all three complete because the owner serializes them.
    assert_eq!(
        one.unwrap().model_view().unwrap(),
        three.unwrap().model_view().unwrap()
    );
    assert!(two.is_ok());
    assert_eq!(session.known_values().await, 2);

    // Hold the owner inside one detector call and fill every slot.
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    source.gate(gate.clone());
    let admitted = Arc::new(AtomicUsize::new(0));
    let mut waiting = Vec::new();
    for index in 0..ADMISSION_SLOTS {
        let session = session.clone();
        let admitted = admitted.clone();
        // Spawned as the erased future type the runtime's task code already
        // exists for; each admission counts itself when it succeeds.
        let admission: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> =
            Box::pin(async move {
                if session
                    .admit_raw(&draft(&format!("queued {index}")))
                    .await
                    .is_ok()
                {
                    admitted.fetch_add(1, Ordering::SeqCst);
                }
            });
        waiting.push(tokio::spawn(admission));
    }
    while session.inner.slots.available_permits() > 0 {
        tokio::task::yield_now().await;
    }
    // One more draft is turned away at entry; it never waited.
    assert_eq!(
        session.admit_raw(&draft("one too many")).await.err(),
        Some(AdmitError::Refused(AdmitRefusal::Waiting))
    );
    gate.add_permits(ADMISSION_SLOTS);
    for task in waiting {
        task.await.unwrap();
    }
    // Every queued admission succeeded.
    assert_eq!(admitted.load(Ordering::SeqCst), ADMISSION_SLOTS);
    assert!(session.check().is_ok());
}

#[tokio::test]
async fn the_scope_capacity_is_a_refusal_of_a_draft_before_any_effect() {
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
    let committed = commits(&session);
    for batch in emails[..SCOPE_MAPPINGS].chunks(128) {
        let node = session.admit_raw(&draft(&batch.join("\n"))).await.unwrap();
        assert_eq!(node.replacements(), batch.len());
    }
    assert_eq!(session.known_values().await, SCOPE_MAPPINGS);
    // One more new value does not fit. Nothing is pruned, raised or saved.
    assert_eq!(
        session
            .admit_raw(&draft(&format!("new {}", emails[SCOPE_MAPPINGS])))
            .await
            .err(),
        Some(AdmitError::Refused(AdmitRefusal::ScopeFull))
    );
    assert_eq!(
        (
            session.known_values().await,
            committed.load(Ordering::SeqCst)
        ),
        (SCOPE_MAPPINGS, 4)
    );
    // Known values are still admitted, and the guard holds every original.
    let known = session
        .admit_raw(&draft(&format!("old {}", emails[7])))
        .await
        .unwrap();
    assert!(!known.model_view().unwrap().contains(&emails[7]));
    assert_eq!(
        session.send_guard().seal(body(&emails[511]), &[]).err(),
        Some(SendRefusal::KnownOriginal)
    );
}
