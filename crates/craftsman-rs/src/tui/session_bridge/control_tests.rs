//! UI-local controls use actual Agent finalization, never fabricated pause receipts.
use super::*;
use crate::agent_controls::AppControls;
use crate::context_compiler::{ContextOptions, MAX_OBSERVED_INPUT_BYTES};
use crate::failure_control::Settings;
use crate::tui::commands::{SLASH_COMMANDS, SubmissionRoute, live_command, submission_route};
use crate::tui::program_status::Status;
use async_trait::async_trait;
use craftsman_harness::testing::{MemCheckpointer, ScriptedModel};
use craftsman_harness::{
    Agent, AgentBoundaryPolicy, AgentError, AgentState, Checkpointer, Msg, Suspension, ThreadInfo,
    Tool, ToolCall, ToolCtx, ToolDef, ToolErrorKind, ToolMeta, ToolOutcome,
};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::Notify;

const WAIT: Duration = Duration::from_secs(2);
const THREAD: &str = "ui-control-proof";
const PROMPT: &str = "Repair the controlled fixture using actual observations.";
const TOOL: &str = "ui_observed_probe";
const ERROR: &str = "The synthetic observational diagnostic is unavailable.";

struct ObservationalError {
    calls: AtomicUsize,
}

#[async_trait]
impl Tool for ObservationalError {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: TOOL.into(),
            description: "Synthetic observation; performs no write or process operation.".into(),
            parameters: json!({"type":"object","properties":{"target":{"type":"string"}}}),
        }
    }

    async fn call(&self, _args: Value, _ctx: &mut ToolCtx<'_>) -> Result<ToolOutcome, AgentError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let mut meta = ToolMeta::new();
        meta.error_kind = Some(ToolErrorKind::NotFound);
        Ok(ToolOutcome::err(ERROR).with_meta(meta))
    }
}

struct Fixture {
    _workspace: tempfile::TempDir,
    controls: Arc<AppControls>,
    model: Arc<ScriptedModel>,
    agent: Arc<Agent>,
    tool: Arc<ObservationalError>,
}

fn responses() -> Vec<Msg> {
    (0..3)
        .map(|index| Msg::Assistant {
            content: "Inspect the admitted observational result.".into(),
            tool_calls: vec![ToolCall {
                id: format!("ui-observation-{index}"),
                name: TOOL.into(),
                args: json!({"target":"fixture.rs"}),
            }],
        })
        .collect()
}

fn fixture(checkpointer: Arc<dyn Checkpointer>, script: Vec<Msg>) -> Fixture {
    fixture_with_context(
        checkpointer,
        script,
        ContextOptions {
            enabled: true,
            max_bytes: 12288,
        },
    )
}

fn fixture_with_context(
    checkpointer: Arc<dyn Checkpointer>,
    script: Vec<Msg>,
    context: ContextOptions,
) -> Fixture {
    let workspace = tempfile::tempdir().unwrap();
    let settings = Settings::parse(Some(&json!({"repeat_limit":3})))
        .unwrap()
        .unwrap();
    let controls = AppControls::new(context, Some(&settings), workspace.path()).unwrap();
    let model = Arc::new(ScriptedModel::new(script));
    let tool = Arc::new(ObservationalError {
        calls: AtomicUsize::new(0),
    });
    let registered: Arc<dyn Tool> = tool.clone();
    let agent = Arc::new(
        Agent::builder(model.clone())
            .thread_id(THREAD)
            .max_rounds(8)
            .checkpointer(checkpointer)
            .durable_turn_checkpoints(true)
            .boundary_policy(controls.clone())
            .middleware(controls.observer())
            .middleware(controls.prepare())
            .middleware(controls.context.compiler())
            .middleware(controls.context.request_observer())
            .tool(registered.clone())
            .build(),
    );
    controls.bind_tools(&agent, &[registered], &[]);
    Fixture {
        _workspace: workspace,
        controls,
        model,
        agent,
        tool,
    }
}

fn draft(cancelled: bool) -> App {
    let cfg: Config = serde_json::from_value(json!({"voice":{"enabled":false}})).unwrap();
    let mut app = App::new(&cfg);
    app.input = "retain this 匠 draft".into();
    app.cursor = 5;
    app.running = true;
    app.cancel_requested = cancelled;
    app.discard_stream_deltas = cancelled;
    app
}

fn assert_draft(app: &App) {
    assert_eq!(app.input, "retain this 匠 draft");
    assert_eq!(app.cursor, 5);
}

