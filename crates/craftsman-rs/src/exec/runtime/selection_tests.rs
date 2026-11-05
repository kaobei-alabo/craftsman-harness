use super::*;
use crate::exec::construction::{Options, SessionSelection};
use crate::exec::input::{Invocation, Sources};
use crate::exec::permissions::{OnUnapproved, Selectors};
use crate::exec::provider::request::Fault;
use craftsman_harness::{FileData, Msg, ToolCall, ToolErrorKind};
use serde_json::{Value, json};
use std::path::PathBuf;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
const CAP: usize = 1024 * 1024;
const SEED: &str = "retained-selection-secret";

struct Fixture {
    _root: tempfile::TempDir,
    workspace: PathBuf,
    config: PathBuf,
}
impl Fixture {
    fn new(seed: &str) -> Self {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("workspace");
        let config = root.path().join("selected-config");
        std::fs::create_dir(&workspace).unwrap();
        std::fs::create_dir(&config).unwrap();
        let f = Self {
            _root: root,
            workspace,
            config,
        };
        f.configure(seed, "http://127.0.0.1:1");
        f
    }
    fn configure(&self, seed: &str, url: &str) {
        std::fs::write(
            self.config.join("config.json"),
            json!({"model":"synthetic-model","api_key":seed,"base_url":url}).to_string(),
        )
        .unwrap();
    }
    fn options(&self) -> Options {
        Options {
            workspace: self.workspace.clone(),
            config_dir: self.config.clone(),
            trust_workspace: false,
            session: SessionSelection::Ephemeral,
            selectors: Selectors {
                on_unapproved: OnUnapproved::Fail,
                ..Default::default()
            },
        }
    }
    async fn runtime(&self) -> Runtime {
        Runtime::build(Prepared::prepare(self.options()).await.unwrap())
            .await
            .unwrap()
    }
    fn no_store(&self) {
        assert!(!self.config.join("threads.db").exists());
        assert!(!self.config.join("sessions-rs").exists());
    }
}
fn messages(r: &Runtime, values: Vec<Msg>) {
    r.session.agent.state.lock().unwrap().messages = values;
}
fn full(r: &Runtime) -> Value {
    serde_json::from_slice(r.message_view().unwrap().bytes().unwrap()).unwrap()
}
fn size(full: &Value, omitted: usize) -> usize {
    let rows = full["messages"].as_array().unwrap();
    serde_json::to_vec(&json!({"message_scope":"retained_state","messages":&rows[omitted..],"messages_omitted":omitted})).unwrap().len()
}
fn selected(r: &Runtime, max: usize) -> Value {
    let s = r.message_selection(max).unwrap();
    assert!(s.bytes().unwrap().len() <= max);
    let json: Value = serde_json::from_slice(s.bytes().unwrap()).unwrap();
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
fn error(r: &Runtime, max: usize, expected: Fault) {
    assert_eq!(r.message_selection(max).err(), Some(expected));
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
async fn empty_checked_snapshot_fits_exact_framing_and_never_fabricates_messages() {
    let f = Fixture::new(SEED);
    let r = f.runtime().await;
    let original = full(&r);
    let max = size(&original, 0);
    assert_eq!(selected(&r, max), original);
    error(&r, max - 1, Fault::Limit);
    f.no_store();
}
#[tokio::test]
async fn selecting_all_reuses_the_exact_original_checked_frame_bytes() {
    let f = Fixture::new(SEED);
    let r = f.runtime().await;
    messages(
        &r,
        vec![
            Msg::user(format!("original {SEED}")),
            Msg::assistant("done"),
        ],
    );
    let view = r.message_view().unwrap();
    let original = view.bytes().unwrap().to_vec();
    let selection = view.select(original.len()).unwrap();
    assert_eq!(selection.bytes().unwrap(), original);
    assert_eq!(selection.message_count().unwrap(), 2);
    assert_eq!(selection.messages_omitted().unwrap(), 0);
    f.no_store();
}
#[tokio::test]
async fn zero_budget_is_refusal_not_a_truncated_or_repaired_empty_json_prefix() {
    let f = Fixture::new(SEED);
    let r = f.runtime().await;
    messages(&r, vec![Msg::user("data")]);
    error(&r, 0, Fault::Limit);
}
#[tokio::test]
async fn caller_budgets_cannot_broaden_the_private_capture_or_frame_cap() {
    let f = Fixture::new(SEED);
    let r = f.runtime().await;
    error(&r, CAP + 1, Fault::Limit);
    error(&r, usize::MAX, Fault::Limit);
    assert!(r.message_selection(CAP).is_ok());
}
#[tokio::test]
async fn all_four_roles_select_as_whole_original_ordered_suffix_records() {
    let f = Fixture::new(SEED);
    let r = f.runtime().await;
    let original = vec![
        Msg::System {
            content: "oldest system".into(),
        },
        Msg::user("user"),
        assistant("assistant", json!({"data":SEED,"number":7,"enabled":true})),
        Msg::Tool {
            tool_call_id: "original-id".into(),
            content: SEED.into(),
            is_error: true,
        },
    ];
    messages(&r, original.clone());
    let source = full(&r);
    let s = selected(&r, size(&source, 1));
    assert_eq!(
        s["messages"],
        json!(&source["messages"].as_array().unwrap()[1..])
    );
    assert_eq!(s["messages_omitted"], 1);
    assert_eq!(s["messages"][1]["tool_calls"][0]["id"], "original-id");
    assert_eq!(s["messages"][1]["tool_calls"][0]["args"]["number"], 7);
    assert_eq!(s["messages"][2]["is_error"], true);
    assert_eq!(r.retained_state().messages, original);
    f.no_store();
}
#[tokio::test]
async fn newest_contiguous_suffix_is_kept_not_old_small_messages_around_a_large_last_record() {
    let f = Fixture::new(SEED);
    let r = f.runtime().await;
    messages(
        &r,
        vec![
            Msg::user("old small"),
            Msg::assistant("old small"),
            Msg::user("x".repeat(1000)),
        ],
    );
    let source = full(&r);
    let s = selected(&r, size(&source, 3));
    assert_eq!(s["messages"], json!([]));
    assert_eq!(s["messages_omitted"], 3);
    assert_eq!(r.retained_state().messages.len(), 3);
}
#[tokio::test]
async fn exact_last_message_encoded_budget_retains_the_whole_message() {
    let f = Fixture::new(SEED);
    let r = f.runtime().await;
    messages(
        &r,
        vec![Msg::user("old".repeat(100)), Msg::assistant("last")],
    );
    let source = full(&r);
    let s = selected(&r, size(&source, 1));
    assert_eq!(s["messages"], json!([source["messages"][1].clone()]));
    assert_eq!(s["messages_omitted"], 1);
}
#[tokio::test]
async fn one_byte_below_last_message_budget_omits_whole_record_never_its_tail() {
    let f = Fixture::new(SEED);
    let r = f.runtime().await;
    messages(
        &r,
        vec![Msg::user("old".repeat(100)), Msg::assistant("last")],
    );
    let source = full(&r);
    let s = selected(&r, size(&source, 1) - 1);
    assert_eq!(s["messages"], json!([]));
    assert_eq!(s["messages_omitted"], 2);
}
#[tokio::test]
async fn omission_integer_digit_growth_is_included_in_exact_layout_arithmetic() {
    let f = Fixture::new(SEED);
    let r = f.runtime().await;
    messages(&r, vec![Msg::user("whole".repeat(10)); 12]);
    let source = full(&r);
    let s = selected(&r, size(&source, 10));
    assert_eq!(s["messages"].as_array().unwrap().len(), 2);
    assert_eq!(s["messages_omitted"], 10);
    assert!(size(&source, 9) > size(&source, 10));
}
#[tokio::test]
async fn encoded_escaping_counts_not_decoded_text_or_scalar_lengths() {
    let f = Fixture::new(SEED);
    let r = f.runtime().await;
    messages(
        &r,
        vec![
            Msg::user("old".repeat(100)),
            Msg::assistant("\u{0001}".repeat(100)),
        ],
    );
    let source = full(&r);
    let exact = size(&source, 1);
    assert_eq!(selected(&r, exact)["messages"].as_array().unwrap().len(), 1);
    assert_eq!(selected(&r, exact - 1)["messages"], json!([]));
}
#[tokio::test]
async fn unicode_byte_boundaries_never_clip_a_scalar_or_decode_lossily() {
    let f = Fixture::new(SEED);
    let r = f.runtime().await;
    messages(
        &r,
        vec![Msg::user("old".repeat(100)), Msg::assistant("中文🦀\r\n")],
    );
    let source = full(&r);
    let exact = size(&source, 1);
    assert_eq!(selected(&r, exact)["messages"][0]["content"], "中文🦀\r\n");
    assert_eq!(selected(&r, exact - 1)["messages"], json!([]));
}
#[tokio::test]
async fn nested_argument_quoted_braces_arrays_and_escapes_are_not_record_boundaries() {
    let f = Fixture::new(SEED);
    let r = f.runtime().await;
    let args = json!({"quoted\"key":[{"data":"\\\"}]},[\r\n\u{0000}","more":[[true,null,8]]}],"中文":"🦀"});
    messages(
        &r,
        vec![
            Msg::user("old".repeat(100)),
            assistant("{[quoted\"]}", args.clone()),
        ],
    );
    let source = full(&r);
    let s = selected(&r, size(&source, 1));
    assert_eq!(s["messages"][0]["tool_calls"][0]["args"], args);
    assert_eq!(s["messages"][0], source["messages"][1]);
}
#[tokio::test]
async fn malformed_string_and_non_object_argument_shapes_stay_typed_original_shapes() {
    let f = Fixture::new(SEED);
    let r = f.runtime().await;
    messages(
        &r,
        vec![
            Msg::user("old".repeat(100)),
            assistant("", json!(format!("{{bad {SEED}"))),
        ],
    );
    let source = full(&r);
    let s = selected(&r, size(&source, 1));
    assert_eq!(s["messages"][0]["tool_calls"][0]["args"], "{bad [REDACTED]");
    messages(&r, vec![assistant("", json!([SEED, null, 3]))]);
    assert_eq!(
        selected(&r, CAP)["messages"][0]["tool_calls"][0]["args"],
        json!(["[REDACTED]", null, 3])
    );
}
#[tokio::test]
async fn empty_assistant_content_and_empty_calls_keep_the_checked_original_shape() {
    let f = Fixture::new(SEED);
    let r = f.runtime().await;
    messages(&r, vec![Msg::user("old".repeat(100)), Msg::assistant("")]);
    let source = full(&r);
    assert_eq!(
        selected(&r, size(&source, 1))["messages"][0],
        json!({"role":"assistant","content":""})
    );
}
#[tokio::test]
async fn all_256_admitted_messages_can_be_omitted_with_an_exact_truthful_count() {
    let f = Fixture::new(SEED);
    let r = f.runtime().await;
    messages(&r, vec![Msg::user("whole"); 256]);
    let source = full(&r);
    let s = selected(&r, size(&source, 256));
    assert_eq!(s["messages"], json!([]));
    assert_eq!(s["messages_omitted"], 256);
    assert_eq!(r.retained_state().messages.len(), 256);
}
#[tokio::test]
async fn selection_of_an_older_view_is_a_snapshot_not_later_retained_state_or_history() {
    let f = Fixture::new(SEED);
    let r = f.runtime().await;
    messages(
        &r,
        vec![
            Msg::user("old".repeat(100)),
            Msg::assistant("original retained"),
        ],
    );
    let source = full(&r);
    let view = r.message_view().unwrap();
    messages(&r, vec![Msg::user("new retained summary")]);
    let s = view.select(size(&source, 1)).unwrap();
    let json: Value = serde_json::from_slice(s.bytes().unwrap()).unwrap();
    assert_eq!(json["messages"][0]["content"], "original retained");
    assert_eq!(s.messages_omitted().unwrap(), 1);
    assert_eq!(
        r.retained_state().messages,
        [Msg::user("new retained summary")]
    );
}
#[tokio::test]
async fn frozen_selection_seeds_do_not_reload_replaced_config_or_add_new_credentials() {
    let f = Fixture::new(SEED);
    let r = f.runtime().await;
    f.configure("later-key", "http://127.0.0.1:2");
    messages(
        &r,
        vec![
            Msg::user("old".repeat(100)),
            Msg::assistant(format!("{SEED} later-key")),
        ],
    );
    let source = full(&r);
    assert_eq!(
        selected(&r, size(&source, 1))["messages"][0]["content"],
        "[REDACTED] later-key"
    );
    f.no_store();
}
#[tokio::test]
async fn selected_frame_binding_rejects_equal_fresh_runtime_state_and_permissions() {
    let f = Fixture::new(SEED);
    let r = f.runtime().await;
    let other = f.runtime().await;
    messages(&r, vec![Msg::user("same")]);
    messages(&other, vec![Msg::user("same")]);
    let s = r.message_selection(CAP).unwrap();
    assert!(s.bind(&r.message_scope()).is_ok());
    assert_eq!(s.bind(&other.message_scope()), Err(Fault::Collision));
}
#[tokio::test]
async fn late_typed_stop_hides_selected_bytes_counts_omissions_and_binding() {
    let f = Fixture::new(SEED);
    let r = f.runtime().await;
    messages(&r, vec![Msg::user("original")]);
    let s = r.message_selection(CAP).unwrap();
    r.gate.observe_error_kind(ToolErrorKind::NeedsApproval);
    assert_eq!(s.bytes().err(), Some(Fault::Stopped));
    assert_eq!(s.message_count(), Err(Fault::Stopped));
    assert_eq!(s.messages_omitted(), Err(Fault::Stopped));
    assert_eq!(s.bind(&r.message_scope()), Err(Fault::Stopped));
    error(&r, CAP, Fault::Stopped);
    assert_eq!(r.retained_state().messages, [Msg::user("original")]);
    f.no_store();
}
#[tokio::test]
async fn failure_strings_and_tool_error_booleans_never_latch_or_authorize_selection() {
    let f = Fixture::new(SEED);
    let r = f.runtime().await;
    messages(
        &r,
        vec![Msg::Tool {
            tool_call_id: "id".into(),
            content: "NeedsApproval requires approval denied".into(),
            is_error: true,
        }],
    );
    assert!(r.message_selection(CAP).is_ok());
    assert!(!r.gate.approval_required());
}
#[tokio::test]
async fn an_omitted_credential_colliding_identity_is_not_a_source_admission_bypass() {
    let f = Fixture::new(SEED);
    let r = f.runtime().await;
    messages(
        &r,
        vec![Msg::tool(SEED, "old"), Msg::assistant("safe last")],
    );
    error(&r, 200, Fault::Collision);
}
#[tokio::test]
async fn an_omitted_credential_colliding_argument_key_is_still_refused_whole() {
    let f = Fixture::new(SEED);
    let r = f.runtime().await;
    messages(
        &r,
        vec![
            assistant("", json!({SEED:"old"})),
            Msg::assistant("safe last"),
        ],
    );
    error(&r, 200, Fault::Collision);
}
#[tokio::test]
async fn an_omitted_credential_colliding_numeric_scalar_is_not_rewritten_or_ignored() {
    let f = Fixture::new("42");
    let r = f.runtime().await;
    messages(
        &r,
        vec![
            assistant("", json!({"number":42})),
            Msg::assistant("safe last"),
        ],
    );
    error(&r, 200, Fault::Collision);
}
#[tokio::test]
async fn original_generated_schema_literal_collisions_cannot_be_hidden_by_selection() {
    let f = Fixture::new("role");
    let r = f.runtime().await;
    messages(&r, vec![Msg::user("data")]);
    error(&r, 100, Fault::Collision);
}
#[tokio::test]
async fn private_source_message_cap_is_not_broadened_by_omitting_an_unadmitted_prefix() {
    let f = Fixture::new(SEED);
    let r = f.runtime().await;
    messages(&r, vec![Msg::user("safe"); 257]);
    error(&r, 100, Fault::Limit);
}
#[tokio::test]
async fn unadmitted_oversized_original_content_cannot_be_rescued_by_an_empty_selection() {
    let f = Fixture::new(SEED);
    let r = f.runtime().await;
    let original = Msg::user("x".repeat(CAP + 1));
    messages(&r, vec![original.clone(), Msg::assistant("safe")]);
    error(&r, 100, Fault::Limit);
    assert_eq!(r.retained_state().messages[0], original);
}
#[tokio::test]
async fn original_encoded_frame_bound_must_pass_before_any_message_omission() {
    let f = Fixture::new(SEED);
    let r = f.runtime().await;
    messages(
        &r,
        vec![
            Msg::user("\u{0001}".repeat(200_000)),
            Msg::assistant("safe"),
        ],
    );
    error(&r, 100, Fault::Limit);
}
#[tokio::test]
async fn original_call_count_bound_remains_whole_even_for_an_omitted_assistant() {
    let f = Fixture::new(SEED);
    let r = f.runtime().await;
    let call = ToolCall {
        id: "id".into(),
        name: "example".into(),
        args: json!({}),
    };
    messages(
        &r,
        vec![
            Msg::Assistant {
                content: "".into(),
                tool_calls: vec![call; 257],
            },
            Msg::assistant("safe"),
        ],
    );
    error(&r, 100, Fault::Limit);
}
#[tokio::test]
async fn original_argument_depth_bound_is_not_bypassed_by_suffix_selection() {
    let f = Fixture::new(SEED);
    let r = f.runtime().await;
    let mut args = Value::Null;
    for _ in 0..65 {
        args = json!([args]);
    }
    messages(&r, vec![assistant("", args), Msg::assistant("safe")]);
    error(&r, 100, Fault::Limit);
}
#[tokio::test]
async fn original_argument_node_bound_precedes_whole_message_selection() {
    let f = Fixture::new(SEED);
    let r = f.runtime().await;
    messages(
        &r,
        vec![
            assistant("", Value::Array(vec![Value::Null; 100_001])),
            Msg::assistant("safe"),
        ],
    );
    error(&r, 100, Fault::Limit);
}
#[tokio::test]
async fn a_new_omission_integer_credential_collision_refuses_without_alternate_layouts() {
    let f = Fixture::new("1");
    let r = f.runtime().await;
    messages(
        &r,
        vec![Msg::user("old".repeat(100)), Msg::assistant("last")],
    );
    let source = full(&r);
    assert!(r.message_view().is_ok());
    error(&r, size(&source, 1), Fault::Collision);
    assert!(!r.gate.approval_required());
}
#[tokio::test]
async fn moving_a_later_role_to_first_can_create_a_new_framing_literal_and_must_refuse() {
    let f = Fixture::new(r#""messages":[{"role":"assistant"#);
    let r = f.runtime().await;
    messages(
        &r,
        vec![Msg::user("old".repeat(100)), Msg::assistant("last")],
    );
    let source = full(&r);
    assert!(r.message_view().is_ok());
    error(&r, size(&source, 1), Fault::Collision);
}
#[tokio::test]
async fn expanded_redaction_markers_are_kept_whole_or_the_whole_record_is_omitted() {
    let f = Fixture::new(SEED);
    let r = f.runtime().await;
    messages(&r, vec![Msg::user("old".repeat(100)), Msg::assistant(SEED)]);
    let source = full(&r);
    assert_eq!(
        selected(&r, size(&source, 1))["messages"][0]["content"],
        "[REDACTED]"
    );
    assert_eq!(selected(&r, size(&source, 1) - 1)["messages"], json!([]));
}
#[tokio::test]
async fn a_real_same_gate_stop_after_successful_codec_work_refuses_selection() {
    let f = Fixture::new(SEED);
    let r = f.runtime().await;
    messages(&r, vec![Msg::user("data")]);
    let view = r.message_view().unwrap();
    assert_eq!(
        view.select_after_work_for_tests(CAP, || {
            r.gate.observe_error_kind(ToolErrorKind::NeedsApproval);
        })
        .err(),
        Some(Fault::Stopped)
    );
}
#[tokio::test]
async fn a_real_same_gate_stop_after_omission_collision_overrides_the_codec_error() {
    let f = Fixture::new("1");
    let r = f.runtime().await;
    messages(
        &r,
        vec![Msg::user("old".repeat(100)), Msg::assistant("last")],
    );
    let source = full(&r);
    let view = r.message_view().unwrap();
    assert_eq!(
        view.select_after_work_for_tests(size(&source, 1), || {
            r.gate.observe_error_kind(ToolErrorKind::NeedsApproval);
        })
        .err(),
        Some(Fault::Stopped)
    );
}
#[tokio::test]
async fn a_real_same_gate_stop_after_a_bound_failure_overrides_the_fixed_limit() {
    let f = Fixture::new(SEED);
    let r = f.runtime().await;
    let view = r.message_view().unwrap();
    assert_eq!(
        view.select_after_work_for_tests(0, || {
            r.gate.observe_error_kind(ToolErrorKind::NeedsApproval);
        })
        .err(),
        Some(Fault::Stopped)
    );
}
#[tokio::test]
async fn selected_messages_do_not_implicitly_publish_other_original_state_or_final_outcome() {
    let f = Fixture::new(SEED);
    let r = f.runtime().await;
    {
        let mut state = r.session.agent.state.lock().unwrap();
        state.messages = vec![Msg::user("only selected message")];
        state
            .files
            .insert("private-file".into(), FileData::text(SEED));
        state
            .custom
            .insert("private-status".into(), json!({"status":SEED}));
        state.system_extras.push(SEED.into());
    }
    let json = selected(&r, CAP);
    assert_eq!(json.as_object().unwrap().len(), 3);
    assert!(json.get("output").is_none());
    assert!(json.get("status").is_none());
    assert!(json.get("usage").is_none());
    let raw = r.retained_state();
    assert_eq!(raw.files["private-file"].content, SEED);
    assert_eq!(raw.system_extras, [SEED]);
    f.no_store();
}
#[tokio::test]
async fn actual_admitted_input_run_selects_checked_messages_without_changing_original_outcome() {
    let f = Fixture::new(SEED);
    let server = MockServer::start().await;
    f.configure(SEED, &server.uri());
    let event = json!({"choices":[{"delta":{"content":format!("original answer {SEED}")},"finish_reason":"stop"}]});
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            format!("data: {event}\n\ndata: [DONE]\n\n").into_bytes(),
            "text/event-stream",
        ))
        .mount(&server)
        .await;
    let prompt = format!("original input {SEED}");
    let run = Invocation::prepare(
        f.options(),
        Sources {
            prompt: Some(prompt.as_bytes()),
            prompt_file: None,
            files: &[],
        },
    )
    .await
    .unwrap()
    .run()
    .await;
    assert!(run.outcome().is_ok());
    let original_answer = format!("original answer {SEED}");
    assert_eq!(
        run.outcome().as_ref().unwrap().final_text(),
        original_answer
    );
    let view = run.message_view().unwrap();
    let source: Value = serde_json::from_slice(view.bytes().unwrap()).unwrap();
    let count = source["messages"].as_array().unwrap().len();
    let s = run.message_selection(size(&source, count)).unwrap();
    assert_eq!(s.message_count().unwrap(), 0);
    assert_eq!(s.messages_omitted().unwrap(), count);
    assert_eq!(
        run.outcome().as_ref().unwrap().final_text(),
        original_answer
    );
    assert!(run.outcome().is_ok());
    assert!(
        run.retained_state()
            .messages
            .iter()
            .any(|m| matches!(m,Msg::Assistant{content,..} if content.contains(SEED)))
    );
    assert!(
        run.retained_state()
            .messages
            .iter()
            .any(|m| matches!(m,Msg::User{content} if content==&prompt))
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
    f.no_store();
}

#[tokio::test]
async fn maximum_admitted_argument_depth_still_fits_fixed_message_and_call_containers() {
    let f = Fixture::new(SEED);
    let r = f.runtime().await;
    let mut args = Value::Null;
    for _ in 0..64 {
        args = json!([args]);
    }
    messages(&r, vec![Msg::user("old".repeat(100)), assistant("", args)]);
    let source = full(&r);
    let s = selected(&r, size(&source, 1));
    assert_eq!(s["messages"][0], source["messages"][1]);
}
