//! Actual shared-builder main/child/compaction calls, not fabricated counters.
use super::*;
use crate::exec::construction::{Options, SessionSelection};
use crate::exec::input::{Invocation, Sources};
use crate::exec::meter::{RequestLimit, Role};
use crate::exec::permissions::{OnUnapproved, Selectors};
use craftsman_harness::{Msg, ToolCall, ToolErrorKind};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Mutex;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

struct Fixture {
    _root: tempfile::TempDir,
    workspace: PathBuf,
    config: PathBuf,
    server: MockServer,
}
impl Fixture {
    async fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("workspace");
        let config = root.path().join("selected-config");
        std::fs::create_dir(&workspace).unwrap();
        std::fs::create_dir(&config).unwrap();
        let server = MockServer::start().await;
        std::fs::write(
            config.join("config.json"),
            json!({
                "model":"synthetic-request-meter-model", "api_key":"synthetic-request-meter-key",
                "base_url":server.uri(),
            })
            .to_string(),
        )
        .unwrap();
        Self {
            _root: root,
            workspace,
            config,
            server,
        }
    }
    fn options(&self, selectors: Selectors) -> Options {
        Options {
            workspace: self.workspace.clone(),
            config_dir: self.config.clone(),
            trust_workspace: false,
            session: SessionSelection::Ephemeral,
            selectors,
        }
    }
    async fn runtime(&self, maximum: u64, selectors: Selectors) -> Runtime {
        let limit = RequestLimit::new(maximum).unwrap();
        Runtime::build_with_request_limit(
            Prepared::prepare(self.options(selectors)).await.unwrap(),
            limit,
        )
        .await
        .unwrap()
    }
    async fn script(&self, messages: Vec<Msg>) {
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(Script(Mutex::new(messages.into())))
            .mount(&self.server)
            .await;
    }
    async fn requests(&self) -> Vec<Value> {
        self.server
            .received_requests()
            .await
            .unwrap()
            .into_iter()
            .filter(|r| r.url.path() == "/chat/completions")
            .map(|r| serde_json::from_slice(&r.body).unwrap())
            .collect()
    }
    fn named_child(&self, name: &str) {
        let root = self.config.join("plugins");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("request-child.json"), json!({
            "version":1,"name":"request-child-extension","agents":[{
                "name":name,"description":"Synthetic foreground child","system_prompt":"NAMED-CHILD-ORIGINAL-CONTEXT","tools":[]
            }]
        }).to_string()).unwrap();
    }
}
fn source(prompt: &str) -> Sources<'_> {
    Sources {
        prompt: Some(prompt.as_bytes()),
        prompt_file: None,
        files: &[],
    }
}
fn call(id: &str, name: &str, args: Value) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: name.into(),
        args,
    }
}
fn tools(calls: Vec<ToolCall>) -> Msg {
    Msg::Assistant {
        content: "original partial answer".into(),
        tool_calls: calls,
    }
}
fn read(id: &str) -> ToolCall {
    call(id, "read_file", json!({"file_path":"context.txt"}))
}
fn context() -> Vec<Msg> {
    (0..128)
        .map(|n| {
            if n % 2 == 0 {
                Msg::user("synthetic original context")
            } else {
                Msg::assistant("synthetic original reply")
            }
        })
        .collect()
}
struct Script(Mutex<VecDeque<Msg>>);
impl Respond for Script {
    fn respond(&self, _: &Request) -> ResponseTemplate {
        let Some(Msg::Assistant {
            content,
            tool_calls,
        }) = self.0.lock().unwrap().pop_front()
        else {
            return ResponseTemplate::new(400).set_body_string("unexpected synthetic request");
        };
        let calls: Vec<Value> = tool_calls.iter().enumerate().map(|(index,c)| json!({
            "index":index,"id":c.id,"type":"function","function":{"name":c.name,"arguments":c.args.to_string()}
        })).collect();
        let stop = if calls.is_empty() {
            "stop"
        } else {
            "tool_calls"
        };
        let body = format!(
            "data: {}\n\ndata: {}\n\ndata: {{\"choices\":[],\"usage\":{{\"prompt_tokens\":20,\"completion_tokens\":5}}}}\n\ndata: [DONE]\n\n",
            json!({"choices":[{"delta":{"content":content,"tool_calls":calls}}]}),
            json!({"choices":[{"delta":{},"finish_reason":stop}]})
        );
        ResponseTemplate::new(200)
            .insert_header("content-type", "text/event-stream")
            .set_body_string(body)
    }
}