async fn run(fixture: &Fixture, input: &str) -> Result<TurnResult, AgentError> {
    tokio::time::timeout(WAIT, fixture.agent.run_turn(input))
        .await
        .expect("real control turn did not return within its fixture bound")
}

async fn assert_real_pause(
    fixture: &Fixture,
    cp: &MemCheckpointer,
) -> Result<TurnResult, AgentError> {
    let result = run(fixture, PROMPT).await;
    assert!(
        matches!(&result, Err(AgentError::State(reason)) if reason.contains("failure budget paused"))
    );
    assert_eq!(fixture.model.requests.lock().unwrap().len(), 3);
    assert_eq!(fixture.tool.calls.load(Ordering::SeqCst), 3);
    assert!(!fixture.agent.is_turn_active());
    let receipt = fixture
        .controls
        .confirmed_pause()
        .expect("actual finalized pause receipt");
    assert_eq!(receipt.failures(), 3);
    let (_, saved, suspension) = cp.load_latest(THREAD).await.unwrap().unwrap();
    assert!(suspension.is_none());
    assert_eq!(
        saved.custom["_craftsman_durable_turn_v1"]["in_progress"],
        false
    );
    assert_eq!(saved.messages, fixture.agent.state.lock().unwrap().messages);
    assert_eq!(saved.messages.iter().filter(|message| matches!(message, Msg::Tool { content, is_error: true, .. } if content == ERROR)).count(), 3);
    assert!(!saved.messages.iter().any(|message| matches!(message, Msg::Assistant { content, tool_calls } if tool_calls.is_empty() && !content.is_empty())));
    result
}

#[tokio::test]
async fn actual_saved_boundary_pause_maps_to_activity_and_preserves_composer() {
    for cancelled_in_ui in [false, true] {
        let cp = Arc::new(MemCheckpointer::default());
        let fixture = fixture(cp.clone(), responses());
        assert!(fixture.controls.confirmed_pause().is_none());
        let result = assert_real_pause(&fixture, &cp).await;
        let event = UiEvent::turn_result_with_controls(result, Some(&fixture.controls));
        assert!(matches!(&event, UiEvent::TurnPaused(3)));
        let mut app = draft(cancelled_in_ui);
        handle_ui_event(&mut app, event, None);
        assert_draft(&app);
        assert!(!app.running && !app.cancel_requested && !app.discard_stream_deltas);
        assert_eq!(
            app.current_program_status(false),
            if cancelled_in_ui {
                Status::Idle
            } else {
                Status::Paused
            }
        );
        assert!(
            app.transcript
                .iter()
                .any(|item| item.kind == TranscriptKind::Activity
                    && item.text.starts_with("Paused after at least 3"))
        );
        assert!(
            app.transcript
                .iter()
                .all(|item| item.kind != TranscriptKind::Error)
        );
    }
}

#[derive(Default)]
struct FailFinalSave {
    saved: MemCheckpointer,
    failed_final: AtomicUsize,
}

fn final_snapshot(state: &AgentState) -> bool {
    state.custom["_craftsman_durable_turn_v1"]["in_progress"] == false
}

#[async_trait]
impl Checkpointer for FailFinalSave {
    async fn save(
        &self,
        thread: &str,
        seq: u64,
        state: &AgentState,
        suspension: Option<&Suspension>,
    ) -> Result<(), AgentError> {
        if final_snapshot(state) {
            self.failed_final.fetch_add(1, Ordering::SeqCst);
            return Err(AgentError::State("isolated final save failure".into()));
        }
        self.saved.save(thread, seq, state, suspension).await
    }
    async fn load_latest(
        &self,
        thread: &str,
    ) -> Result<Option<(u64, AgentState, Option<Suspension>)>, AgentError> {
        self.saved.load_latest(thread).await
    }
    async fn list_threads(&self) -> Result<Vec<ThreadInfo>, AgentError> {
        self.saved.list_threads().await
    }
    async fn delete_thread(&self, thread: &str) -> Result<(), AgentError> {
        self.saved.delete_thread(thread).await
    }
}

