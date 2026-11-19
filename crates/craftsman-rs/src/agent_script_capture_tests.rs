//! Foreign selections must fail in the actual typed private construction core.
use super::*;
use crate::exec::construction::{Options, Prepared, SessionSelection};
use std::path::Path;

pub(crate) async fn script_capture_reject_prepared_fixture(
    root: &Path,
    config: &Path,
    selection: FrontendCaptureSelection,
) {
    let prepared = Prepared::prepare_with_environment_for_tests(
        Options {
            workspace: root.to_path_buf(),
            config_dir: config.to_path_buf(),
            trust_workspace: false,
            session: SessionSelection::Ephemeral,
            selectors: Default::default(),
        },
        BTreeMap::new(),
    )
    .await
    .unwrap();
    let (original, _) = build_prepared_session(&prepared).await.unwrap();
    assert!(
        original.script_capture.is_none(),
        "actual None-policy private construction stays unselected"
    );
    assert!(!original.agent.durable_turn_checkpoints_enabled());
    assert!(
        original
            .agent
            .registered_tool_names()
            .all(|name| !name.contains("script"))
    );
    original.lsp.shutdown().await;
    let gate = Arc::new(crate::exec::runtime::InvocationGate::captured(&prepared));
    let meter = crate::exec::meter::Meter::from_scope(RequestMeterScope {
        gate: gate.clone(),
        limit: Default::default(),
        token_limit: None,
    });
    let refused = build_session_with_profile(
        SessionConstruction::Headless {
            prepared: &prepared,
            gate: &gate,
        },
        Arc::new(Mutex::new(ApprovalState::default())),
        "",
        true,
        false,
        None,
        SessionExtras {
            script_capture: Some(selection),
            token_sink: Some(Arc::new(|_| {})),
            ..Default::default()
        },
        Some(meter),
    )
    .await;
    assert!(refused.is_none());
    assert!(!config.join("threads.db").exists());
    assert!(
        !config.join("agents").exists(),
        "private rejection never materializes SDK/skills"
    );
}
