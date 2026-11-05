use super::*;
use crate::exec::construction::{Options, SessionSelection};
use crate::exec::input::{Invocation, Sources};
use crate::exec::permissions::{OnUnapproved, Selectors};
use crate::exec::provider::request::Fault;
use craftsman_harness::{FileData, Msg, Todo, TodoStatus, ToolCall, ToolErrorKind};
use serde_json::{Value, json};
use std::path::PathBuf;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

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
fn messages(runtime: &Runtime, values: Vec<Msg>) {
    runtime.session.agent.state.lock().unwrap().messages = values;
}
fn value(runtime: &Runtime) -> Value {
    let view = runtime.message_view().unwrap();
    serde_json::from_slice(view.bytes().unwrap()).unwrap()
}
fn call(id: &str, name: &str, args: Value) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: name.into(),
        args,
    }
}
fn assistant(content: &str, calls: Vec<ToolCall>) -> Msg {
    Msg::Assistant {
        content: content.into(),
        tool_calls: calls,
    }
}
fn error(runtime: &Runtime, expected: Fault) {
    assert_eq!(runtime.message_view().err(), Some(expected));
}

#[tokio::test]
async fn empty_actual_retained_state_has_a_checked_empty_snapshot_not_a_fabricated_transcript() {
    let f = Fixture::new("retained-runtime-secret");
    let r = f.runtime().await;
    let view = r.message_view().unwrap();
    assert_eq!(view.message_count().unwrap(), 0);
    let json: Value = serde_json::from_slice(view.bytes().unwrap()).unwrap();
    assert_eq!(
        json,
        json!({"message_scope":"retained_state","messages":[],"messages_omitted":0})
    );
    f.no_store();
}