#[tokio::test]
async fn actual_final_save_failure_keeps_error_instead_of_a_successful_pause() {
    let cp = Arc::new(FailFinalSave::default());
    let fixture = fixture(cp.clone(), responses());
    let result = run(&fixture, PROMPT).await;
    assert!(
        matches!(&result, Err(AgentError::State(reason)) if reason.contains("checkpoint could not be saved"))
    );
    assert_eq!(cp.failed_final.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.tool.calls.load(Ordering::SeqCst), 3);
    assert!(fixture.agent.checkpoint_failed());
    assert!(fixture.controls.confirmed_pause().is_none());
    let (_, saved, _) = cp.load_latest(THREAD).await.unwrap().unwrap();
    assert_eq!(
        saved.custom["_craftsman_durable_turn_v1"]["in_progress"],
        true
    );
    let event = UiEvent::turn_result_with_controls(result, Some(&fixture.controls));
    assert!(matches!(&event, UiEvent::TurnFinished(Err(_))));
    let mut app = draft(true);
    handle_ui_event(&mut app, event, None);
    assert_draft(&app);
    assert_eq!(app.current_program_status(false), Status::Error);
    assert!(
        app.transcript
            .iter()
            .any(|item| item.kind == TranscriptKind::Error
                && item.text.contains("checkpoint could not be saved"))
    );
    assert!(
        !app.transcript
            .iter()
            .any(|item| item.text.starts_with("Paused after "))
    );
}

#[derive(Default)]
struct BlockFinalSave {
    saved: MemCheckpointer,
    entered: Notify,
    release: Notify,
}

#[async_trait]
impl Checkpointer for BlockFinalSave {
    async fn save(
        &self,
        thread: &str,
        seq: u64,
        state: &AgentState,
        suspension: Option<&Suspension>,
    ) -> Result<(), AgentError> {
        if final_snapshot(state) {
            self.entered.notify_one();
            self.release.notified().await;
        }
        self.saved.save(thread, seq, state, suspension).await
    }
    async fn load_latest(
        &self,
        thread: &str,
    ) -> Result<Option<(u64, AgentState, Option<Suspension>)>, AgentError> {
        self.saved.load_latest(thread).await
    }
    async fn list_threads(&self) -> Result<Vec<ThreadInfo>, AgentError> {
        self.saved.list_threads().await
    }
    async fn delete_thread(&self, thread: &str) -> Result<(), AgentError> {
        self.saved.delete_thread(thread).await
    }
}

#[tokio::test]
async fn actual_cancellation_during_final_save_never_mints_a_pause_receipt() {
    let cp = Arc::new(BlockFinalSave::default());
    let fixture = fixture(cp.clone(), responses());
    let agent = fixture.agent.clone();
    let task = tokio::spawn(async move { agent.run_turn(PROMPT).await });
    tokio::time::timeout(WAIT, cp.entered.notified())
        .await
        .unwrap();
    assert!(fixture.agent.is_turn_active());
    assert!(fixture.controls.confirmed_pause().is_none());
    assert_eq!(fixture.tool.calls.load(Ordering::SeqCst), 3);
    assert!(fixture.agent.request_cancel());
    cp.release.notify_one();
    let result = tokio::time::timeout(WAIT, task).await.unwrap().unwrap();
    assert!(matches!(&result, Err(AgentError::Cancelled)));
    assert!(fixture.controls.confirmed_pause().is_none());
    let (_, saved, _) = cp.load_latest(THREAD).await.unwrap().unwrap();
    assert_eq!(
        saved.custom["_craftsman_durable_turn_v1"]["in_progress"],
        false
    );
    let event = UiEvent::turn_result_with_controls(result, Some(&fixture.controls));
    assert!(!matches!(&event, UiEvent::TurnPaused(_)));
    let mut app = draft(true);
    handle_ui_event(&mut app, event, None);
    assert_draft(&app);
    assert!(!app.running && !app.cancel_requested);
    assert_eq!(app.current_program_status(false), Status::Idle);
    assert!(
        app.transcript
            .iter()
            .all(|item| item.kind != TranscriptKind::Error)
    );
}

#[tokio::test]
async fn real_followup_model_error_cannot_reuse_a_previous_pause_receipt() {
    let cp = Arc::new(MemCheckpointer::default());
    let fixture = fixture(cp.clone(), responses());
    let _ = assert_real_pause(&fixture, &cp).await;
    let result = run(
        &fixture,
        "A new accepted task starts a fresh control budget.",
    )
    .await;
    assert!(matches!(&result, Err(AgentError::Model(_))));
    assert!(fixture.controls.confirmed_pause().is_none());
    assert_eq!(fixture.model.requests.lock().unwrap().len(), 4);
    let event = UiEvent::turn_result_with_controls(result, Some(&fixture.controls));
    assert!(matches!(&event, UiEvent::TurnFinished(Err(_))));
    let mut app = draft(false);
    handle_ui_event(&mut app, event, None);
    assert_draft(&app);
    assert_eq!(app.current_program_status(false), Status::Error);
    assert!(
        app.transcript
            .iter()
            .any(|item| item.kind == TranscriptKind::Error
                && item.text.contains("scripted model exhausted"))
    );
}

