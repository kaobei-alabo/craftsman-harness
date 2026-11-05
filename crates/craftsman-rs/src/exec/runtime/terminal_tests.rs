use super::*;
use crate::exec::construction::{Options, SessionSelection};
use crate::exec::input::{Invocation, Run, Sources};
use crate::exec::permissions::{OnUnapproved, Selectors};
use crate::exec::provider::request::Fault;
use crate::exec::terminal::{Error, ReturnKind, Terminal};
use async_trait::async_trait;
use craftsman_harness::{
    AgentError, AgentRegistry, FileData, ModelProvider, ModelRequest, ModelResponse, Msg, StateFs,
    Tool, ToolCall, ToolCtx, ToolDef, ToolErrorKind, ToolOutcome,
};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
const SEED: &str = "terminal-runtime-secret";

struct Fixture {
    _root: tempfile::TempDir,
    workspace: PathBuf,
    config: PathBuf,
    server: MockServer,
}
impl Fixture {
    async fn new(seed: &str) -> Self {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("workspace");
        let config = root.path().join("selected-config");
        std::fs::create_dir(&workspace).unwrap();
        std::fs::create_dir(&config).unwrap();
        let f = Self {
            _root: root,
            workspace,
            config,
            server: MockServer::start().await,
        };
        f.configure(seed);
        f
    }
    fn configure(&self, seed: &str) {
        std::fs::write(
            self.config.join("config.json"),
            json!({"model":"synthetic-model","api_key":seed,"base_url":self.server.uri()})
                .to_string(),
        )
        .unwrap();
    }
    fn options(&self, allow: &[&str]) -> Options {
        Options {
            workspace: self.workspace.clone(),
            config_dir: self.config.clone(),
            trust_workspace: false,
            session: SessionSelection::Ephemeral,
            selectors: Selectors {
                on_unapproved: OnUnapproved::Fail,
                allow: allow.iter().map(|n| format!("tool:{n}")).collect(),
                ..Default::default()
            },
        }
    }
    async fn response(&self, content: &str) {
        let data = json!({"choices":[{"delta":{"content":content},"finish_reason":"stop"}]});
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                format!("data: {data}\n\ndata: [DONE]\n\n").into_bytes(),
                "text/event-stream",
            ))
            .mount(&self.server)
            .await;
    }
    async fn approval_response(&self) {
        let data = json!({"choices":[{"delta":{"content":format!("actual partial {SEED}"),"tool_calls":[{"index":0,"id":"stop","type":"function","function":{"name":"execute","arguments":json!({"command":"must-not-execute"}).to_string()}},{"index":1,"id":"later","type":"function","function":{"name":"write_file","arguments":json!({"file_path":"later.txt","content":SEED}).to_string()}}]},"finish_reason":"tool_calls"}]});
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                format!("data: {data}\n\ndata: [DONE]\n\n").into_bytes(),
                "text/event-stream",
            ))
            .mount(&self.server)
            .await;
    }
    async fn run(&self, prompt: &str, allow: &[&str]) -> Run {
        Invocation::prepare(
            self.options(allow),
            Sources {
                prompt: Some(prompt.as_bytes()),
                prompt_file: None,
                files: &[],
            },
        )
        .await
        .unwrap()
        .run()
        .await
    }
    async fn returned(&self) -> Run {
        self.response(&format!("original answer {SEED}")).await;
        let r = self.run(&format!("original input {SEED}"), &[]).await;
        assert!(r.outcome().is_ok());
        r
    }
    fn no_store(&self) {
        assert!(!self.config.join("threads.db").exists());
        assert!(!self.config.join("sessions-rs").exists());
    }
}
fn value(t: &Terminal) -> Value {
    let s = t.snapshot().unwrap();
    let json: Value = serde_json::from_slice(s.bytes().unwrap()).unwrap();
    assert_eq!(
        json["messages"].as_array().unwrap().len(),
        s.message_count().unwrap()
    );
    json
}
fn state(run: &Run) -> Arc<std::sync::Mutex<AgentState>> {
    run.runtime_for_tests().session.agent.state.clone()
}
fn gate(run: &Run) -> Arc<InvocationGate> {
    run.runtime_for_tests().gate.clone()
}
struct CountingProvider(Arc<AtomicUsize>);
#[async_trait]
impl ModelProvider for CountingProvider {
    async fn complete(&self, _: ModelRequest) -> Result<ModelResponse, AgentError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(AgentError::Model("must not run".into()))
    }
}
fn request() -> ModelRequest {
    ModelRequest {
        system: "synthetic".into(),
        messages: vec![],
        tools: vec![],
    }
}
struct CountingTool(Arc<AtomicUsize>);
#[async_trait]
impl Tool for CountingTool {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "write_file".into(),
            description: "synthetic".into(),
            parameters: json!({}),
        }
    }
    async fn call(&self, _: Value, _: &mut ToolCtx<'_>) -> Result<ToolOutcome, AgentError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(ToolOutcome::ok("must not run"))
    }
}

