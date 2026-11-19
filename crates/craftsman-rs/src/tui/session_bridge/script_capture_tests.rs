//! Marked fixtures mint selections inside actual UiBridge/AgentSlot ownership.
use super::*;
use crate::script_capture::{CaptureError, Operation, tests as proof};
use std::time::Duration;

pub(crate) async fn script_capture_fixture(marker: &proof::FixtureMarker) {
    let mut fixture = proof::fixture(marker).await;
    let approval = Arc::new(Mutex::new(ApprovalState::default()));
    let (tx, _events) = mpsc::unbounded_channel();
    let bridge = UiBridge { tx };
    if marker.case() == proof::Case::Rejection {
        rejection_case(&bridge, &fixture, marker).await;
        return;
    }
    let root = craftsman_harness::SessionCancellation::default();
    let mut slot = AgentSlot::spawn(&fixture.cfg, approval.clone(), bridge, root.clone());
    let _ = tokio::time::timeout(Duration::from_secs(30), slot.get())
        .await
        .unwrap();
    let session = slot.peek().expect("actual TUI builder initialized");
    assert_eq!(session.agent.durable_turn_checkpoints_enabled(), true);
    match marker.case() {
        proof::Case::Capture => proof::capture_case(session, &fixture).await,
        proof::Case::Policy => {
            proof::policy_case(session, &fixture).await;
            // Changing ordinary mutation policy never grants sensitive reads.
            approval.lock().unwrap().mode = crate::config::PermissionMode::BypassPermissions;
            assert_eq!(
                session
                    .script_capture
                    .as_ref()
                    .unwrap()
                    .capture(session, &["actual-settings/config.json"])
                    .await
                    .err(),
                Some(CaptureError::Denied)
            );
            approval.lock().unwrap().mode = crate::config::PermissionMode::Plan;
            let captures = session.script_capture.as_ref().unwrap();
            let originals = captures
                .capture(session, &["project-a/empty.bin"])
                .await
                .unwrap();
            let preview = captures
                .preview(
                    session,
                    &originals,
                    br#"{"changes":[{"target":"project-a/empty.bin","after":{"kind":"absent"}}]}"#,
                )
                .unwrap();
            assert_eq!(
                preview.operations(session).unwrap(),
                vec![("project-a/empty.bin".into(), Operation::Delete)]
            );
            fixture.originals_preserved();
        }
        proof::Case::Races => proof::races_case(session, &fixture).await,
        proof::Case::Boundaries => proof::boundaries_case(session, &mut fixture).await,
        proof::Case::Crud => proof::crud_case(session, &fixture).await,
        proof::Case::Bounds => proof::bounds_case(session, &fixture).await,
        proof::Case::Worker => proof::worker_case(session, &fixture).await,
        proof::Case::Lifetime => lifetime_case(&mut slot, &fixture, approval.clone()).await,
        proof::Case::None => {
            assert!(session.script_capture.is_none());
            assert!(SessionExtras::default().script_capture.is_none());
            proof::ordinary_turn(session, &fixture).await;
            crate::acp_serve::script_capture_none_fixture(&fixture).await;
        }
        _ => unreachable!("ACP and rejected construction use their own actual owner"),
    }
    slot.shutdown().await;
    if let Some(session) = slot.peek() {
        if let Some(capture) = &session.script_capture {
            assert_eq!(
                capture.capture(session, &["project-a/raw.bin"]).await.err(),
                Some(CaptureError::Revoked)
            );
        }
    }
    fixture.originals_preserved();
}