#[tokio::test]
async fn real_rejected_initial_input_cannot_reuse_a_previous_pause_receipt() {
    let cp = Arc::new(MemCheckpointer::default());
    let fixture = fixture(cp.clone(), responses());
    let _ = assert_real_pause(&fixture, &cp).await;
    let oversized = "x".repeat(MAX_OBSERVED_INPUT_BYTES + 1);
    let result = run(&fixture, &oversized).await;
    assert!(matches!(&result, Err(AgentError::State(_))));
    assert_eq!(fixture.model.requests.lock().unwrap().len(), 3);
    assert!(fixture.controls.confirmed_pause().is_none());
    let (_, saved, _) = cp.load_latest(THREAD).await.unwrap().unwrap();
    assert!(matches!(saved.messages.last(), Some(Msg::User { content }) if content == &oversized));
    let event = UiEvent::turn_result_with_controls(result, Some(&fixture.controls));
    assert!(matches!(&event, UiEvent::TurnFinished(Err(_))));
    let mut app = draft(false);
    handle_ui_event(&mut app, event, None);
    assert_draft(&app);
    assert_eq!(app.current_program_status(false), Status::Error);
    assert!(
        app.transcript
            .iter()
            .any(|item| item.kind == TranscriptKind::Error)
    );
    assert!(
        !app.transcript
            .iter()
            .any(|item| item.text.starts_with("Paused after "))
    );
}

async fn assert_failed_reset_refuses_sampling(
    fixture: &Fixture,
    cp: &MemCheckpointer,
    key: &str,
    replacement: Value,
) {
    let original = fixture.agent.state.lock().unwrap().clone();
    assert!(original.custom.contains_key(key));
    assert!(fixture.controls.confirmed_pause().is_some());
    let error = {
        let mut state = fixture.agent.state.lock().unwrap();
        state.custom.insert(key.to_owned(), replacement);
        fixture
            .controls
            .reset(&mut state)
            .expect_err("malformed reset must refuse")
    };
    assert!(fixture.controls.confirmed_pause().is_none());
    // Even the prior valid transcript cannot reuse cached admission after a
    // failed reset. It needs a real successful rebind, not an old pause stamp.
    assert!(matches!(
        fixture.controls.before_model(&original),
        Err(AgentError::State(_))
    ));
    assert_eq!(fixture.model.requests.lock().unwrap().len(), 3);
    let event = UiEvent::turn_result_with_controls(Err(error), Some(&fixture.controls));
    assert!(matches!(&event, UiEvent::TurnFinished(Err(_))));
    let mut app = draft(false);
    handle_ui_event(&mut app, event, None);
    assert_draft(&app);
    assert_eq!(app.current_program_status(false), Status::Error);
    assert!(
        app.transcript
            .iter()
            .any(|item| item.kind == TranscriptKind::Error)
    );
    assert!(
        !app.transcript
            .iter()
            .any(|item| item.text.starts_with("Paused after "))
    );

    let followup = "This accepted follow-up must remain saved while reset is unavailable.";
    let result = run(fixture, followup).await;
    assert!(matches!(&result, Err(AgentError::State(_))));
    assert_eq!(fixture.model.requests.lock().unwrap().len(), 3);
    assert_eq!(fixture.tool.calls.load(Ordering::SeqCst), 3);
    assert!(fixture.controls.confirmed_pause().is_none());
    let (_, saved, _) = cp.load_latest(THREAD).await.unwrap().unwrap();
    assert!(matches!(saved.messages.last(), Some(Msg::User { content }) if content == followup));
    let event = UiEvent::turn_result_with_controls(result, Some(&fixture.controls));
    assert!(matches!(&event, UiEvent::TurnFinished(Err(_))));
    let mut app = draft(false);
    handle_ui_event(&mut app, event, None);
    assert_draft(&app);
    assert_eq!(app.current_program_status(false), Status::Error);
    assert!(
        !app.transcript
            .iter()
            .any(|item| item.text.starts_with("Paused after "))
    );
}

#[tokio::test]
async fn failed_context_reset_after_real_pause_clears_receipt_and_refuses_sampling() {
    let cp = Arc::new(MemCheckpointer::default());
    let fixture = fixture(cp.clone(), responses());
    let _ = assert_real_pause(&fixture, &cp).await;
    assert_failed_reset_refuses_sampling(
        &fixture,
        &cp,
        "_craftsman_context_v1",
        json!({"version":999}),
    )
    .await;
}