#[tokio::test]
async fn only_a_consumed_actual_run_can_mint_independent_terminal_scope() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    assert_eq!(run.terminal_scope().err(), Some(Error::Unsettled));
    let g = gate(&run);
    let t = run.terminal();
    assert!(g.execution_closed());
    assert!(!g.approval_required());
    assert!(t.scope().is_ok());
    f.no_store();
}
#[tokio::test]
async fn a_plain_equal_prepared_runtime_cannot_bind_an_actual_returned_run_witness() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    let other = Runtime::build(Prepared::prepare(f.options(&[])).await.unwrap())
        .await
        .unwrap();
    other.close_for_terminal();
    assert_eq!(other.terminal_scope(&run).err(), Some(Error::Foreign));
}
#[tokio::test]
async fn bypassing_canonical_run_sealing_cannot_open_publication_authority() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    let t = Terminal::from_run(run);
    assert_eq!(t.snapshot().err(), Some(Error::Unsettled));
}
#[tokio::test]
async fn returned_fact_is_not_a_public_completion_verification_or_exit_mapping() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    let t = run.terminal();
    let s = t.snapshot().unwrap();
    assert_eq!(s.facts().unwrap().returned(), ReturnKind::Returned);
    assert!(!s.facts().unwrap().approval_required());
    let json: Value = serde_json::from_slice(s.bytes().unwrap()).unwrap();
    assert_eq!(json["run_return"], "returned");
    assert_eq!(json["approval_required"], false);
    for field in [
        "status",
        "exit_code",
        "stop_reason",
        "usage",
        "rounds",
        "wall_ms",
        "verification",
        "schema",
    ] {
        assert!(json.get(field).is_none());
    }
}
#[tokio::test]
async fn original_final_answer_is_separately_checked_not_inferred_from_last_retained_message() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    let shared = state(&run);
    shared.lock().unwrap().messages = vec![Msg::user("different retained last message")];
    assert_eq!(
        run.outcome().as_ref().unwrap().final_text(),
        format!("original answer {SEED}")
    );
    let t = run.terminal();
    let json = value(&t);
    assert_eq!(json["final_output"], "original answer [REDACTED]");
    assert_eq!(
        json["messages"][0]["content"],
        "different retained last message"
    );
    assert_eq!(
        shared.lock().unwrap().messages,
        [Msg::user("different retained last message")]
    );
    f.no_store();
}
#[tokio::test]
async fn terminal_seal_does_not_forge_approval_and_blocks_actual_compaction_provider_traffic() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    let g = gate(&run);
    let compaction = run.runtime_for_tests().session.compaction.clone();
    let shared = state(&run);
    let t = run.terminal();
    assert!(g.execution_closed());
    assert!(!g.approval_required());
    let original = shared.lock().unwrap().messages.clone();
    assert!(compaction.compact_now(&original).await.is_err());
    assert_eq!(f.server.received_requests().await.unwrap().len(), 1);
    assert!(!g.approval_required());
    assert!(t.snapshot().is_ok());
}
#[tokio::test]
async fn same_runtime_run_turn_after_seal_is_refused_before_state_or_provider_effects() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    let runtime = run.runtime_for_tests();
    runtime.close_for_terminal();
    let original = runtime.retained_state().messages;
    assert!(
        runtime
            .run_turn("later must not be admitted".into())
            .await
            .is_err()
    );
    assert_eq!(runtime.retained_state().messages, original);
    assert_eq!(f.server.received_requests().await.unwrap().len(), 1);
    assert!(!runtime.gate.approval_required());
    assert!(run.terminal().snapshot().is_ok());
}
#[tokio::test]
async fn all_role_provider_wrappers_observe_the_same_deny_only_terminal_seal() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    let g = gate(&run);
    let n = Arc::new(AtomicUsize::new(0));
    let provider = g.wrap_provider(Arc::new(CountingProvider(n.clone())));
    let t = run.terminal();
    assert!(provider.complete(request()).await.is_err());
    assert_eq!(n.load(Ordering::SeqCst), 0);
    assert!(!g.approval_required());
    assert!(t.snapshot().is_ok());
}
#[tokio::test]
async fn tool_terminals_after_seal_deny_without_dispatch_or_fabricated_needs_approval() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    let g = gate(&run);
    let n = Arc::new(AtomicUsize::new(0));
    let tool = g.wrap_tool(Arc::new(CountingTool(n.clone())));
    let t = run.terminal();
    let mut raw = AgentState::default();
    let mut ctx = ToolCtx {
        state: &mut raw,
        backend: Arc::new(StateFs::new(Arc::new(std::sync::Mutex::new(
            AgentState::default(),
        )))),
        registry: Arc::new(AgentRegistry::default()),
        notifications: Default::default(),
    };
    let outcome = tool
        .call(json!({"file_path":"later","content":SEED}), &mut ctx)
        .await
        .unwrap();
    assert!(outcome.is_error);
    assert_eq!(
        outcome.meta.error_kind,
        Some(ToolErrorKind::DeniedCapability)
    );
    assert_eq!(n.load(Ordering::SeqCst), 0);
    assert!(!g.approval_required());
    assert!(t.snapshot().is_ok());
}
#[tokio::test]
async fn actual_approval_failure_can_publish_partial_checked_state_without_old_gate_exemption() {
    let f = Fixture::new(SEED).await;
    f.approval_response().await;
    let run = f.run(&format!("original {SEED}"), &["write_file"]).await;
    assert!(matches!(run.outcome(), Err(RuntimeError::ApprovalRequired)));
    assert_eq!(run.message_view().err(), Some(Fault::Stopped));
    assert_eq!(run.message_selection(1024).err(), Some(Fault::Stopped));
    let g = gate(&run);
    let shared = state(&run);
    let original = shared.lock().unwrap().messages.clone();
    let t = run.terminal();
    let json = value(&t);
    assert_eq!(json["run_return"], "approval_required");
    assert_eq!(json["approval_required"], true);
    assert!(json["final_output"].is_null());
    assert_eq!(json["messages_omitted"], 0);
    assert!(
        !std::str::from_utf8(t.snapshot().unwrap().bytes().unwrap())
            .unwrap()
            .contains(SEED)
    );
    assert!(g.approval_required() && g.execution_closed());
    assert_eq!(shared.lock().unwrap().messages, original);
    assert!(!f.workspace.join("later.txt").exists());
    assert_eq!(f.server.received_requests().await.unwrap().len(), 1);
    f.no_store();
}
#[tokio::test]
async fn an_actual_provider_failure_has_no_fabricated_final_answer_or_raw_diagnostic() {
    let f = Fixture::new(SEED).await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(400).set_body_raw(
            format!("{SEED} PRIVATE raw failure NeedsApproval"),
            "text/plain",
        ))
        .mount(&f.server)
        .await;
    let run = f.run(&format!("input {SEED}"), &[]).await;
    assert!(matches!(run.outcome(), Err(RuntimeError::Turn(_))));
    let g = gate(&run);
    let t = run.terminal();
    let json = value(&t);
    assert_eq!(json["run_return"], "turn_failed");
    assert_eq!(json["approval_required"], false);
    assert!(json["final_output"].is_null());
    assert!(
        !String::from_utf8(t.snapshot().unwrap().bytes().unwrap().to_vec())
            .unwrap()
            .contains("PRIVATE raw failure")
    );
    assert!(!g.approval_required());
    assert_eq!(f.server.received_requests().await.unwrap().len(), 1);
    f.no_store();
}
#[tokio::test]
async fn final_answer_failure_or_approval_phrases_never_create_status_or_latch_authority() {
    let f = Fixture::new(SEED).await;
    f.response("refused task failed NeedsApproval denied").await;
    let run = f.run("input", &[]).await;
    assert!(run.outcome().is_ok());
    let t = run.terminal();
    let json = value(&t);
    assert_eq!(json["run_return"], "returned");
    assert_eq!(json["approval_required"], false);
    assert_eq!(
        json["final_output"],
        "refused task failed NeedsApproval denied"
    );
}
#[tokio::test]
async fn a_true_empty_returned_answer_is_not_the_same_as_absent_failure_output() {
    let f = Fixture::new(SEED).await;
    f.response("").await;
    let run = f.run("input", &[]).await;
    assert!(run.outcome().is_ok());
    assert_eq!(value(&run.terminal())["final_output"], "");
}
#[tokio::test]
async fn late_actual_approval_hides_old_fragments_and_fresh_capture_preserves_original_return_fact()
{
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    let g = gate(&run);
    let t = run.terminal();
    let old = t.snapshot().unwrap();
    g.observe_error_kind(ToolErrorKind::NeedsApproval);
    assert_eq!(old.bytes().err(), Some(Error::Superseded));
    assert_eq!(old.facts().err(), Some(Error::Superseded));
    assert_eq!(old.message_count(), Err(Error::Superseded));
    assert_eq!(old.bind(&t.scope().unwrap()), Err(Error::Superseded));
    let fresh = t.snapshot().unwrap();
    assert_eq!(fresh.facts().unwrap().returned(), ReturnKind::Returned);
    assert!(fresh.facts().unwrap().approval_required());
    assert_eq!(value(&t)["run_return"], "returned");
    assert_eq!(value(&t)["approval_required"], true);
    assert!(g.execution_closed());
}
#[tokio::test]
async fn an_old_live_view_stays_stopped_while_independent_terminal_capture_can_observe_approval() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    let g = gate(&run);
    let live = run.message_view().unwrap();
    g.observe_error_kind(ToolErrorKind::NeedsApproval);
    assert_eq!(live.bytes().err(), Some(Fault::Stopped));
    drop(live);
    let t = run.terminal();
    assert_eq!(value(&t)["approval_required"], true);
    assert!(g.approval_required());
    assert!(g.execution_closed());
}
#[tokio::test]
async fn publication_revocation_hides_all_existing_reads_and_refuses_new_capture() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    let g = gate(&run);
    let t = run.terminal();
    let s = t.snapshot().unwrap();
    t.close();
    assert_eq!(s.bytes().err(), Some(Error::Closed));
    assert_eq!(s.facts().err(), Some(Error::Closed));
    assert_eq!(s.message_count(), Err(Error::Closed));
    assert_eq!(t.snapshot().err(), Some(Error::Closed));
    assert!(g.execution_closed());
    assert!(!g.approval_required());
}
#[tokio::test]
async fn equal_fresh_runs_cannot_bind_to_each_others_terminal_publication_authority() {
    let f = Fixture::new(SEED).await;
    f.response("same").await;
    let one = f.run("same", &[]).await.terminal();
    let two = f.run("same", &[]).await.terminal();
    let s = one.snapshot().unwrap();
    assert!(s.bind(&one.scope().unwrap()).is_ok());
    assert_eq!(s.bind(&two.scope().unwrap()), Err(Error::Foreign));
}
#[tokio::test]
async fn actual_terminal_scope_keeps_original_all_role_order_ids_keys_scalars_and_flags() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    let shared = state(&run);
    let original = vec![
        Msg::System {
            content: format!("system {SEED}"),
        },
        Msg::user(format!("user {SEED}")),
        Msg::Assistant {
            content: format!("assistant {SEED}"),
            tool_calls: vec![ToolCall {
                id: "original-id".into(),
                name: "original-name".into(),
                args: json!({"data":SEED,"number":7,"enabled":true}),
            }],
        },
        Msg::Tool {
            tool_call_id: "original-id".into(),
            content: format!("tool {SEED}"),
            is_error: true,
        },
    ];
    shared.lock().unwrap().messages = original.clone();
    let json = value(&run.terminal());
    let rows = json["messages"].as_array().unwrap();
    assert_eq!(
        rows.iter()
            .map(|r| r["role"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["system", "user", "assistant", "tool"]
    );
    assert_eq!(rows[2]["tool_calls"][0]["id"], "original-id");
    assert_eq!(rows[2]["tool_calls"][0]["name"], "original-name");
    assert_eq!(rows[2]["tool_calls"][0]["args"]["number"], 7);
    assert_eq!(rows[3]["is_error"], true);
    assert_eq!(shared.lock().unwrap().messages, original);
}
#[tokio::test]
async fn malformed_and_non_object_args_are_data_views_not_repaired_execution_aliases() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    let shared = state(&run);
    let original = Msg::Assistant {
        content: "".into(),
        tool_calls: vec![ToolCall {
            id: "id".into(),
            name: "example".into(),
            args: json!(format!("{{bad {SEED}")),
        }],
    };
    shared.lock().unwrap().messages = vec![original.clone()];
    let json = value(&run.terminal());
    assert_eq!(
        json["messages"][0]["tool_calls"][0]["args"],
        "{bad [REDACTED]"
    );
    assert_eq!(shared.lock().unwrap().messages, [original]);
}
#[tokio::test]
async fn terminal_capture_never_serializes_other_raw_state_or_provenance() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    let shared = state(&run);
    {
        let mut s = shared.lock().unwrap();
        s.files.insert("private".into(), FileData::text(SEED));
        s.custom
            .insert("private-origin".into(), json!({"data":SEED}));
        s.system_extras.push(SEED.into());
    }
    let json = value(&run.terminal());
    assert_eq!(json.as_object().unwrap().len(), 6);
    for key in [
        "files",
        "todos",
        "custom",
        "system_extras",
        "origin",
        "usage",
        "error",
    ] {
        assert!(json.get(key).is_none());
    }
    assert_eq!(shared.lock().unwrap().files["private"].content, SEED);
    f.no_store();
}
#[tokio::test]
async fn frozen_terminal_seeds_do_not_reload_replaced_configuration() {
    let f = Fixture::new(SEED).await;
    f.response(&format!("{SEED} later-key")).await;
    let run = f.run("input", &[]).await;
    f.configure("later-key");
    let json = value(&run.terminal());
    assert_eq!(json["final_output"], "[REDACTED] later-key");
}
#[tokio::test]
async fn terminal_fixed_schema_key_collisions_refuse_without_aliasing_metadata() {
    let f = Fixture::new("run_return").await;
    f.response("data").await;
    let run = f.run("input", &[]).await;
    let t = run.terminal();
    assert_eq!(t.snapshot().err(), Some(Error::Codec(Fault::Collision)));
}
#[tokio::test]
async fn terminal_return_tag_collisions_are_refused_not_renamed_to_public_success() {
    let f = Fixture::new("returned").await;
    f.response("done").await;
    let run = f.run("input", &[]).await;
    assert!(run.outcome().is_ok());
    assert_eq!(
        run.terminal().snapshot().err(),
        Some(Error::Codec(Fault::Collision))
    );
}
#[tokio::test]
async fn terminal_boolean_collisions_preserve_real_gate_fact_or_refuse() {
    let f = Fixture::new("false").await;
    f.response("done").await;
    let run = f.run("input", &[]).await;
    let g = gate(&run);
    let t = run.terminal();
    assert!(!g.approval_required());
    assert_eq!(t.snapshot().err(), Some(Error::Codec(Fault::Collision)));
    assert!(!g.approval_required());
}
#[tokio::test]
async fn encoded_escaping_cannot_create_a_credential_after_terminal_text_redaction() {
    let f = Fixture::new(r"escaped\nsecret").await;
    f.response("escaped\nsecret").await;
    let run = f.run("input", &[]).await;
    assert_eq!(
        run.terminal().snapshot().err(),
        Some(Error::Codec(Fault::Collision))
    );
}
#[tokio::test]
async fn unsafe_message_id_refuses_even_after_execution_is_closed() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    let shared = state(&run);
    shared.lock().unwrap().messages = vec![Msg::tool(SEED, "data")];
    assert_eq!(
        run.terminal().snapshot().err(),
        Some(Error::Codec(Fault::Collision))
    );
}
#[tokio::test]
async fn unsafe_argument_key_refuses_original_state_without_mutation_or_repair() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    let shared = state(&run);
    let original = Msg::Assistant {
        content: "".into(),
        tool_calls: vec![ToolCall {
            id: "id".into(),
            name: "name".into(),
            args: json!({SEED:"data"}),
        }],
    };
    shared.lock().unwrap().messages = vec![original.clone()];
    let t = run.terminal();
    assert_eq!(t.snapshot().err(), Some(Error::Codec(Fault::Collision)));
    assert_eq!(shared.lock().unwrap().messages, [original]);
}
#[tokio::test]
async fn terminal_message_cap_refuses_whole_without_omission_or_raw_state_clone() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    let shared = state(&run);
    shared.lock().unwrap().messages = vec![Msg::user("whole"); 257];
    let t = run.terminal();
    assert_eq!(t.snapshot().err(), Some(Error::Codec(Fault::Limit)));
    assert_eq!(shared.lock().unwrap().messages.len(), 257);
}
#[tokio::test]
async fn oversized_original_message_refuses_before_copies_and_preserves_original_data() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    let shared = state(&run);
    let original = Msg::user("x".repeat(1024 * 1024 + 1));
    shared.lock().unwrap().messages = vec![original.clone()];
    let t = run.terminal();
    assert_eq!(t.snapshot().err(), Some(Error::Codec(Fault::Limit)));
    assert_eq!(shared.lock().unwrap().messages, [original]);
}
#[tokio::test]
async fn terminal_final_output_work_limit_is_not_silent_answer_truncation() {
    let f = Fixture::new(SEED).await;
    f.response(&"x".repeat(400_000)).await;
    let run = f.run("input", &[]).await;
    assert_eq!(run.outcome().as_ref().unwrap().final_text().len(), 400_000);
    let shared = state(&run);
    shared.lock().unwrap().messages = vec![];
    let t = run.terminal();
    assert_eq!(t.snapshot().err(), Some(Error::Codec(Fault::Limit)));
    assert!(shared.lock().unwrap().messages.is_empty());
}
#[tokio::test]
async fn combined_finite_fragment_budget_refuses_even_when_each_channel_passes() {
    let f = Fixture::new(SEED).await;
    f.response(&"\u{0001}".repeat(110_000)).await;
    let run = f.run("input", &[]).await;
    let shared = state(&run);
    shared.lock().unwrap().messages = vec![Msg::user("\u{0001}".repeat(110_000))];
    let t = run.terminal();
    assert_eq!(t.snapshot().err(), Some(Error::Codec(Fault::Limit)));
}
#[tokio::test]
async fn original_argument_depth_bound_is_not_a_terminal_authority_exemption() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    let shared = state(&run);
    let mut args = Value::Null;
    for _ in 0..65 {
        args = json!([args]);
    }
    shared.lock().unwrap().messages = vec![Msg::Assistant {
        content: "".into(),
        tool_calls: vec![ToolCall {
            id: "id".into(),
            name: "name".into(),
            args,
        }],
    }];
    assert_eq!(
        run.terminal().snapshot().err(),
        Some(Error::Codec(Fault::Limit))
    );
}
#[tokio::test]
async fn unicode_controls_and_overlapping_literal_data_remain_checked_finite_data() {
    let f = Fixture::new("秘密🦀").await;
    f.response("before 秘密🦀秘密🦀 after\r\n\u{0000}").await;
    let run = f.run("input", &[]).await;
    let json = value(&run.terminal());
    assert_eq!(json["final_output"], "before [REDACTED] after\r\n\u{0000}");
}
#[tokio::test]
async fn actual_after_work_approval_change_hides_success_and_overrides_codec_failure() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    let g = gate(&run);
    let t = run.terminal();
    assert_eq!(
        t.snapshot_after_work_for_tests(|| g.observe_error_kind(ToolErrorKind::NeedsApproval))
            .err(),
        Some(Error::Superseded)
    );
    assert!(t.snapshot().unwrap().facts().unwrap().approval_required());
    let f = Fixture::new("run_return").await;
    f.response("data").await;
    let run = f.run("input", &[]).await;
    let g = gate(&run);
    let t = run.terminal();
    assert_eq!(
        t.snapshot_after_work_for_tests(|| g.observe_error_kind(ToolErrorKind::NeedsApproval))
            .err(),
        Some(Error::Superseded)
    );
}
#[tokio::test]
async fn actual_after_work_publication_revocation_overrides_success_and_bound_failure() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    let t = run.terminal();
    assert_eq!(
        t.snapshot_after_work_for_tests(|| t.close()).err(),
        Some(Error::Closed)
    );
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    state(&run).lock().unwrap().messages = vec![Msg::user("whole"); 257];
    let t = run.terminal();
    assert_eq!(
        t.snapshot_after_work_for_tests(|| t.close()).err(),
        Some(Error::Closed)
    );
}
#[tokio::test]
async fn codec_failure_retains_original_runtime_and_allows_safe_fresh_capture_without_reexecution()
{
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    let shared = state(&run);
    shared.lock().unwrap().messages = vec![Msg::tool(SEED, "unsafe")];
    let t = run.terminal();
    assert_eq!(t.snapshot().err(), Some(Error::Codec(Fault::Collision)));
    shared.lock().unwrap().messages = vec![Msg::user("retained safe")];
    assert_eq!(value(&t)["final_output"], "original answer [REDACTED]");
    assert_eq!(f.server.received_requests().await.unwrap().len(), 1);
}
#[tokio::test]
async fn snapshot_is_immutable_after_later_state_changes_not_a_live_or_child_ledger() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    let shared = state(&run);
    let t = run.terminal();
    let s = t.snapshot().unwrap();
    let first = s.bytes().unwrap().to_vec();
    let n = s.message_count().unwrap();
    shared.lock().unwrap().messages = vec![Msg::user("new summary")];
    assert_eq!(s.bytes().unwrap(), first);
    assert_eq!(s.message_count().unwrap(), n);
    assert_eq!(value(&t)["messages"].as_array().unwrap().len(), 1);
}
#[tokio::test]
async fn closed_execution_denial_uses_actual_frozen_seeds_and_never_relabels_approval() {
    let f = Fixture::new("invocation").await;
    f.response("done").await;
    let run = f.run("input", &[]).await;
    let g = gate(&run);
    let t = run.terminal();
    let denied = g.denied("write_file", &json!({})).unwrap();
    assert!(!denied.content.contains("invocation"));
    assert!(denied.content.contains("[REDACTED]"));
    assert_eq!(
        denied.meta.error_kind,
        Some(ToolErrorKind::DeniedCapability)
    );
    assert!(!g.approval_required());
    assert!(t.snapshot().is_ok());
}