#[tokio::test]
async fn all_four_original_message_roles_and_tool_error_flags_have_separate_checked_text() {
    let seed = "retained-runtime-secret";
    let f = Fixture::new(seed);
    let r = f.runtime().await;
    let original = vec![
        Msg::System {
            content: format!("system {seed}"),
        },
        Msg::user(format!("user {seed}")),
        Msg::assistant(format!("assistant {seed}")),
        Msg::Tool {
            tool_call_id: "original-call".into(),
            content: format!("tool {seed}"),
            is_error: true,
        },
    ];
    messages(&r, original.clone());
    let view = r.message_view().unwrap();
    assert_eq!(view.message_count().unwrap(), 4);
    let json: Value = serde_json::from_slice(view.bytes().unwrap()).unwrap();
    let rows = json["messages"].as_array().unwrap();
    assert_eq!(
        rows.iter()
            .map(|m| m["role"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["system", "user", "assistant", "tool"]
    );
    assert!(
        rows.iter()
            .all(|m| m["content"].as_str().unwrap().contains("[REDACTED]"))
    );
    assert_eq!(rows[3]["tool_call_id"], "original-call");
    assert_eq!(rows[3]["is_error"], true);
    assert_eq!(r.retained_state().messages, original);
    f.no_store();
}

#[tokio::test]
async fn decoded_argument_data_is_redacted_before_encoding_without_mutating_original_args() {
    let seed = "retained-runtime-secret";
    let f = Fixture::new(seed);
    let r = f.runtime().await;
    let args = json!({"file_path":format!("{seed}.txt"),"content":format!("ORIGINAL {seed} 中文\r\n"),"nested":[{"data":seed}],"number":7,"enabled":true});
    messages(
        &r,
        vec![assistant(
            "answer",
            vec![call("original-id", "write_file", args.clone())],
        )],
    );
    let json = value(&r);
    let view = &json["messages"][0]["tool_calls"][0];
    assert_eq!(view["id"], "original-id");
    assert_eq!(view["name"], "write_file");
    assert_eq!(view["args"]["file_path"], "[REDACTED].txt");
    assert_eq!(view["args"]["nested"][0]["data"], "[REDACTED]");
    assert_eq!(view["args"]["number"], 7);
    assert_eq!(view["args"]["enabled"], true);
    let state = r.retained_state();
    let Msg::Assistant { tool_calls, .. } = &state.messages[0] else {
        panic!("assistant");
    };
    assert_eq!(tool_calls[0].args, args);
    assert!(!f.workspace.join("[REDACTED].txt").exists());
    f.no_store();
}

#[tokio::test]
async fn non_object_and_malformed_string_argument_shapes_are_not_repaired_or_executed_from_views() {
    let seed = "retained-runtime-secret";
    let f = Fixture::new(seed);
    let r = f.runtime().await;
    let original = vec![assistant(
        "",
        vec![
            call("array-id", "example", json!([seed, 4])),
            call("string-id", "example", json!(format!("{{bad {seed}"))),
            call("null-id", "example", Value::Null),
        ],
    )];
    messages(&r, original.clone());
    let json = value(&r);
    let calls = json["messages"][0]["tool_calls"].as_array().unwrap();
    assert_eq!(calls[0]["args"], json!(["[REDACTED]", 4]));
    assert_eq!(calls[1]["args"], "{bad [REDACTED]");
    assert!(calls[2]["args"].is_null());
    assert_eq!(r.retained_state().messages, original);
}

#[tokio::test]
async fn assistant_empty_content_and_empty_call_list_keep_the_original_message_shape() {
    let f = Fixture::new("retained-runtime-secret");
    let r = f.runtime().await;
    messages(&r, vec![assistant("", vec![])]);
    let json = value(&r);
    assert_eq!(
        json["messages"][0],
        json!({"role":"assistant","content":""})
    );
}

#[tokio::test]
async fn credential_colliding_call_ids_tool_names_and_tool_result_ids_are_refused_not_aliased() {
    let seed = "retained-runtime-secret";
    let f = Fixture::new(seed);
    let r = f.runtime().await;
    for msg in [
        assistant("", vec![call(seed, "example", json!({}))]),
        assistant("", vec![call("id", seed, json!({}))]),
        Msg::tool(seed, "result"),
    ] {
        messages(&r, vec![msg.clone()]);
        error(&r, Fault::Collision);
        assert_eq!(r.retained_state().messages, [msg]);
    }
}

#[tokio::test]
async fn credential_colliding_decoded_argument_member_names_refuse_the_whole_snapshot() {
    let seed = "retained-runtime-secret";
    let f = Fixture::new(seed);
    let r = f.runtime().await;
    messages(
        &r,
        vec![assistant(
            "",
            vec![call("id", "example", json!({"nested":{seed:"data"}}))],
        )],
    );
    error(&r, Fault::Collision);
}

#[tokio::test]
async fn argument_member_names_preserve_original_unicode_and_never_become_view_aliases() {
    let f = Fixture::new("retained-runtime-secret");
    let r = f.runtime().await;
    let args = json!({"中文": {"quoted\"key":"value"},"array":[1,2]});
    messages(
        &r,
        vec![assistant("", vec![call("id", "example", args.clone())])],
    );
    assert_eq!(value(&r)["messages"][0]["tool_calls"][0]["args"], args);
}

#[tokio::test]
async fn credential_colliding_numeric_scalar_is_not_converted_to_a_string_or_rewritten() {
    let f = Fixture::new("42");
    let r = f.runtime().await;
    messages(
        &r,
        vec![assistant(
            "",
            vec![call("id", "example", json!({"number":42}))],
        )],
    );
    error(&r, Fault::Collision);
}

#[tokio::test]
async fn encoded_escaping_cannot_create_a_new_credential_after_data_redaction() {
    let f = Fixture::new(r"escaped\nsecret");
    let r = f.runtime().await;
    messages(&r, vec![Msg::user("escaped\nsecret")]);
    error(&r, Fault::Collision);
}

#[tokio::test]
async fn generated_role_and_schema_key_collisions_refuse_instead_of_renaming_structure() {
    let f = Fixture::new("role");
    let r = f.runtime().await;
    messages(&r, vec![Msg::user("data")]);
    error(&r, Fault::Collision);
}

#[tokio::test]
async fn short_credentials_are_not_silently_ignored_in_retained_message_views() {
    let f = Fixture::new("q");
    let r = f.runtime().await;
    messages(&r, vec![Msg::user("qq data q")]);
    assert_eq!(
        value(&r)["messages"][0]["content"],
        "[REDACTED] data [REDACTED]"
    );
}

#[tokio::test]
async fn unicode_overlapping_credentials_have_complete_finite_field_views() {
    let f = Fixture::new("秘密🦀");
    let r = f.runtime().await;
    messages(&r, vec![Msg::user("before 秘密🦀秘密🦀 after\r\n")]);
    assert_eq!(
        value(&r)["messages"][0]["content"],
        "before [REDACTED] after\r\n"
    );
}

#[tokio::test]
async fn oversized_content_refuses_without_copying_or_truncating_retained_original_state() {
    let f = Fixture::new("retained-runtime-secret");
    let r = f.runtime().await;
    let text = "x".repeat(1024 * 1024 + 1);
    messages(&r, vec![Msg::user(text.clone())]);
    error(&r, Fault::Limit);
    assert!(
        matches!(&r.session.agent.state.lock().unwrap().messages[0],Msg::User { content } if content==&text)
    );
}

#[tokio::test]
async fn combined_original_and_view_work_limit_is_separate_from_encoded_byte_length() {
    let f = Fixture::new("retained-runtime-secret");
    let r = f.runtime().await;
    let text = "x".repeat(400_000);
    messages(&r, vec![Msg::user(text)]);
    error(&r, Fault::Limit);
}

#[tokio::test]
async fn escaped_encoded_record_bound_refuses_a_whole_message_not_a_repaired_prefix() {
    let f = Fixture::new("retained-runtime-secret");
    let r = f.runtime().await;
    messages(&r, vec![Msg::user("\u{0001}".repeat(200_000))]);
    error(&r, Fault::Limit);
}

#[tokio::test]
async fn private_message_count_cap_preserves_all_whole_records_or_refuses() {
    let f = Fixture::new("retained-runtime-secret");
    let r = f.runtime().await;
    messages(&r, vec![Msg::user("data"); 256]);
    let view = r.message_view().unwrap();
    assert_eq!(view.message_count().unwrap(), 256);
    messages(&r, vec![Msg::user("data"); 257]);
    error(&r, Fault::Limit);
}

#[tokio::test]
async fn tool_call_count_and_identity_scalar_limits_are_admitted_before_copying() {
    let f = Fixture::new("retained-runtime-secret");
    let r = f.runtime().await;
    messages(
        &r,
        vec![assistant("", vec![call("id", "example", json!({})); 257])],
    );
    error(&r, Fault::Limit);
    messages(
        &r,
        vec![assistant(
            "",
            vec![call(&"a".repeat(129), "example", json!({}))],
        )],
    );
    error(&r, Fault::Limit);
    messages(&r, vec![Msg::tool("a".repeat(129), "data")]);
    error(&r, Fault::Limit);
}

#[tokio::test]
async fn deep_argument_values_fail_before_recursive_view_allocation() {
    let f = Fixture::new("retained-runtime-secret");
    let r = f.runtime().await;
    let mut value = Value::Null;
    for _ in 0..65 {
        value = json!([value]);
    }
    messages(&r, vec![assistant("", vec![call("id", "example", value)])]);
    error(&r, Fault::Limit);
}

#[tokio::test]
async fn many_argument_nodes_fail_before_copying_a_large_original_value() {
    let f = Fixture::new("retained-runtime-secret");
    let r = f.runtime().await;
    messages(
        &r,
        vec![assistant(
            "",
            vec![call(
                "id",
                "example",
                Value::Array(vec![Value::Null; 100_001]),
            )],
        )],
    );
    error(&r, Fault::Limit);
}

#[tokio::test]
async fn non_message_state_files_todos_custom_and_context_are_not_implicitly_published() {
    let seed = "retained-runtime-secret";
    let f = Fixture::new(seed);
    let r = f.runtime().await;
    {
        let mut state = r.session.agent.state.lock().unwrap();
        state.messages = vec![Msg::user("only message")];
        state
            .files
            .insert("private-file".into(), FileData::text(seed));
        state.todos.push(Todo {
            content: seed.into(),
            status: TodoStatus::Pending,
        });
        state
            .custom
            .insert("private-custom".into(), json!({"data":seed}));
        state.system_extras.push(seed.into());
    }
    let json = value(&r);
    assert_eq!(json.as_object().unwrap().len(), 3);
    assert_eq!(json["messages"].as_array().unwrap().len(), 1);
    let state = r.retained_state();
    assert_eq!(state.files["private-file"].content, seed);
    assert_eq!(state.todos[0].content, seed);
    assert_eq!(state.system_extras, [seed]);
}

#[tokio::test]
async fn snapshot_scope_is_the_actual_main_retained_state_not_a_complete_compaction_ledger() {
    let f = Fixture::new("retained-runtime-secret");
    let r = f.runtime().await;
    messages(
        &r,
        vec![Msg::user("before compaction"), Msg::assistant("old reply")],
    );
    let first = r.message_view().unwrap();
    let first_bytes = first.bytes().unwrap().to_vec();
    messages(&r, vec![Msg::user("retained summary")]);
    let next = value(&r);
    assert_eq!(
        next["messages"],
        json!([{"role":"user","content":"retained summary"}])
    );
    assert_eq!(next["messages_omitted"], 0);
    assert_eq!(first.message_count().unwrap(), 2);
    assert_eq!(first.bytes().unwrap(), first_bytes);
}

#[tokio::test]
async fn checked_view_binds_to_actual_runtime_identity_not_equal_fresh_states_or_config() {
    let f = Fixture::new("retained-runtime-secret");
    let first = f.runtime().await;
    let other = f.runtime().await;
    messages(&first, vec![Msg::user("same")]);
    messages(&other, vec![Msg::user("same")]);
    let view = first.message_view().unwrap();
    assert!(view.bind(&first.message_scope()).is_ok());
    assert_eq!(view.bind(&other.message_scope()), Err(Fault::Collision));
}

#[tokio::test]
async fn frozen_snapshot_seeds_do_not_reload_replaced_configuration() {
    let seed = "retained-runtime-secret";
    let f = Fixture::new(seed);
    let r = f.runtime().await;
    f.configure("later-key", "http://127.0.0.1:2");
    messages(&r, vec![Msg::user(format!("original {seed} later-key"))]);
    assert_eq!(
        value(&r)["messages"][0]["content"],
        "original [REDACTED] later-key"
    );
    f.no_store();
}

#[tokio::test]
async fn late_typed_approval_latch_hides_existing_views_and_refuses_fresh_capture() {
    let f = Fixture::new("retained-runtime-secret");
    let r = f.runtime().await;
    messages(&r, vec![Msg::user("partial state")]);
    let view = r.message_view().unwrap();
    r.gate.observe_error_kind(ToolErrorKind::NeedsApproval);
    assert_eq!(view.bytes().err(), Some(Fault::Stopped));
    assert_eq!(view.message_count(), Err(Fault::Stopped));
    error(&r, Fault::Stopped);
    assert_eq!(r.retained_state().messages, [Msg::user("partial state")]);
    f.no_store();
}

#[tokio::test]
async fn message_failure_strings_and_success_metadata_never_mint_approval_authority() {
    let f = Fixture::new("retained-runtime-secret");
    let r = f.runtime().await;
    messages(
        &r,
        vec![Msg::Tool {
            tool_call_id: "id".into(),
            content: "NeedsApproval denied requires approval".into(),
            is_error: true,
        }],
    );
    assert!(r.message_view().is_ok());
    assert!(!r.gate.approval_required());
}

#[tokio::test]
async fn actual_builder_completed_turn_has_a_safe_view_without_a_raw_store_or_public_writer() {
    let seed = "retained-runtime-secret";
    let f = Fixture::new(seed);
    let server = MockServer::start().await;
    f.configure(seed, &server.uri());
    let event = json!({"choices":[{"delta":{"content":format!("ORIGINAL answer {seed}")}}]});
    Mock::given(method("POST")).and(path("/chat/completions")).respond_with(ResponseTemplate::new(200).set_body_raw(format!("data: {event}\n\ndata: {{\"choices\":[{{\"delta\":{{}},\"finish_reason\":\"stop\"}}]}}\n\ndata: [DONE]\n\n").into_bytes(),"text/event-stream")).mount(&server).await;
    let r = f.runtime().await;
    r.run_turn("ORIGINAL prompt".into()).await.unwrap();
    let snapshot = value(&r);
    assert!(
        snapshot["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["role"] == "assistant" && m["content"] == "ORIGINAL answer [REDACTED]")
    );
    assert!(
        r.retained_state()
            .messages
            .iter()
            .any(|m| matches!(m,Msg::Assistant{content,..} if content.contains(seed)))
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
    f.no_store();
}

#[tokio::test]
async fn admitted_input_run_selects_actual_checked_messages_not_its_raw_state_clone() {
    let seed = "retained-runtime-secret";
    let f = Fixture::new(seed);
    let server = MockServer::start().await;
    f.configure(seed, &server.uri());
    Mock::given(method("POST")).and(path("/chat/completions")).respond_with(ResponseTemplate::new(200).set_body_raw(b"data: {\"choices\":[{\"delta\":{\"content\":\"done\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n".to_vec(),"text/event-stream")).mount(&server).await;
    let prompt = format!("ORIGINAL input {seed}");
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
    let view = run.message_view().unwrap();
    let snapshot: Value = serde_json::from_slice(view.bytes().unwrap()).unwrap();
    assert!(
        snapshot["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["role"] == "user" && m["content"] == "ORIGINAL input [REDACTED]")
    );
    assert!(
        run.retained_state()
            .messages
            .iter()
            .any(|m| matches!(m,Msg::User{content} if content==&prompt))
    );
    f.no_store();
}