#[tokio::test]
async fn failed_governor_only_reset_after_real_pause_clears_receipt_and_refuses_sampling() {
    let cp = Arc::new(MemCheckpointer::default());
    let fixture = fixture_with_context(cp.clone(), responses(), ContextOptions::default());
    let _ = assert_real_pause(&fixture, &cp).await;
    assert!(!fixture.controls.context.options().enabled);
    assert_failed_reset_refuses_sampling(
        &fixture,
        &cp,
        "_craftsman_failure_input_v1",
        json!({"version":999}),
    )
    .await;
}

#[tokio::test]
async fn mismatched_stamps_reset_after_real_pause_clears_receipt_and_refuses_sampling() {
    let cp = Arc::new(MemCheckpointer::default());
    let fixture = fixture(cp.clone(), responses());
    let _ = assert_real_pause(&fixture, &cp).await;
    let mut changed =
        fixture.agent.state.lock().unwrap().custom["_craftsman_failure_input_v1"].clone();
    let revision = changed["stamp"]["revision"].as_u64().unwrap();
    changed["stamp"]["revision"] = json!(revision.checked_add(1).unwrap());
    assert_failed_reset_refuses_sampling(&fixture, &cp, "_craftsman_failure_input_v1", changed)
        .await;
}

#[tokio::test]
async fn actual_context_inspection_is_local_and_preserves_state_and_draft() {
    let cp = Arc::new(MemCheckpointer::default());
    let fixture = fixture(cp.clone(), responses());
    let _ = assert_real_pause(&fixture, &cp).await;
    let state = fixture.agent.state.lock().unwrap().clone();
    let original = serde_json::to_value(&state).unwrap();
    let requests = fixture.model.requests.lock().unwrap().len();
    let report = fixture.controls.context.render(&state, Some(8192)).unwrap();
    let (_, json) = report.split_once('\n').unwrap();
    let view: Value = serde_json::from_str(json).unwrap();
    assert_eq!(view["context_enabled"], true);
    assert_eq!(view["configured_model_window"], 8192);
    assert_eq!(view["current_portable_state"]["tool_messages"], 3);
    assert_eq!(view["last_prepared_request"]["tool_messages"], 2);
    assert_eq!(view["last_response_reported_input_tokens"], Value::Null);
    assert_eq!(view["last_response_reported_output_tokens"], Value::Null);
    assert_eq!(view["context_record"]["initial_request"]["text"], PROMPT);
    assert_eq!(serde_json::to_value(&state).unwrap(), original);
    assert_eq!(fixture.model.requests.lock().unwrap().len(), requests);
    let mut app = draft(false);
    app.push(report);
    assert_draft(&app);
    assert!(app.running);
    assert_eq!(fixture.model.requests.lock().unwrap().len(), requests);
}

#[tokio::test]
async fn context_command_is_reserved_live_and_uninitialized_route_is_local() {
    assert!(
        SLASH_COMMANDS
            .iter()
            .any(|(command, _)| *command == "/context")
    );
    assert!(crate::extensions::RESERVED_COMMANDS.contains(&"context"));
    assert!(live_command("/context"));
    assert!(matches!(
        submission_route("/context", None),
        SubmissionRoute::Slash
    ));
    assert!(matches!(
        submission_route("/context", Some("expanded model input")),
        SubmissionRoute::Prompt
    ));
    let (tx, mut receive) = mpsc::unbounded_channel();
    let owner = craftsman_harness::SessionCancellation::default();
    let mut slot = AgentSlot {
        inner: AgentSlotInner::Ready(Box::new(None)),
        bridge: UiBridge { tx },
        root_cancellation: owner.clone(),
        cancellation: owner,
        retired: tokio::task::JoinSet::new(),
    };
    let mut cfg = Config::default();
    let before = serde_json::to_value(&cfg).unwrap();
    let approval = Arc::new(Mutex::new(ApprovalState::default()));
    let mut app = draft(false);
    crate::tui::commands::handle_slash("/context", &mut cfg, &mut slot, &approval, &mut app).await;
    assert_draft(&app);
    assert!(app.running && !app.cancel_requested);
    assert!(app.transcript.iter().any(|item| item.text == "Context inspector: the local session is not initialized yet."));
    assert_eq!(serde_json::to_value(&cfg).unwrap(), before);
    assert!(receive.try_recv().is_err());
    crate::tui::commands::handle_slash("/context extra", &mut cfg, &mut slot, &approval, &mut app)
        .await;
    assert_draft(&app);
    assert!(
        app.transcript
            .iter()
            .any(|item| item.kind == TranscriptKind::Error && item.text == "usage: /context")
    );
    assert!(receive.try_recv().is_err());
    assert_eq!(serde_json::to_value(&cfg).unwrap(), before);
}
