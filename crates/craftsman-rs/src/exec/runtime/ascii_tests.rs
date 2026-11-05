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
    value["messages"] = json!(&source["messages"].as_array().unwrap()[omitted..]);
    value["messages_omitted"] = json!(omitted);
    serde_json::to_string(&value)
        .unwrap()
        .chars()
        .map(|c| {
            if c.is_ascii() && c != '\u{007f}' {
                1
            } else {
                6 * c.len_utf16()
            }
        })
        .sum()
}
fn value(t: &Terminal, max: usize) -> Value {
    let view = t.ascii(max).unwrap();
    let bytes = view.bytes().unwrap();
    assert!(bytes.len() <= max && bytes.iter().all(|b| (0x20..=0x7e).contains(b)));
    assert_eq!(bytes.first(), Some(&b'{'));
    assert_eq!(bytes.last(), Some(&b'}'));
    let json: Value = serde_json::from_slice(bytes).unwrap();
    assert_eq!(
        json["messages"].as_array().unwrap().len(),
        view.message_count().unwrap()
    );
    assert_eq!(
        json["messages_omitted"].as_u64().unwrap() as usize,
        view.messages_omitted().unwrap()
    );
    json
}
fn error(t: &Terminal, max: usize, fault: Fault) {
    assert_eq!(t.ascii(max).err(), Some(Error::Codec(fault)));
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
async fn already_printable_ascii_keeps_the_exact_old_selected_frame() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    let t = run.terminal();
    let old = t.selection(CAP).unwrap();
    let ascii = t.ascii(CAP).unwrap();
    assert_eq!(ascii.bytes().unwrap(), old.bytes().unwrap());
    assert_eq!(ascii.facts().unwrap(), old.facts().unwrap());
    assert_eq!(ascii.messages_omitted().unwrap(), 0);
    f.no_store();
}
#[tokio::test]
async fn bmp_unicode_is_ascii_escaped_without_changing_decoded_output() {
    let f = Fixture::new(SEED).await;
    f.response("é 中文 €").await;
    let run = f.run("input", &[]).await;
    let t = run.terminal();
    let v = t.ascii(CAP).unwrap();
    let text = std::str::from_utf8(v.bytes().unwrap()).unwrap();
    assert!(text.contains(r"\u00e9") && text.contains(r"\u4e2d") && text.contains(r"\u20ac"));
    assert_eq!(value(&t, CAP)["final_output"], "é 中文 €");
}
#[tokio::test]
async fn supplementary_scalars_use_valid_utf16_surrogate_pairs_not_lossy_replacement() {
    let f = Fixture::new(SEED).await;
    f.response("🦀💩𐐷").await;
    let run = f.run("input", &[]).await;
    let t = run.terminal();
    let a = t.ascii(CAP).unwrap();
    let text = std::str::from_utf8(a.bytes().unwrap()).unwrap();
    assert!(text.contains(r"\ud83e\udd80"));
    assert!(text.contains(r"\ud83d\udca9"));
    assert_eq!(value(&t, CAP)["final_output"], "🦀💩𐐷");
}
#[tokio::test]
async fn unicode_noncharacters_and_bom_inside_data_stay_actual_data() {
    let f = Fixture::new(SEED).await;
    let text = "\u{feff}\u{fffe}\u{10ffff}";
    f.response(text).await;
    let run = f.run("input", &[]).await;
    let t = run.terminal();
    assert_eq!(value(&t, CAP)["final_output"], text);
    assert_ne!(t.ascii(CAP).unwrap().bytes().unwrap().first(), Some(&0xef));
}
#[tokio::test]
async fn c0_c1_del_and_ansi_data_have_no_raw_control_or_nonascii_bytes() {
    let f = Fixture::new(SEED).await;
    let text = "\u{0000}\u{001b}[31m\u{007f}\u{0080}\u{009b}\u{009f}\r\n\t";
    f.response(text).await;
    let run = f.run("input", &[]).await;
    let t = run.terminal();
    let a = t.ascii(CAP).unwrap();
    let encoded = std::str::from_utf8(a.bytes().unwrap()).unwrap();
    assert!(a.bytes().unwrap().iter().all(|b| (0x20..=0x7e).contains(b)));
    assert!(encoded.contains(r"\u007f") && encoded.contains(r"\u009b"));
    assert_eq!(value(&t, CAP)["final_output"], text);
}
#[tokio::test]
async fn literal_backslash_u_sequences_are_not_decoded_or_repaired_as_surrogates() {
    let f = Fixture::new(SEED).await;
    let text = r"literal \ud800 \u4e2d \n";
    f.response(text).await;
    let run = f.run("input", &[]).await;
    let t = run.terminal();
    assert_eq!(value(&t, CAP)["final_output"], text);
    assert_eq!(
        t.ascii(CAP).unwrap().bytes().unwrap(),
        t.selection(CAP).unwrap().bytes().unwrap()
    );
}
#[tokio::test]
async fn quotes_backslashes_braces_and_nested_arg_shapes_keep_decoded_identity() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    let args = json!({"中文\"key":[{"data":"\\\"}]},[🦀\r\n","nested":[[true,null,8]]}]});
    state(&run).lock().unwrap().messages = vec![
        Msg::user("old".repeat(100)),
        assistant("[\"{🦀}", args.clone()),
    ];
    let t = run.terminal();
    let source = whole(&t);
    let v = value(&t, size(&source, 1));
    assert_eq!(v["messages"][0]["tool_calls"][0]["args"], args);
}
#[tokio::test]
async fn unicode_ids_names_and_member_keys_are_escaped_not_aliased() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    state(&run).lock().unwrap().messages = vec![
        Msg::Assistant {
            content: "".into(),
            tool_calls: vec![ToolCall {
                id: "调用🦀".into(),
                name: "名称".into(),
                args: json!({"字段":"值","number":7,"enabled":true}),
            }],
        },
        Msg::Tool {
            tool_call_id: "调用🦀".into(),
            content: "data".into(),
            is_error: true,
        },
    ];
    let t = run.terminal();
    let v = value(&t, CAP);
    assert_eq!(v["messages"][0]["tool_calls"][0]["id"], "调用🦀");
    assert_eq!(v["messages"][0]["tool_calls"][0]["name"], "名称");
    assert_eq!(v["messages"][0]["tool_calls"][0]["args"]["字段"], "值");
    assert_eq!(v["messages"][1]["tool_call_id"], "调用🦀");
    assert_eq!(v["messages"][1]["is_error"], true);
}
#[tokio::test]
async fn malformed_string_and_non_object_args_stay_typed_original_data() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    state(&run).lock().unwrap().messages = vec![assistant("", json!(format!("{{bad 中文 {SEED}")))];
    let t = run.terminal();
    assert_eq!(
        value(&t, CAP)["messages"][0]["tool_calls"][0]["args"],
        "{bad 中文 [REDACTED]"
    );
}
#[tokio::test]
async fn normalization_is_not_applied_to_canonically_distinct_unicode_data() {
    let f = Fixture::new(SEED).await;
    let text = "é e\u{0301} Å A\u{030a}";
    f.response(text).await;
    let run = f.run("input", &[]).await;
    assert_eq!(value(&run.terminal(), CAP)["final_output"], text);
}
#[tokio::test]
async fn post_escape_byte_budget_keeps_last_whole_message_at_exact_boundary() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    state(&run).lock().unwrap().messages =
        vec![Msg::user("old".repeat(100)), Msg::assistant("中文🦀")];
    let t = run.terminal();
    let source = whole(&t);
    let exact = size(&source, 1);
    let v = value(&t, exact);
    assert_eq!(v["messages"], json!([source["messages"][1].clone()]));
    assert_eq!(v["messages_omitted"], 1);
    assert_eq!(v["final_output"], source["final_output"]);
}
#[tokio::test]
async fn one_byte_below_ascii_message_size_omits_whole_record_not_scalar_suffix() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    state(&run).lock().unwrap().messages =
        vec![Msg::user("old".repeat(100)), Msg::assistant("中文🦀")];
    let t = run.terminal();
    let source = whole(&t);
    let v = value(&t, size(&source, 1) - 1);
    assert_eq!(v["messages"], json!([]));
    assert_eq!(v["messages_omitted"], 2);
    assert_eq!(v["final_output"], source["final_output"]);
}
#[tokio::test]
async fn utf8_fit_does_not_claim_fit_under_the_same_ascii_byte_budget() {
    let f = Fixture::new(SEED).await;
    f.response("ééééé").await;
    let run = f.run("input", &[]).await;
    state(&run).lock().unwrap().messages =
        vec![Msg::user("old".repeat(100)), Msg::assistant("🦀🦀")];
    let t = run.terminal();
    let source = whole(&t);
    let utf8_budget = {
        let mut v = source.clone();
        v["messages"] = json!([source["messages"][1].clone()]);
        v["messages_omitted"] = json!(1);
        serde_json::to_vec(&v).unwrap().len()
    };
    assert_eq!(
        t.selection(utf8_budget).unwrap().message_count().unwrap(),
        1
    );
    let v = value(&t, utf8_budget);
    assert_eq!(v["messages"], json!([]));
    assert_eq!(v["final_output"], "ééééé");
}
#[tokio::test]
async fn exact_minimum_ascii_header_budget_preserves_full_output_and_facts() {
    let f = Fixture::new(SEED).await;
    f.response("Unicode中文🦀").await;
    let run = f.run("input", &[]).await;
    let t = run.terminal();
    let source = whole(&t);
    let n = source["messages"].as_array().unwrap().len();
    let v = value(&t, size(&source, n));
    assert_eq!(v["messages"], json!([]));
    assert_eq!(v["messages_omitted"], n);
    assert_eq!(v["final_output"], "Unicode中文🦀");
    error(&t, size(&source, n) - 1, Fault::Limit);
}
#[tokio::test]
async fn ascii_header_expansion_that_cannot_fit_is_refusal_never_answer_truncation() {
    let f = Fixture::new(SEED).await;
    let text = format!(
        "{}{}",
        "\u{0001}".repeat(100_000),
        "\u{007f}".repeat(200_000)
    );
    f.response(&text).await;
    let run = f.run("input", &[]).await;
    assert_eq!(run.outcome().as_ref().unwrap().final_text(), text);
    state(&run).lock().unwrap().messages = vec![];
    let t = run.terminal();
    assert!(t.selection(CAP).is_ok());
    error(&t, CAP, Fault::Limit);
    assert!(t.selection(CAP).is_ok());
}
#[tokio::test]
async fn full_ascii_projection_can_exceed_final_budget_before_whole_message_selection() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    let shared = state(&run);
    shared.lock().unwrap().messages = vec![
        Msg::user("\u{007f}".repeat(150_000)),
        Msg::assistant("é".repeat(80_000)),
    ];
    let t = run.terminal();
    assert_eq!(t.selection(CAP).unwrap().message_count().unwrap(), 2);
    let v = value(&t, CAP);
    assert_eq!(v["messages"].as_array().unwrap().len(), 1);
    assert_eq!(v["messages_omitted"], 1);
    assert_eq!(v["messages"][0]["content"], "é".repeat(80_000));
    assert_eq!(v["final_output"], "original answer [REDACTED]");
    assert_eq!(shared.lock().unwrap().messages.len(), 2);
}
#[tokio::test]
async fn oversized_newest_ascii_message_is_omitted_not_replaced_with_older_small_records() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    state(&run).lock().unwrap().messages = vec![
        Msg::user("old small"),
        Msg::assistant("\u{0001}".repeat(100_000) + &"\u{007f}".repeat(200_000)),
    ];
    let t = run.terminal();
    let v = value(&t, CAP);
    assert_eq!(v["messages"], json!([]));
    assert_eq!(v["messages_omitted"], 2);
    assert_eq!(v["final_output"], "original answer [REDACTED]");
}
#[tokio::test]
async fn escaped_omission_digit_growth_is_included_in_complete_ascii_size() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    state(&run).lock().unwrap().messages = vec![Msg::user("中文".repeat(10)); 12];
    let t = run.terminal();
    let source = whole(&t);
    let v = value(&t, size(&source, 10));
    assert_eq!(v["messages"].as_array().unwrap().len(), 2);
    assert_eq!(v["messages_omitted"], 10);
}
#[tokio::test]
async fn caller_budgets_only_tighten_final_ascii_cap() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    let t = run.terminal();
    for n in [0, CAP + 1, usize::MAX] {
        error(&t, n, Fault::Limit);
    }
    assert!(t.ascii(CAP).is_ok());
}
#[tokio::test]
async fn bmp_escape_created_credential_refuses_complete_ascii_channel() {
    let f = Fixture::new(r"\u4e2d").await;
    f.response("中").await;
    let run = f.run("input", &[]).await;
    let t = run.terminal();
    assert!(t.selection(CAP).is_ok());
    error(&t, CAP, Fault::Collision);
}
#[tokio::test]
async fn surrogate_pair_created_credential_refuses_complete_ascii_channel() {
    let f = Fixture::new(r"\ud83e\udd80").await;
    f.response("🦀").await;
    let run = f.run("input", &[]).await;
    let t = run.terminal();
    assert!(t.selection(CAP).is_ok());
    error(&t, CAP, Fault::Collision);
}
#[tokio::test]
async fn del_escape_created_credential_is_refused_not_rewritten() {
    let f = Fixture::new(r"\u007f").await;
    f.response("\u{007f}").await;
    let run = f.run("input", &[]).await;
    let t = run.terminal();
    assert!(t.selection(CAP).is_ok());
    error(&t, CAP, Fault::Collision);
}
#[tokio::test]
async fn created_credential_in_an_omitted_old_message_is_not_skipped_by_ascii_selection() {
    let f = Fixture::new(r"\u4e2d").await;
    f.response("done").await;
    let run = f.run("input", &[]).await;
    state(&run).lock().unwrap().messages = vec![Msg::user("中"), Msg::assistant("safe last")];
    let t = run.terminal();
    let source = whole(&t);
    error(&t, size(&source, 1), Fault::Collision);
}
#[tokio::test]
async fn ascii_member_key_escape_collision_never_renames_argument_keys() {
    let f = Fixture::new(r"\u53c2").await;
    f.response("done").await;
    let run = f.run("input", &[]).await;
    state(&run).lock().unwrap().messages = vec![assistant("", json!({"参":"data"}))];
    let t = run.terminal();
    assert!(t.selection(CAP).is_ok());
    error(&t, CAP, Fault::Collision);
}
#[tokio::test]
async fn generated_escape_match_spanning_verifier_chunks_is_not_missed() {
    let f = Fixture::new(r"\u4e2d").await;
    let lead = r#"{"run_return":"returned","approval_required":false,"final_output":""#;
    let text = "a".repeat(65_536 - lead.len() - 3) + "中";
    f.response(&text).await;
    let run = f.run("input", &[]).await;
    let t = run.terminal();
    assert!(t.selection(CAP).is_ok());
    error(&t, CAP, Fault::Collision);
}
#[tokio::test]
async fn final_ascii_omission_integer_collision_has_no_alternate_layout_fallback() {
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
async fn final_ascii_header_to_message_join_collision_preserves_facts_or_refuses() {
    let seed = r#""final_output":"\u4e2d","message_scope":"retained_state","messages":[{"role":"assistant"#;
    let f = Fixture::new(seed).await;
    f.response("中").await;
    let run = f.run("input", &[]).await;
    state(&run).lock().unwrap().messages =
        vec![Msg::user("old".repeat(100)), Msg::assistant("last")];
    let t = run.terminal();
    let source = whole(&t);
    error(&t, size(&source, 1), Fault::Collision);
}
#[tokio::test]
async fn complete_source_utf8_encoded_budget_is_not_bypassed_by_ascii_omission() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    state(&run).lock().unwrap().messages = vec![Msg::user("\u{0001}".repeat(200_000))];
    error(&run.terminal(), CAP, Fault::Limit);
}
#[tokio::test]
async fn original_final_answer_source_and_work_caps_are_not_weakened() {
    let f = Fixture::new(SEED).await;
    f.response(&"x".repeat(CAP + 1)).await;
    let run = f.run("input", &[]).await;
    state(&run).lock().unwrap().messages = vec![];
    error(&run.terminal(), CAP, Fault::Limit);
    let f = Fixture::new(SEED).await;
    f.response(&"x".repeat(400_000)).await;
    let run = f.run("input", &[]).await;
    state(&run).lock().unwrap().messages = vec![];
    error(&run.terminal(), CAP, Fault::Limit);
}
#[tokio::test]
async fn original_message_count_and_raw_size_caps_are_not_ascii_exemptions() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    let shared = state(&run);
    let t = run.terminal();
    shared.lock().unwrap().messages = vec![Msg::user("data"); 257];
    error(&t, CAP, Fault::Limit);
    shared.lock().unwrap().messages = vec![Msg::user("x".repeat(CAP + 1))];
    error(&t, CAP, Fault::Limit);
}
#[tokio::test]
async fn original_ids_keys_and_numeric_authority_collisions_still_refuse_before_projection() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    let shared = state(&run);
    let t = run.terminal();
    shared.lock().unwrap().messages = vec![Msg::tool(SEED, "old")];
    error(&t, CAP, Fault::Collision);
    shared.lock().unwrap().messages = vec![assistant("", json!({SEED:"old"}))];
    error(&t, CAP, Fault::Collision);
    let f = Fixture::new("42").await;
    f.response("done").await;
    let run = f.run("input", &[]).await;
    state(&run).lock().unwrap().messages = vec![assistant("", json!({"number":42}))];
    error(&run.terminal(), CAP, Fault::Collision);
}
#[tokio::test]
async fn original_call_depth_and_node_bounds_hold_before_ascii_projection() {
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
async fn deepest_admitted_args_and_unescaped_unicode_inside_them_stay_roundtrip_valid() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    let mut args = json!("中文🦀");
    for _ in 0..64 {
        args = json!([args]);
    }
    state(&run).lock().unwrap().messages = vec![assistant("", args.clone())];
    let t = run.terminal();
    assert_eq!(value(&t, CAP)["messages"][0]["tool_calls"][0]["args"], args);
}
#[tokio::test]
async fn all_256_admitted_messages_can_be_omitted_with_truthful_ascii_counts() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    state(&run).lock().unwrap().messages = vec![Msg::user("中"); 256];
    let t = run.terminal();
    let source = whole(&t);
    let v = value(&t, size(&source, 256));
    assert_eq!(v["messages"], json!([]));
    assert_eq!(v["messages_omitted"], 256);
    assert_eq!(v["final_output"], source["final_output"]);
}
#[tokio::test]
async fn actual_approval_partial_ascii_output_does_not_reopen_execution_or_change_original_return()
{
    let f = Fixture::new(SEED).await;
    f.approval_response().await;
    let run = f.run("原始输入", &["write_file"]).await;
    assert!(matches!(run.outcome(), Err(RuntimeError::ApprovalRequired)));
    let g = gate(&run);
    let t = run.terminal();
    let source = whole(&t);
    let n = source["messages"].as_array().unwrap().len();
    let v = value(&t, size(&source, n));
    assert_eq!(v["run_return"], "approval_required");
    assert_eq!(v["approval_required"], true);
    assert!(v["final_output"].is_null());
    assert_eq!(v["messages_omitted"], n);
    assert!(g.execution_closed() && g.approval_required());
    assert!(!f.workspace.join("later.txt").exists());
    assert_eq!(f.server.received_requests().await.unwrap().len(), 1);
    f.no_store();
}
#[tokio::test]
async fn absent_failure_output_and_genuine_empty_returned_output_remain_distinct() {
    let f = Fixture::new(SEED).await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(400).set_body_raw(format!("{SEED} raw failure"), "text/plain"),
        )
        .mount(&f.server)
        .await;
    let run = f.run("input", &[]).await;
    let t = run.terminal();
    assert!(value(&t, CAP)["final_output"].is_null());
    let f = Fixture::new(SEED).await;
    f.response("").await;
    let run = f.run("input", &[]).await;
    assert!(run.outcome().is_ok());
    assert_eq!(value(&run.terminal(), CAP)["final_output"], "");
}
#[tokio::test]
async fn late_actual_approval_supersedes_every_old_ascii_read_and_fresh_keeps_real_return_fact() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    let g = gate(&run);
    let t = run.terminal();
    let a = t.ascii(CAP).unwrap();
    g.observe_error_kind(ToolErrorKind::NeedsApproval);
    assert_eq!(a.bytes().err(), Some(Error::Superseded));
    assert_eq!(a.facts().err(), Some(Error::Superseded));
    assert_eq!(a.message_count(), Err(Error::Superseded));
    assert_eq!(a.messages_omitted(), Err(Error::Superseded));
    assert_eq!(a.bind(&t.scope().unwrap()), Err(Error::Superseded));
    let fresh = t.ascii(CAP).unwrap();
    assert_eq!(fresh.facts().unwrap().returned(), ReturnKind::Returned);
    assert!(fresh.facts().unwrap().approval_required());
    assert!(g.execution_closed());
}
#[tokio::test]
async fn publication_revocation_hides_all_ascii_reads_and_refuses_new_capture() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    let t = run.terminal();
    let a = t.ascii(CAP).unwrap();
    t.close();
    assert_eq!(a.bytes().err(), Some(Error::Closed));
    assert_eq!(a.facts().err(), Some(Error::Closed));
    assert_eq!(a.message_count(), Err(Error::Closed));
    assert_eq!(a.messages_omitted(), Err(Error::Closed));
    assert_eq!(t.ascii(CAP).err(), Some(Error::Closed));
}
#[tokio::test]
async fn equal_fresh_terminal_owners_cannot_bind_ascii_fragments_to_each_other() {
    let f = Fixture::new(SEED).await;
    f.response("same中").await;
    let one = f.run("same", &[]).await.terminal();
    let two = f.run("same", &[]).await.terminal();
    let a = one.ascii(CAP).unwrap();
    assert!(a.bind(&one.scope().unwrap()).is_ok());
    assert_eq!(a.bind(&two.scope().unwrap()), Err(Error::Foreign));
}
#[tokio::test]
async fn after_work_actual_epoch_change_overrides_ascii_success_collision_and_limit() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    let g = gate(&run);
    let t = run.terminal();
    assert_eq!(
        t.ascii_after_work_for_tests(CAP, || g.observe_error_kind(ToolErrorKind::NeedsApproval))
            .err(),
        Some(Error::Superseded)
    );
    let f = Fixture::new(r"\u4e2d").await;
    f.response("中").await;
    let run = f.run("input", &[]).await;
    let g = gate(&run);
    let t = run.terminal();
    assert_eq!(
        t.ascii_after_work_for_tests(CAP, || g.observe_error_kind(ToolErrorKind::NeedsApproval))
            .err(),
        Some(Error::Superseded)
    );
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    let g = gate(&run);
    let t = run.terminal();
    assert_eq!(
        t.ascii_after_work_for_tests(0, || g.observe_error_kind(ToolErrorKind::NeedsApproval))
            .err(),
        Some(Error::Superseded)
    );
}
#[tokio::test]
async fn after_work_actual_revocation_overrides_ascii_success_and_budget_failure() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    let t = run.terminal();
    assert_eq!(
        t.ascii_after_work_for_tests(CAP, || t.close()).err(),
        Some(Error::Closed)
    );
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    let t = run.terminal();
    assert_eq!(
        t.ascii_after_work_for_tests(0, || t.close()).err(),
        Some(Error::Closed)
    );
}
#[tokio::test]
async fn immutable_ascii_bytes_are_not_a_live_current_or_complete_history_ledger() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    let shared = state(&run);
    let t = run.terminal();
    let a = t.ascii(CAP).unwrap();
    let bytes = a.bytes().unwrap().to_vec();
    let n = a.message_count().unwrap();
    shared.lock().unwrap().messages = vec![Msg::user("new summary中")];
    assert_eq!(a.bytes().unwrap(), bytes);
    assert_eq!(a.message_count().unwrap(), n);
    assert_eq!(value(&t, CAP)["messages"].as_array().unwrap().len(), 1);
}
#[tokio::test]
async fn runner_command_text_stays_inside_one_ascii_json_record_not_a_human_channel_claim() {
    let f = Fixture::new(SEED).await;
    let text = "::warning::data\n##vso[task.setvariable variable=x]value\r\n中\u{2028}\u{2029}";
    f.response(text).await;
    let run = f.run("input", &[]).await;
    let t = run.terminal();
    let a = t.ascii(CAP).unwrap();
    let bytes = a.bytes().unwrap();
    assert_eq!(bytes.first(), Some(&b'{'));
    assert!(!bytes.contains(&b'\n') && !bytes.contains(&b'\r'));
    assert_eq!(value(&t, CAP)["final_output"], text);
}
#[tokio::test]
async fn original_final_answer_and_frozen_credentials_do_not_follow_last_state_or_reloaded_config()
{
    let f = Fixture::new(SEED).await;
    f.response(&format!("{SEED} later-key 中文")).await;
    let run = f.run("input", &[]).await;
    state(&run).lock().unwrap().messages = vec![Msg::assistant("different last")];
    f.configure("later-key");
    let t = run.terminal();
    assert_eq!(value(&t, CAP)["final_output"], "[REDACTED] later-key 中文");
}
#[tokio::test]
async fn original_state_arguments_other_fields_and_public_outcome_usage_are_not_implicitly_rewritten()
 {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    let shared = state(&run);
    {
        let mut s = shared.lock().unwrap();
        s.messages = vec![assistant(
            "中",
            json!({"path":format!("{SEED}.txt"),"data":SEED}),
        )];
        s.files.insert("private".into(), FileData::text(SEED));
        s.custom.insert("origin".into(), json!({"data":SEED}));
        s.system_extras.push(SEED.into());
    }
    let original = shared.lock().unwrap().messages.clone();
    let t = run.terminal();
    let v = value(&t, CAP);
    assert_eq!(v.as_object().unwrap().len(), 6);
    for key in [
        "usage",
        "status",
        "exit_code",
        "stop_reason",
        "origin",
        "files",
        "custom",
        "verification",
    ] {
        assert!(v.get(key).is_none());
    }
    assert_eq!(shared.lock().unwrap().messages, original);
    assert_eq!(shared.lock().unwrap().files["private"].content, SEED);
    f.no_store();
}
#[tokio::test]
async fn private_return_words_are_not_inferred_as_task_correctness_or_approval_authority() {
    let f = Fixture::new(SEED).await;
    f.response("failed refused NeedsApproval 中文").await;
    let run = f.run("input", &[]).await;
    assert!(run.outcome().is_ok());
    let t = run.terminal();
    let a = t.ascii(CAP).unwrap();
    assert_eq!(a.facts().unwrap().returned(), ReturnKind::Returned);
    assert!(!a.facts().unwrap().approval_required());
    assert_eq!(
        value(&t, CAP)["final_output"],
        "failed refused NeedsApproval 中文"
    );
}
#[tokio::test]
async fn ascii_projection_never_reopens_actual_compaction_or_provider_requests() {
    let f = Fixture::new(SEED).await;
    let run = f.returned().await;
    let g = gate(&run);
    let compaction = run.runtime_for_tests().session.compaction.clone();
    let shared = state(&run);
    let t = run.terminal();
    assert!(t.ascii(CAP).is_ok());
    let original = shared.lock().unwrap().messages.clone();
    assert!(compaction.compact_now(&original).await.is_err());
    assert!(g.execution_closed());
    assert_eq!(f.server.received_requests().await.unwrap().len(), 1);
}