#[tokio::test]
async fn actual_default_input_run_selects_private_counts_without_public_usage_claim() {
    let f = Fixture::new().await;
    std::fs::write(f.workspace.join("context.txt"), "original file").unwrap();
    f.script(vec![
        tools(vec![read("read")]),
        Msg::assistant("actual final"),
    ])
    .await;
    let run = Invocation::prepare(f.options(Selectors::default()), source("input"))
        .await
        .unwrap()
        .run()
        .await;
    assert_eq!(run.outcome().as_ref().unwrap().final_text(), "actual final");
    let counts = run.request_counts().unwrap();
    assert_eq!(counts.maximum(), 50);
    assert_eq!(counts.admitted(), 2);
    assert_eq!(counts.role(Role::Main), [2, 0, 2, 0, 0]);
    assert_eq!(counts.role(Role::Subagent), [0; 5]);
    assert!(counts.refusal().is_none());
    assert!(counts.same_source(&run.request_counts().unwrap()));
    assert_eq!(f.requests().await.len(), 2);
    assert!(!f.config.join("threads.db").exists());
}
#[tokio::test]
async fn main_request_after_partial_response_refuses_without_new_traffic() {
    let f = Fixture::new().await;
    std::fs::write(f.workspace.join("context.txt"), "original file").unwrap();
    f.script(vec![
        tools(vec![read("read")]),
        Msg::assistant("must not request"),
    ])
    .await;
    let runtime = f.runtime(1, Selectors::default()).await;
    assert!(matches!(
        runtime.run_turn("input".into()).await,
        Err(RuntimeError::RequestLimit)
    ));
    let s = runtime.request_snapshot().unwrap();
    assert_eq!(s.admitted(), 1);
    assert_eq!(s.role(Role::Main), [1, 0, 1, 0, 0]);
    assert_eq!(s.refusal(), Some((Role::Main, 1, 1)));
    assert_eq!(f.requests().await.len(), 1);
    assert!(runtime.gate.execution_closed());
    assert!(!runtime.gate.approval_required());
    assert!(
        runtime.retained_state().messages.iter().any(
            |m| matches!(m,Msg::Assistant { content,.. } if content=="original partial answer")
        )
    );
}
#[tokio::test]
async fn exact_final_real_response_can_complete_at_the_cap() {
    let f = Fixture::new().await;
    f.script(vec![Msg::assistant("exact final")]).await;
    let runtime = f.runtime(1, Selectors::default()).await;
    assert_eq!(
        runtime.run_turn("input".into()).await.unwrap().final_text(),
        "exact final"
    );
    assert!(runtime.request_snapshot().unwrap().refusal().is_none());
    assert!(!runtime.gate.execution_closed());
    assert_eq!(f.requests().await.len(), 1);
}
#[tokio::test]
async fn real_foreground_child_consumes_the_same_admission_cap() {
    let f = Fixture::new().await;
    f.script(vec![
        tools(vec![call(
            "child",
            "task",
            json!({"subagent_type":"general-purpose","description":"inspect"}),
        )]),
        Msg::assistant("actual child"),
        Msg::assistant("must not request"),
    ])
    .await;
    let runtime = f.runtime(2, Selectors::default()).await;
    assert!(matches!(
        runtime.run_turn("input".into()).await,
        Err(RuntimeError::RequestLimit)
    ));
    let s = runtime.request_snapshot().unwrap();
    assert_eq!(s.admitted(), 2);
    assert_eq!(s.role(Role::Main), [1, 0, 1, 0, 0]);
    assert_eq!(s.role(Role::Subagent), [1, 0, 1, 0, 0]);
    assert_eq!(s.refusal(), Some((Role::Main, 2, 2)));
    assert_eq!(f.requests().await.len(), 2);
    assert_eq!(runtime.session.registry.list().len(), 1);
}
#[tokio::test]
async fn named_child_role_is_not_inferred_from_a_model_or_name_alias() {
    let f = Fixture::new().await;
    f.named_child("not-the-model-name");
    f.script(vec![
        tools(vec![call(
            "child",
            "task",
            json!({"subagent_type":"not-the-model-name","description":"inspect"}),
        )]),
        Msg::assistant("named child"),
        Msg::assistant("must not request"),
    ])
    .await;
    let runtime = f.runtime(2, Selectors::default()).await;
    assert!(matches!(
        runtime.run_turn("input".into()).await,
        Err(RuntimeError::RequestLimit)
    ));
    let s = runtime.request_snapshot().unwrap();
    assert_eq!(s.role(Role::Subagent), [1, 0, 1, 0, 0]);
    assert_eq!(s.role(Role::Main), [1, 0, 1, 0, 0]);
    let requests = f.requests().await;
    assert_eq!(requests.len(), 2);
    assert!(
        requests
            .iter()
            .all(|r| r["model"] == "synthetic-request-meter-model")
    );
    assert!(
        requests[1]["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains("NAMED-CHILD-ORIGINAL-CONTEXT")
    );
}
#[tokio::test]
async fn real_compaction_before_main_uses_the_same_budget_and_source() {
    let f = Fixture::new().await;
    f.script(vec![
        Msg::assistant("actual compacted context"),
        Msg::assistant("must not request"),
    ])
    .await;
    let runtime = f.runtime(1, Selectors::default()).await;
    let original = context();
    assert!(
        runtime
            .session
            .compaction
            .compact_now(&original)
            .await
            .unwrap()
            .is_some()
    );
    assert!(matches!(
        runtime.run_turn("input".into()).await,
        Err(RuntimeError::RequestLimit)
    ));
    let s = runtime.request_snapshot().unwrap();
    assert_eq!(s.role(Role::Compaction), [1, 0, 1, 0, 0]);
    assert_eq!(s.role(Role::Main), [0; 5]);
    assert_eq!(s.refusal(), Some((Role::Main, 1, 1)));
    assert_eq!(f.requests().await.len(), 1);
}
#[tokio::test]
async fn automatic_compaction_and_final_main_share_exact_two_call_cap() {
    let f = Fixture::new().await;
    f.script(vec![
        Msg::assistant("actual automatic summary"),
        Msg::assistant("actual final"),
    ])
    .await;
    let runtime = f.runtime(2, Selectors::default()).await;
    runtime
        .session
        .agent
        .state
        .lock()
        .unwrap()
        .messages
        .extend(context());
    assert_eq!(
        runtime.run_turn("input".into()).await.unwrap().final_text(),
        "actual final"
    );
    let s = runtime.request_snapshot().unwrap();
    assert_eq!(s.admitted(), 2);
    assert_eq!(s.role(Role::Compaction), [1, 0, 1, 0, 0]);
    assert_eq!(s.role(Role::Main), [1, 0, 1, 0, 0]);
    assert!(s.refusal().is_none());
    assert_eq!(f.requests().await.len(), 2);
    assert_eq!(
        *runtime.session.usage.lock().unwrap(),
        (20, 5),
        "legacy main usage is not rewritten into a guessed all-role ledger"
    );
}
#[tokio::test]
async fn caught_automatic_compaction_refusal_cannot_turn_into_completion() {
    let f = Fixture::new().await;
    f.script(vec![
        Msg::assistant("first real response"),
        Msg::assistant("must not request"),
    ])
    .await;
    let runtime = f.runtime(1, Selectors::default()).await;
    assert!(runtime.run_turn("first".into()).await.is_ok());
    runtime
        .session
        .agent
        .state
        .lock()
        .unwrap()
        .messages
        .extend(context());
    assert!(matches!(
        runtime.run_turn("second".into()).await,
        Err(RuntimeError::RequestLimit)
    ));
    let s = runtime.request_snapshot().unwrap();
    assert_eq!(s.admitted(), 1);
    assert_eq!(s.role(Role::Compaction), [0; 5]);
    assert_eq!(s.refusal(), Some((Role::Compaction, 1, 1)));
    assert_eq!(
        f.requests().await.len(),
        1,
        "retain the original first request; never reset history to fabricate zero"
    );
}
#[tokio::test]
async fn child_budget_stop_closes_same_response_allowed_tool_dispatch() {
    let f = Fixture::new().await;
    f.script(vec![
        tools(vec![
            call(
                "child",
                "task",
                json!({"subagent_type":"general-purpose","description":"inspect"}),
            ),
            call(
                "later",
                "write_file",
                json!({"file_path":"must-not-write.txt","content":"no"}),
            ),
        ]),
        Msg::assistant("must not request"),
    ])
    .await;
    let selectors = Selectors {
        allow: vec!["tool:write_file".into()],
        on_unapproved: OnUnapproved::Fail,
        ..Default::default()
    };
    let runtime = f.runtime(1, selectors).await;
    assert!(matches!(
        runtime.run_turn("input".into()).await,
        Err(RuntimeError::RequestLimit)
    ));
    assert!(!f.workspace.join("must-not-write.txt").exists());
    assert!(!runtime.gate.approval_required());
    assert!(runtime.gate.execution_closed());
    assert_eq!(f.requests().await.len(), 1);
    let state = runtime.retained_state();
    assert!(state.messages.iter().any(
        |m| matches!(m,Msg::Tool {tool_call_id,is_error,..} if tool_call_id=="later" && *is_error)
    ));
    assert_eq!(
        runtime.request_snapshot().unwrap().refusal(),
        Some((Role::Subagent, 1, 1))
    );
}
#[tokio::test]
async fn true_approval_required_precedes_cap_and_is_never_a_fake_limit() {
    let f = Fixture::new().await;
    f.script(vec![tools(vec![call(
        "unapproved",
        "write_file",
        json!({"file_path":"must-not-write.txt","content":"no"}),
    )])])
    .await;
    let runtime = f
        .runtime(
            1,
            Selectors {
                on_unapproved: OnUnapproved::Fail,
                ..Default::default()
            },
        )
        .await;
    assert!(matches!(
        runtime.run_turn("input".into()).await,
        Err(RuntimeError::ApprovalRequired)
    ));
    assert!(runtime.gate.approval_required());
    assert!(runtime.request_snapshot().unwrap().refusal().is_none());
    assert_eq!(f.requests().await.len(), 1);
    assert!(!f.workspace.join("must-not-write.txt").exists());
}
#[tokio::test]
async fn late_true_approval_supersedes_an_observed_real_budget_stop() {
    let f = Fixture::new().await;
    std::fs::write(f.workspace.join("context.txt"), "original file").unwrap();
    f.script(vec![tools(vec![read("read")])]).await;
    let runtime = f
        .runtime(
            1,
            Selectors {
                on_unapproved: OnUnapproved::Fail,
                ..Default::default()
            },
        )
        .await;
    assert!(matches!(
        runtime.run_turn("input".into()).await,
        Err(RuntimeError::RequestLimit)
    ));
    runtime
        .gate
        .observe_error_kind(ToolErrorKind::NeedsApproval);
    assert!(matches!(
        runtime.run_turn("again".into()).await,
        Err(RuntimeError::ApprovalRequired)
    ));
    assert_eq!(
        runtime.request_snapshot().unwrap().refusal(),
        Some((Role::Main, 1, 1))
    );
    assert_eq!(f.requests().await.len(), 1);
}
#[tokio::test]
async fn poisoned_runtime_refuses_before_any_new_provider_or_zero_snapshot() {
    let f = Fixture::new().await;
    f.script(vec![
        Msg::assistant("first actual response"),
        Msg::assistant("must not request"),
    ])
    .await;
    let runtime = f.runtime(2, Selectors::default()).await;
    assert!(runtime.run_turn("first".into()).await.is_ok());
    let old = runtime.request_snapshot().unwrap();
    runtime.request_meter.poison_for_tests();
    assert!(matches!(
        runtime.run_turn("second".into()).await,
        Err(RuntimeError::RequestAccounting)
    ));
    assert!(matches!(
        runtime.request_snapshot(),
        Err(RuntimeError::RequestAccounting)
    ));
    assert_eq!(old.admitted(), 1);
    assert_eq!(f.requests().await.len(), 1);
    assert!(runtime.gate.execution_closed());
}
#[tokio::test]
async fn distinct_actual_input_runs_do_not_alias_equal_counter_views() {
    let f = Fixture::new().await;
    f.script(vec![Msg::assistant("first"), Msg::assistant("second")])
        .await;
    let a = Invocation::prepare(f.options(Selectors::default()), source("first"))
        .await
        .unwrap()
        .run()
        .await;
    let b = Invocation::prepare(f.options(Selectors::default()), source("second"))
        .await
        .unwrap()
        .run()
        .await;
    let av = a.request_counts().unwrap();
    let bv = b.request_counts().unwrap();
    assert_eq!(av.admitted(), 1);
    assert_eq!(bv.admitted(), 1);
    assert!(!av.same_source(&bv));
    assert_eq!(f.requests().await.len(), 2);
}