#[tokio::test]
async fn oversized_original_final_answer_is_admitted_before_any_message_copy() {
    let f = Fixture::new(SEED).await;
    f.response(&"x".repeat(1024 * 1024 + 1)).await;
    let run = f.run("input", &[]).await;
    assert_eq!(
        run.outcome().as_ref().unwrap().final_text().len(),
        1024 * 1024 + 1
    );
    // If message production ran first this id would collide. The original
    // final-answer byte limit instead refuses before that producer can copy.
    state(&run).lock().unwrap().messages = vec![Msg::tool(SEED, "unsafe id")];
    assert_eq!(
        run.terminal().snapshot().err(),
        Some(Error::Codec(Fault::Limit))
    );
}
#[tokio::test]
async fn terminal_call_count_bound_is_unchanged_after_finalization() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    let call = ToolCall {
        id: "id".into(),
        name: "name".into(),
        args: json!({}),
    };
    state(&run).lock().unwrap().messages = vec![Msg::Assistant {
        content: "".into(),
        tool_calls: vec![call; 257],
    }];
    assert_eq!(
        run.terminal().snapshot().err(),
        Some(Error::Codec(Fault::Limit))
    );
}
#[tokio::test]
async fn terminal_node_work_bound_is_unchanged_after_finalization() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    state(&run).lock().unwrap().messages = vec![Msg::Assistant {
        content: "".into(),
        tool_calls: vec![ToolCall {
            id: "id".into(),
            name: "name".into(),
            args: Value::Array(vec![Value::Null; 100_001]),
        }],
    }];
    assert_eq!(
        run.terminal().snapshot().err(),
        Some(Error::Codec(Fault::Limit))
    );
}
#[tokio::test]
async fn terminal_identity_scalar_bound_is_unchanged_after_finalization() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    state(&run).lock().unwrap().messages = vec![Msg::tool("字".repeat(129), "data")];
    assert_eq!(
        run.terminal().snapshot().err(),
        Some(Error::Codec(Fault::Limit))
    );
}
#[tokio::test]
async fn terminal_maximum_admitted_argument_depth_remains_a_valid_checked_shape() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    let mut args = Value::Null;
    for _ in 0..64 {
        args = json!([args]);
    }
    state(&run).lock().unwrap().messages = vec![Msg::Assistant {
        content: "".into(),
        tool_calls: vec![ToolCall {
            id: "id".into(),
            name: "name".into(),
            args: args.clone(),
        }],
    }];
    assert_eq!(
        value(&run.terminal())["messages"][0]["tool_calls"][0]["args"],
        args
    );
}
