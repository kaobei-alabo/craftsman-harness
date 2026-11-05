use super::*;
use crate::exec::construction::{Options, SessionSelection};
use crate::exec::input::{Invocation, Run, Sources};
use crate::exec::permissions::{OnUnapproved, Selectors};
use crate::exec::provider::request::Fault;
use crate::exec::terminal::{Error, ReturnKind, Terminal};
use craftsman_harness::{FileData, Msg, ToolCall, ToolErrorKind};
use serde_json::{Value, json};
use std::path::PathBuf;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
const SEED: &str = "terminal-runtime-secret";
const CAP: usize = 1024 * 1024;
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
fn whole(t: &Terminal) -> Value {
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

fn size(source: &Value, omitted: usize) -> usize {
    let mut value = source.clone();
    let rows = source["messages"].as_array().unwrap();
    value["messages"] = json!(&rows[omitted..]);
    value["messages_omitted"] = json!(omitted);
    serde_json::to_vec(&value).unwrap().len()
}
fn selected(t: &Terminal, max: usize) -> Value {
    let s = t.selection(max).unwrap();
    let bytes = s.bytes().unwrap();
    assert!(bytes.len() <= max);
    let json: Value = serde_json::from_slice(bytes).unwrap();
    assert_eq!(
        json["messages"].as_array().unwrap().len(),
        s.message_count().unwrap()
    );
    assert_eq!(
        json["messages_omitted"].as_u64().unwrap() as usize,
        s.messages_omitted().unwrap()
    );
    json
}
fn error(t: &Terminal, max: usize, fault: Fault) {
    assert_eq!(t.selection(max).err(), Some(Error::Codec(fault)));
}
fn assistant(content: &str, args: Value) -> Msg {
    Msg::Assistant {
        content: content.into(),
        tool_calls: vec![ToolCall {
            id: "original-id".into(),
            name: "example".into(),
            args,
        }],
    }
}

#[tokio::test]
async fn all_messages_fit_with_exact_original_terminal_snapshot_bytes() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    let t = run.terminal();
    let snapshot = t.snapshot().unwrap();
    let bytes = snapshot.bytes().unwrap();
    let s = t.selection(bytes.len()).unwrap();
    assert_eq!(s.bytes().unwrap(), bytes);
    assert_eq!(s.facts().unwrap(), snapshot.facts().unwrap());
    assert_eq!(s.messages_omitted().unwrap(), 0);
    assert_eq!(
        s.message_count().unwrap(),
        snapshot.message_count().unwrap()
    );
    f.no_store();
}
#[tokio::test]
async fn empty_retained_state_still_preserves_the_original_checked_final_answer() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    state(&run).lock().unwrap().messages = vec![];
    let t = run.terminal();
    let source = whole(&t);
    assert_eq!(selected(&t, size(&source, 0)), source);
    assert_eq!(source["final_output"], "original answer [REDACTED]");
}
#[tokio::test]
async fn exact_last_record_budget_keeps_whole_suffix_and_full_original_output() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    let shared = state(&run);
    shared.lock().unwrap().messages = vec![Msg::user("old".repeat(100)), Msg::assistant("last")];
    let t = run.terminal();
    let source = whole(&t);
    let json = selected(&t, size(&source, 1));
    assert_eq!(json["messages"], json!([source["messages"][1].clone()]));
    assert_eq!(json["messages_omitted"], 1);
    assert_eq!(json["final_output"], source["final_output"]);
    assert_eq!(shared.lock().unwrap().messages.len(), 2);
}
#[tokio::test]
async fn one_byte_below_last_record_budget_omits_only_whole_message_not_answer() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    state(&run).lock().unwrap().messages =
        vec![Msg::user("old".repeat(100)), Msg::assistant("last")];
    let t = run.terminal();
    let source = whole(&t);
    let json = selected(&t, size(&source, 1) - 1);
    assert_eq!(json["messages"], json!([]));
    assert_eq!(json["messages_omitted"], 2);
    assert_eq!(json["final_output"], source["final_output"]);
    assert_eq!(json["run_return"], source["run_return"]);
}
#[tokio::test]
async fn exact_empty_message_budget_keeps_metadata_answer_and_truthful_omission_count() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    let t = run.terminal();
    let source = whole(&t);
    let n = source["messages"].as_array().unwrap().len();
    let json = selected(&t, size(&source, n));
    assert_eq!(json["messages"], json!([]));
    assert_eq!(json["messages_omitted"], n);
    assert_eq!(json["final_output"], source["final_output"]);
    error(&t, size(&source, n) - 1, Fault::Limit);
}
#[tokio::test]
async fn oversized_newest_record_does_not_select_older_smaller_records_around_it() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    state(&run).lock().unwrap().messages = vec![
        Msg::user("old small"),
        Msg::assistant("old small"),
        Msg::assistant("x".repeat(1000)),
    ];
    let t = run.terminal();
    let source = whole(&t);
    let json = selected(&t, size(&source, 3) + 40);
    assert_eq!(json["messages"], json!([]));
    assert_eq!(json["messages_omitted"], 3);
    assert_eq!(json["final_output"], source["final_output"]);
}
#[tokio::test]
async fn admitted_individual_channels_can_be_reduced_before_the_combined_frame_limit() {
    let f = Fixture::new(SEED).await;
    let output = "\u{0001}".repeat(110_000);
    f.response(&output).await;
    let run = f.run("input", &[]).await;
    assert_eq!(run.outcome().as_ref().unwrap().final_text(), output);
    let shared = state(&run);
    shared.lock().unwrap().messages = vec![Msg::user("\u{0001}".repeat(110_000))];
    let t = run.terminal();
    assert_eq!(t.snapshot().err(), Some(Error::Codec(Fault::Limit)));
    let json = selected(&t, CAP);
    assert_eq!(json["messages"], json!([]));
    assert_eq!(json["messages_omitted"], 1);
    assert_eq!(json["final_output"], output);
    assert_eq!(json["run_return"], "returned");
    assert_eq!(t.snapshot().err(), Some(Error::Codec(Fault::Limit)));
    assert_eq!(shared.lock().unwrap().messages.len(), 1);
    f.no_store();
}
#[tokio::test]
async fn caller_budgets_cannot_broaden_private_framing_or_drop_the_final_answer() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    let t = run.terminal();
    for max in [0, CAP + 1, usize::MAX] {
        error(&t, max, Fault::Limit);
    }
    assert!(t.selection(CAP).is_ok());
}
#[tokio::test]
async fn all_four_roles_flags_ids_keys_and_scalars_are_original_checked_whole_records() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    let shared = state(&run);
    let original = vec![
        Msg::System {
            content: "old system".into(),
        },
        Msg::user("user"),
        assistant(SEED, json!({"data":SEED,"number":7,"enabled":true})),
        Msg::Tool {
            tool_call_id: "original-id".into(),
            content: SEED.into(),
            is_error: true,
        },
    ];
    shared.lock().unwrap().messages = original.clone();
    let t = run.terminal();
    let source = whole(&t);
    let json = selected(&t, size(&source, 1));
    assert_eq!(
        json["messages"],
        json!(&source["messages"].as_array().unwrap()[1..])
    );
    assert_eq!(json["messages"][1]["tool_calls"][0]["id"], "original-id");
    assert_eq!(json["messages"][1]["tool_calls"][0]["args"]["number"], 7);
    assert_eq!(json["messages"][2]["is_error"], true);
    assert_eq!(shared.lock().unwrap().messages, original);
}
#[tokio::test]
async fn escaped_quoted_braces_arrays_and_nested_argument_shapes_are_not_record_cuts() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    let args = json!({"quoted\"key":[{"data":"\\\"}]},[\r\n\u{0000}","nested":[[true,null,8]]}],"中文":"🦀"});
    state(&run).lock().unwrap().messages = vec![
        Msg::user("old".repeat(100)),
        assistant("{[quoted\"]}", args.clone()),
    ];
    let t = run.terminal();
    let source = whole(&t);
    let json = selected(&t, size(&source, 1));
    assert_eq!(json["messages"][0]["tool_calls"][0]["args"], args);
}
#[tokio::test]
async fn malformed_string_and_non_object_args_are_preserved_not_repaired_or_executed() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    state(&run).lock().unwrap().messages = vec![
        Msg::user("old".repeat(100)),
        assistant("", json!(format!("{{bad {SEED}"))),
    ];
    let t = run.terminal();
    let source = whole(&t);
    assert_eq!(
        selected(&t, size(&source, 1))["messages"][0]["tool_calls"][0]["args"],
        "{bad [REDACTED]"
    );
}
#[tokio::test]
async fn unicode_budget_uses_exact_encoded_bytes_never_partial_scalars() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    state(&run).lock().unwrap().messages =
        vec![Msg::user("old".repeat(100)), Msg::assistant("中文🦀\r\n")];
    let t = run.terminal();
    let source = whole(&t);
    let exact = size(&source, 1);
    assert_eq!(selected(&t, exact)["messages"][0]["content"], "中文🦀\r\n");
    assert_eq!(selected(&t, exact - 1)["messages"], json!([]));
}
#[tokio::test]
async fn escaped_control_byte_lengths_not_original_text_lengths_determine_fit() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    state(&run).lock().unwrap().messages = vec![
        Msg::user("old".repeat(100)),
        Msg::assistant("\u{0001}".repeat(100)),
    ];
    let t = run.terminal();
    let source = whole(&t);
    let exact = size(&source, 1);
    assert_eq!(selected(&t, exact)["messages"].as_array().unwrap().len(), 1);
    assert_eq!(selected(&t, exact - 1)["messages"], json!([]));
}
#[tokio::test]
async fn redaction_markers_are_whole_data_or_the_complete_message_is_omitted() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    state(&run).lock().unwrap().messages = vec![Msg::user("old".repeat(100)), Msg::assistant(SEED)];
    let t = run.terminal();
    let source = whole(&t);
    assert_eq!(
        selected(&t, size(&source, 1))["messages"][0]["content"],
        "[REDACTED]"
    );
    assert_eq!(selected(&t, size(&source, 1) - 1)["messages"], json!([]));
}
#[tokio::test]
async fn omission_digit_growth_is_included_without_repeated_encoding_search() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    state(&run).lock().unwrap().messages = vec![Msg::user("whole".repeat(10)); 12];
    let t = run.terminal();
    let source = whole(&t);
    let json = selected(&t, size(&source, 10));
    assert_eq!(json["messages"].as_array().unwrap().len(), 2);
    assert_eq!(json["messages_omitted"], 10);
    assert_eq!(json["final_output"], source["final_output"]);
}
#[tokio::test]
async fn all_256_admitted_records_can_be_omitted_but_answer_and_facts_remain() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    let shared = state(&run);
    shared.lock().unwrap().messages = vec![Msg::user("whole"); 256];
    let t = run.terminal();
    let source = whole(&t);
    let json = selected(&t, size(&source, 256));
    assert_eq!(json["messages"], json!([]));
    assert_eq!(json["messages_omitted"], 256);
    assert_eq!(json["final_output"], source["final_output"]);
    assert_eq!(shared.lock().unwrap().messages.len(), 256);
}
#[tokio::test]
async fn unadmitted_message_count_cannot_be_rescued_by_a_small_selected_suffix() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    state(&run).lock().unwrap().messages = vec![Msg::user("safe"); 257];
    let t = run.terminal();
    error(&t, CAP, Fault::Limit);
}
#[tokio::test]
async fn oversized_original_message_is_not_copied_or_silently_omitted_around_admission() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    let shared = state(&run);
    let original = Msg::user("x".repeat(CAP + 1));
    shared.lock().unwrap().messages = vec![original.clone()];
    let t = run.terminal();
    error(&t, CAP, Fault::Limit);
    assert_eq!(shared.lock().unwrap().messages, [original]);
}
#[tokio::test]
async fn whole_original_message_encoded_limit_precedes_any_omission() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    state(&run).lock().unwrap().messages = vec![Msg::user("\u{0001}".repeat(200_000))];
    error(&run.terminal(), CAP, Fault::Limit);
}
#[tokio::test]
async fn original_final_output_source_limit_is_not_relaxed_to_fit_messages() {
    let f = Fixture::new(SEED).await;
    f.response(&"x".repeat(CAP + 1)).await;
    let run = f.run("input", &[]).await;
    assert_eq!(run.outcome().as_ref().unwrap().final_text().len(), CAP + 1);
    state(&run).lock().unwrap().messages = vec![];
    error(&run.terminal(), CAP, Fault::Limit);
}
#[tokio::test]
async fn final_output_work_limit_stays_whole_even_if_all_messages_are_omitted() {
    let f = Fixture::new(SEED).await;
    f.response(&"x".repeat(400_000)).await;
    let run = f.run("input", &[]).await;
    state(&run).lock().unwrap().messages = vec![];
    error(&run.terminal(), CAP, Fault::Limit);
}
#[tokio::test]
async fn an_unsafe_omitted_message_id_is_still_a_whole_source_refusal() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    state(&run).lock().unwrap().messages =
        vec![Msg::tool(SEED, "old"), Msg::assistant("safe last")];
    error(&run.terminal(), 200, Fault::Collision);
}
#[tokio::test]
async fn an_unsafe_omitted_argument_key_cannot_be_renamed_or_ignored() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    state(&run).lock().unwrap().messages = vec![
        assistant("", json!({SEED:"old"})),
        Msg::assistant("safe last"),
    ];
    error(&run.terminal(), 200, Fault::Collision);
}
#[tokio::test]
async fn call_identity_depth_and_node_caps_are_not_terminal_selection_exemptions() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    let shared = state(&run);
    let t = run.terminal();
    let call = ToolCall {
        id: "id".into(),
        name: "name".into(),
        args: json!({}),
    };
    shared.lock().unwrap().messages = vec![Msg::Assistant {
        content: "".into(),
        tool_calls: vec![call; 257],
    }];
    error(&t, CAP, Fault::Limit);
    shared.lock().unwrap().messages = vec![Msg::tool("字".repeat(129), "old")];
    error(&t, CAP, Fault::Limit);
    let mut args = Value::Null;
    for _ in 0..65 {
        args = json!([args]);
    }
    shared.lock().unwrap().messages = vec![assistant("", args)];
    error(&t, CAP, Fault::Limit);
    shared.lock().unwrap().messages = vec![assistant("", Value::Array(vec![Value::Null; 100_001]))];
    error(&t, CAP, Fault::Limit);
}
#[tokio::test]
async fn maximum_admitted_argument_depth_is_selectable_with_fixed_containers() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    let mut args = Value::Null;
    for _ in 0..64 {
        args = json!([args]);
    }
    state(&run).lock().unwrap().messages =
        vec![Msg::user("old".repeat(100)), assistant("", args.clone())];
    let t = run.terminal();
    let source = whole(&t);
    assert_eq!(
        selected(&t, size(&source, 1))["messages"][0]["tool_calls"][0]["args"],
        args
    );
}
#[tokio::test]
async fn new_omission_integer_collision_refuses_without_alternate_layout_or_count() {
    let f = Fixture::new("1").await;
    f.response("done").await;
    let run = f.run("input", &[]).await;
    state(&run).lock().unwrap().messages =
        vec![Msg::user("old".repeat(100)), Msg::assistant("last")];
    let t = run.terminal();
    let source = whole(&t);
    error(&t, size(&source, 1), Fault::Collision);
}
#[tokio::test]
async fn new_first_record_role_join_collision_is_not_hidden_by_more_omissions() {
    let f = Fixture::new(r#""messages":[{"role":"assistant"#).await;
    f.response("done").await;
    let run = f.run("input", &[]).await;
    state(&run).lock().unwrap().messages =
        vec![Msg::user("old".repeat(100)), Msg::assistant("last")];
    let t = run.terminal();
    let source = whole(&t);
    error(&t, size(&source, 1), Fault::Collision);
}
#[tokio::test]
async fn retained_terminal_header_and_new_message_boundary_are_checked_together() {
    let f = Fixture::new(
        r#""final_output":"done","message_scope":"retained_state","messages":[{"role":"assistant"#,
    )
    .await;
    f.response("done").await;
    let run = f.run("input", &[]).await;
    assert!(run.outcome().is_ok());
    state(&run).lock().unwrap().messages =
        vec![Msg::user("old".repeat(100)), Msg::assistant("last")];
    let t = run.terminal();
    let source = whole(&t);
    error(&t, size(&source, 1), Fault::Collision);
}
#[tokio::test]
async fn fixed_terminal_metadata_keys_cannot_be_omitted_to_avoid_credential_collision() {
    let f = Fixture::new("final_output").await;
    f.response("done").await;
    let run = f.run("input", &[]).await;
    error(&run.terminal(), CAP, Fault::Collision);
}
#[tokio::test]
async fn output_encoded_escaping_must_pass_before_message_selection() {
    let f = Fixture::new(r"escaped\nsecret").await;
    f.response("escaped\nsecret").await;
    let run = f.run("input", &[]).await;
    error(&run.terminal(), CAP, Fault::Collision);
}
#[tokio::test]
async fn actual_approval_terminal_selection_retains_partial_facts_without_reopening_execution() {
    let f = Fixture::new(SEED).await;
    f.approval_response().await;
    let run = f.run(&format!("input {SEED}"), &["write_file"]).await;
    assert!(matches!(run.outcome(), Err(RuntimeError::ApprovalRequired)));
    let g = gate(&run);
    let shared = state(&run);
    let original = shared.lock().unwrap().messages.clone();
    let t = run.terminal();
    let source = whole(&t);
    let n = source["messages"].as_array().unwrap().len();
    let json = selected(&t, size(&source, n));
    assert_eq!(json["messages"], json!([]));
    assert_eq!(json["messages_omitted"], n);
    assert_eq!(json["run_return"], "approval_required");
    assert_eq!(json["approval_required"], true);
    assert!(json["final_output"].is_null());
    assert!(g.execution_closed() && g.approval_required());
    assert_eq!(shared.lock().unwrap().messages, original);
    assert!(!f.workspace.join("later.txt").exists());
    assert_eq!(f.server.received_requests().await.unwrap().len(), 1);
    f.no_store();
}
#[tokio::test]
async fn actual_failure_null_output_is_not_changed_to_an_empty_answer_or_success() {
    let f = Fixture::new(SEED).await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(400)
                .set_body_raw(format!("raw {SEED} failure NeedsApproval"), "text/plain"),
        )
        .mount(&f.server)
        .await;
    let run = f.run("input", &[]).await;
    assert!(matches!(run.outcome(), Err(RuntimeError::Turn(_))));
    let t = run.terminal();
    let source = whole(&t);
    let n = source["messages"].as_array().unwrap().len();
    let json = selected(&t, size(&source, n));
    assert_eq!(json["run_return"], "turn_failed");
    assert_eq!(json["approval_required"], false);
    assert!(json["final_output"].is_null());
    assert_eq!(f.server.received_requests().await.unwrap().len(), 1);
}
#[tokio::test]
async fn actual_empty_returned_answer_remains_distinct_from_failure_null() {
    let f = Fixture::new(SEED).await;
    f.response("").await;
    let run = f.run("input", &[]).await;
    assert!(run.outcome().is_ok());
    let t = run.terminal();
    let source = whole(&t);
    let n = source["messages"].as_array().unwrap().len();
    assert_eq!(selected(&t, size(&source, n))["final_output"], "");
}
#[tokio::test]
async fn late_typed_approval_supersedes_all_old_selected_reads_and_keeps_real_return_fact() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    let g = gate(&run);
    let t = run.terminal();
    let s = t.selection(CAP).unwrap();
    g.observe_error_kind(ToolErrorKind::NeedsApproval);
    assert_eq!(s.bytes().err(), Some(Error::Superseded));
    assert_eq!(s.facts().err(), Some(Error::Superseded));
    assert_eq!(s.message_count(), Err(Error::Superseded));
    assert_eq!(s.messages_omitted(), Err(Error::Superseded));
    assert_eq!(s.bind(&t.scope().unwrap()), Err(Error::Superseded));
    let fresh = t.selection(CAP).unwrap();
    assert_eq!(fresh.facts().unwrap().returned(), ReturnKind::Returned);
    assert!(fresh.facts().unwrap().approval_required());
    assert!(g.execution_closed());
}
#[tokio::test]
async fn publication_close_hides_all_selected_reads_without_changing_approval_or_permissions() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    let g = gate(&run);
    let t = run.terminal();
    let s = t.selection(CAP).unwrap();
    t.close();
    assert_eq!(s.bytes().err(), Some(Error::Closed));
    assert_eq!(s.facts().err(), Some(Error::Closed));
    assert_eq!(s.message_count(), Err(Error::Closed));
    assert_eq!(s.messages_omitted(), Err(Error::Closed));
    assert_eq!(t.selection(CAP).err(), Some(Error::Closed));
    assert!(g.execution_closed());
    assert!(!g.approval_required());
}
#[tokio::test]
async fn actual_after_work_epoch_change_overrides_success_collision_and_limit() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    let g = gate(&run);
    let t = run.terminal();
    assert_eq!(
        t.selection_after_work_for_tests(CAP, || g
            .observe_error_kind(ToolErrorKind::NeedsApproval))
            .err(),
        Some(Error::Superseded)
    );
    let f = Fixture::new("final_output").await;
    f.response("data").await;
    let run = f.run("input", &[]).await;
    let g = gate(&run);
    let t = run.terminal();
    assert_eq!(
        t.selection_after_work_for_tests(CAP, || g
            .observe_error_kind(ToolErrorKind::NeedsApproval))
            .err(),
        Some(Error::Superseded)
    );
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    let g = gate(&run);
    let t = run.terminal();
    assert_eq!(
        t.selection_after_work_for_tests(0, || g.observe_error_kind(ToolErrorKind::NeedsApproval))
            .err(),
        Some(Error::Superseded)
    );
}
#[tokio::test]
async fn actual_after_work_publication_revocation_overrides_success_and_bound_failures() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    let t = run.terminal();
    assert_eq!(
        t.selection_after_work_for_tests(CAP, || t.close()).err(),
        Some(Error::Closed)
    );
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    state(&run).lock().unwrap().messages = vec![Msg::user("whole"); 257];
    let t = run.terminal();
    assert_eq!(
        t.selection_after_work_for_tests(CAP, || t.close()).err(),
        Some(Error::Closed)
    );
}
#[tokio::test]
async fn identical_fresh_terminal_owners_cannot_bind_selected_bytes_to_each_other() {
    let f = Fixture::new(SEED).await;
    f.response("same").await;
    let one = f.run("same", &[]).await.terminal();
    let two = f.run("same", &[]).await.terminal();
    let s = one.selection(CAP).unwrap();
    assert!(s.bind(&one.scope().unwrap()).is_ok());
    assert_eq!(s.bind(&two.scope().unwrap()), Err(Error::Foreign));
}
#[tokio::test]
async fn selected_snapshot_stays_immutable_after_later_retained_state_replacement() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    let shared = state(&run);
    shared.lock().unwrap().messages =
        vec![Msg::user("old".repeat(100)), Msg::assistant("old last")];
    let t = run.terminal();
    let source = whole(&t);
    let s = t.selection(size(&source, 1)).unwrap();
    let bytes = s.bytes().unwrap().to_vec();
    shared.lock().unwrap().messages = vec![Msg::user("new summary")];
    assert_eq!(s.bytes().unwrap(), bytes);
    assert_eq!(s.message_count().unwrap(), 1);
    assert_eq!(s.messages_omitted().unwrap(), 1);
    assert_eq!(selected(&t, CAP)["messages"][0]["content"], "new summary");
}
#[tokio::test]
async fn selection_does_not_serialize_other_raw_state_or_public_status_usage_provenance() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    let shared = state(&run);
    {
        let mut st = shared.lock().unwrap();
        st.files.insert("private".into(), FileData::text(SEED));
        st.custom.insert("origin".into(), json!({"data":SEED}));
        st.system_extras.push(SEED.into());
    }
    let t = run.terminal();
    let source = whole(&t);
    let n = source["messages"].as_array().unwrap().len();
    let json = selected(&t, size(&source, n));
    assert_eq!(json.as_object().unwrap().len(), 6);
    for k in [
        "files",
        "todos",
        "custom",
        "origin",
        "usage",
        "status",
        "exit_code",
        "verification",
    ] {
        assert!(json.get(k).is_none());
    }
    assert_eq!(shared.lock().unwrap().files["private"].content, SEED);
    f.no_store();
}
#[tokio::test]
async fn actual_final_answer_is_not_derived_from_selected_last_message_or_new_configuration() {
    let f = Fixture::new(SEED).await;
    f.response(&format!("{SEED} later-key")).await;
    let run = f.run("input", &[]).await;
    state(&run).lock().unwrap().messages = vec![Msg::assistant("different last")];
    f.configure("later-key");
    let t = run.terminal();
    let source = whole(&t);
    let json = selected(&t, size(&source, 1));
    assert_eq!(json["final_output"], "[REDACTED] later-key");
    assert_eq!(json["messages"], json!([]));
}
#[tokio::test]
async fn selected_terminal_data_never_reopens_actual_compaction_or_model_traffic() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    let g = gate(&run);
    let compaction = run.runtime_for_tests().session.compaction.clone();
    let shared = state(&run);
    let t = run.terminal();
    let source = whole(&t);
    let n = source["messages"].as_array().unwrap().len();
    assert!(t.selection(size(&source, n)).is_ok());
    let original = shared.lock().unwrap().messages.clone();
    assert!(compaction.compact_now(&original).await.is_err());
    assert!(g.execution_closed());
    assert!(!g.approval_required());
    assert_eq!(f.server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn an_unsafe_omitted_numeric_argument_is_refused_not_changed_to_text_or_ignored() {
    let f = Fixture::new("42").await;
    f.response("done").await;
    let run = f.run("input", &[]).await;
    state(&run).lock().unwrap().messages = vec![
        assistant("", json!({"number":42})),
        Msg::assistant("safe last"),
    ];
    error(&run.terminal(), 200, Fault::Collision);
}
