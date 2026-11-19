//! Actual ACP session selection retains its existing non-durable policy.
use super::*;
use crate::script_capture::{CaptureError, tests as proof};

pub(crate) async fn script_capture_fixture(marker: &proof::FixtureMarker) {
    let fixture = proof::fixture(marker).await;
    let (tx, _frames) = tokio::sync::mpsc::unbounded_channel();
    let mut state = AcpServerState::new(fixture.cfg.clone(), tx, None);
    state
        .create_session(
            "actual-capture-acp".into(),
            fixture.root.clone(),
            fixture.cfg.clone(),
        )
        .await
        .unwrap();
    let runtime = &state.current().unwrap().runtime;
    assert!(
        !runtime.agent.durable_turn_checkpoints_enabled(),
        "capture must not set ACP cancellation_session"
    );
    let originals = runtime
        .script_capture
        .as_ref()
        .unwrap()
        .capture(runtime, &["project-a/raw.bin", "project-a/create.bin"])
        .await
        .unwrap();
    assert_eq!(
        originals
            .original_bytes(runtime, "project-a/create.bin")
            .unwrap(),
        None
    );
    proof::ordinary_turn(runtime, &fixture).await;
    let prior_messages = runtime.agent.state.lock().unwrap().messages.clone();
    let prior_usage = *runtime.usage.lock().unwrap();
    set_model(&mut state, "capture-fixture-acp-reconfigured".into())
        .await
        .unwrap();
    let rebuilt = &state.current().unwrap().runtime;
    assert!(!rebuilt.agent.durable_turn_checkpoints_enabled());
    assert_eq!(rebuilt.agent.state.lock().unwrap().messages, prior_messages);
    assert_eq!(*rebuilt.usage.lock().unwrap(), prior_usage);
    assert_eq!(
        originals.original_bytes(rebuilt, "project-a/raw.bin").err(),
        Some(CaptureError::Foreign)
    );
    let captures = rebuilt
        .script_capture
        .as_ref()
        .unwrap()
        .capture(rebuilt, &["project-a/raw.bin"])
        .await
        .unwrap();
    assert_eq!(
        captures
            .original_bytes(rebuilt, "project-a/raw.bin")
            .unwrap(),
        Some(&[0, 255, 128, b'\r', b'\n', b'Z'][..])
    );
    state.close().await;
    let closed = &state.current().unwrap().runtime;
    assert_eq!(
        captures.original_bytes(closed, "project-a/raw.bin").err(),
        Some(CaptureError::Revoked)
    );
    assert_eq!(
        closed
            .script_capture
            .as_ref()
            .unwrap()
            .capture(closed, &["project-a/raw.bin"])
            .await
            .err(),
        Some(CaptureError::Revoked)
    );
    assert_eq!(fixture.server.received_requests().await.unwrap().len(), 2);
    fixture.originals_preserved();
}

pub(crate) async fn script_capture_none_fixture(fixture: &proof::Fixture) {
    let (tx, _frames) = tokio::sync::mpsc::unbounded_channel();
    let mut state = AcpServerState::new(fixture.cfg.clone(), tx, None);
    state
        .create_session(
            "actual-unselected-acp".into(),
            fixture.root.clone(),
            fixture.cfg.clone(),
        )
        .await
        .unwrap();
    let runtime = &state.current().unwrap().runtime;
    assert!(
        runtime.script_capture.is_none(),
        "no Config/model/MCP/history label can select a capture"
    );
    assert!(!runtime.agent.durable_turn_checkpoints_enabled());
    state.close().await;
    fixture.originals_preserved();
}