async fn lifetime_case(
    slot: &mut AgentSlot,
    fixture: &proof::Fixture,
    approval: Arc<Mutex<ApprovalState>>,
) {
    let session = slot.peek().unwrap();
    let originals = session
        .script_capture
        .as_ref()
        .unwrap()
        .capture(session, &["project-a/raw.bin"])
        .await
        .unwrap();
    let (tx, _) = mpsc::unbounded_channel();
    let mut equal = AgentSlot::spawn(
        &fixture.cfg,
        approval.clone(),
        UiBridge { tx },
        craftsman_harness::SessionCancellation::default(),
    );
    equal.get().await;
    let fresh = equal.peek().unwrap();
    assert_eq!(
        fresh.agent.state.lock().unwrap().messages,
        session.agent.state.lock().unwrap().messages
    );
    assert_eq!(
        originals.original_bytes(fresh, "project-a/raw.bin").err(),
        Some(CaptureError::Foreign)
    );
    assert_eq!(
        fresh
            .script_capture
            .as_ref()
            .unwrap()
            .preview(fresh, &originals, br#"{"changes":[]}"#)
            .err(),
        Some(CaptureError::Foreign)
    );
    equal.shutdown().await;
    proof::ordinary_turn(session, fixture).await;
    let thread = session.thread_id.clone();
    let session = slot.peek_mut().unwrap();
    let model = Arc::new(craftsman_harness::testing::ScriptedModel::new(vec![]));
    let foreign_agent = craftsman_harness::Agent::builder(model.clone())
        .name("main")
        .state(session.agent.state.clone())
        .build();
    let real_agent = std::mem::replace(&mut session.agent, foreign_agent);
    assert_eq!(
        originals.original_bytes(session, "project-a/raw.bin").err(),
        Some(CaptureError::Foreign),
        "same committed state/name alone is not the actual builder's Agent"
    );
    assert_eq!(
        session
            .script_capture
            .as_ref()
            .unwrap()
            .capture(session, &["project-a/raw.bin"])
            .await
            .err(),
        Some(CaptureError::Foreign)
    );
    session.agent = real_agent;
    assert!(model.requests.lock().unwrap().is_empty());
    session.set_thread_id("owned-capture-switched-thread");
    assert_eq!(
        originals.original_bytes(session, "project-a/raw.bin").err(),
        Some(CaptureError::Revoked)
    );
    let switched = session
        .script_capture
        .as_ref()
        .unwrap()
        .capture(session, &["project-a/raw.bin"])
        .await
        .unwrap();
    assert!(
        session.load_thread(&thread).await.unwrap(),
        "real stored thread loaded"
    );
    assert_eq!(
        switched.original_bytes(session, "project-a/raw.bin").err(),
        Some(CaptureError::Revoked)
    );
    let loaded = session
        .script_capture
        .as_ref()
        .unwrap()
        .capture(session, &["project-a/raw.bin"])
        .await
        .unwrap();
    let previous_messages = session.agent.state.lock().unwrap().messages.clone();
    let previous_usage = *session.usage.lock().unwrap();
    let mut changed = fixture.cfg.clone();
    changed.model = Some("capture-fixture-reconfigured".into());
    slot.rebuild(&changed, approval);
    slot.get().await;
    let rebuilt = slot.peek().unwrap();
    assert_eq!(
        rebuilt.agent.state.lock().unwrap().messages,
        previous_messages
    );
    assert_eq!(*rebuilt.usage.lock().unwrap(), previous_usage);
    assert_eq!(
        loaded.original_bytes(rebuilt, "project-a/raw.bin").err(),
        Some(CaptureError::Foreign)
    );
    let selected = rebuilt
        .script_capture
        .as_ref()
        .unwrap()
        .capture(rebuilt, &["project-a/raw.bin"])
        .await
        .unwrap();
    assert_eq!(
        selected
            .original_bytes(rebuilt, "project-a/raw.bin")
            .unwrap(),
        Some(&[0, 255, 128, b'\r', b'\n', b'Z'][..])
    );
    slot.cancellation().cancel(); // Actual selected TUI owner; no request_cancel proxy.
    assert_eq!(
        selected.original_bytes(rebuilt, "project-a/raw.bin").err(),
        Some(CaptureError::Revoked)
    );
    assert_eq!(
        rebuilt
            .script_capture
            .as_ref()
            .unwrap()
            .capture(rebuilt, &["project-a/raw.bin"])
            .await
            .err(),
        Some(CaptureError::Revoked)
    );
    fixture.config_preserved();
    assert_eq!(
        fixture.server.received_requests().await.unwrap().len(),
        2,
        "no transfer/load/rebuild replay"
    );
}

async fn rejection_case(
    bridge: &UiBridge,
    fixture: &proof::Fixture,
    marker: &proof::FixtureMarker,
) {
    let owner = bridge.capture_build_owner();
    let selection = owner.marked_selection(marker);
    let approval = Arc::new(Mutex::new(ApprovalState::default()));
    // The actual legacy/frozen/service entry has no construction owner.
    let ordinary = agent::build_session_ext(
        &fixture.cfg,
        approval.clone(),
        "",
        true,
        false,
        None,
        SessionExtras {
            script_capture: Some(selection),
            workspace_root: Some(fixture.root.clone()),
            token_sink: Some(Arc::new(|_| {})),
            ..Default::default()
        },
    )
    .await;
    assert!(ordinary.is_none());
    let first = bridge.capture_build_owner();
    let foreign = first.marked_selection(marker);
    let other = bridge.capture_build_owner();
    let refused = agent::build_owned_session_ext(
        &fixture.cfg,
        approval,
        "",
        false,
        false,
        None,
        SessionExtras {
            script_capture: Some(foreign),
            workspace_root: Some(fixture.root.clone()),
            ..Default::default()
        },
        other,
    )
    .await;
    assert!(
        refused.is_none(),
        "equal bridges/configs are different actual constructions"
    );
    agent::script_capture_reject_prepared_fixture(
        &fixture.root,
        &crate::config::config_dir(),
        owner.marked_selection(marker),
    )
    .await;
    assert!(
        !crate::config::config_dir().join("threads.db").exists(),
        "no store or journal before selection rejection"
    );
    assert!(!crate::config::config_dir().join("history.jsonl").exists());
    assert!(fixture.server.received_requests().await.unwrap().is_empty());
    fixture.originals_preserved();
}
