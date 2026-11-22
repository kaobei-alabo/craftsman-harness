use super::*;

#[test]
fn one_entry_deadline_is_not_refreshed_by_a_later_phase() {
    let entry = Instant::now();
    let deadline = entry + ADMISSION_BOUND;
    assert_eq!(admission_deadline(deadline, entry), Ok(()));
    assert_eq!(
        admission_deadline(deadline, deadline - Duration::from_nanos(1)),
        Ok(())
    );
    assert_eq!(
        admission_deadline(deadline, deadline),
        Err(SessionStop::Deadline)
    );
    assert_eq!(
        admission_deadline(deadline, deadline + Duration::from_secs(5)),
        Err(SessionStop::Deadline)
    );
}

#[tokio::test]
async fn the_actual_noop_map_return_is_retained_without_an_admitted_authority_factory() {
    let root = tempfile::tempdir().unwrap();
    let vault = tests::vault(&root).await;
    let source = crate::privacy_hook::tests::LiteralSource::new(&[]);
    let session = PrivacySession::open(
        &vault,
        FreshConversationBinding::reserve(root.path()).unwrap(),
        Detector::from_test_source(source),
    )
    .await
    .unwrap();
    let raw = tests::draft("plain source");
    let node = session.admit_raw(&raw).await.unwrap();
    assert_eq!(node.replacements(), 0);
    assert!(node.admits(&raw));
    assert!(session.inner.last_map_commit.lock().unwrap().is_some());
    // A pure expiry check; no clock/callback fault is inserted into admission.
    let deadline = Instant::now();
    assert_eq!(
        admission_deadline(deadline, deadline),
        Err(SessionStop::Deadline)
    );
    assert!(session.inner.last_map_commit.lock().unwrap().is_some());
}
