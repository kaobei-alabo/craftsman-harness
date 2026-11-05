//! Actual input/runtime/loopback reservations, not synthetic usage ledgers.
use super::*;
use crate::exec::construction::{Options, SessionSelection};
use crate::exec::input::{Invocation, Sources};
use crate::exec::meter::{RequestLimit, Role, TokenLimit};
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
    script: Arc<Mutex<VecDeque<Msg>>>,
}
impl Fixture {
    async fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("workspace");
        let config = root.path().join("selected-config");
        std::fs::create_dir(&workspace).unwrap();
        std::fs::create_dir(&config).unwrap();
        let server = MockServer::start().await;
        std::fs::write(config.join("config.json"),json!({"model":"synthetic-reservation-runtime-model","api_key":"synthetic-reservation-runtime-key","base_url":server.uri(),"model_kwargs":{"max_tokens":1}}).to_string()).unwrap();
        let script = Arc::new(Mutex::new(VecDeque::new()));
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(Script(script.clone()))
            .mount(&server)
            .await;
        Self {
            _root: root,
            workspace,
            config,
            server,
            script,
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
    async fn runtime(&self, tokens: u64, requests: u64, selectors: Selectors) -> Runtime {
        let requests = RequestLimit::new(requests).unwrap();
        let tokens = TokenLimit::new(tokens).unwrap();
        Runtime::build_with_token_limit(
            Prepared::prepare(self.options(selectors)).await.unwrap(),
            requests,
            tokens,
        )
        .await
        .unwrap()
    }
    async fn invocation(&self, tokens: u64) -> Invocation {
        Invocation::prepare_with_token_limit(
            self.options(Selectors::default()),
            source("ORIGINAL input instruction"),
            RequestLimit::default(),
            TokenLimit::new(tokens).unwrap(),
        )
        .await
        .unwrap()
    }
    fn append(&self, messages: impl IntoIterator<Item = Msg>) {
        self.script.lock().unwrap().extend(messages);
    }
    async fn history(&self) -> Vec<Value> {
        self.server
            .received_requests()
            .await
            .unwrap()
            .into_iter()
            .map(|r| serde_json::from_slice(&r.body).unwrap())
            .collect()
    }
    fn named_child(&self) {
        let plugins = self.config.join("plugins");
        std::fs::create_dir_all(&plugins).unwrap();
        std::fs::write(plugins.join("reservation-child.json"),json!({"version":1,"name":"reservation-child-bundle","agents":[{"name":"original-analyst","description":"Original foreground analyst","system_prompt":"ORIGINAL-NAMED-CHILD-CONTEXT","tools":[]}]}).to_string()).unwrap();
    }
}
struct Script(Arc<Mutex<VecDeque<Msg>>>);
impl Respond for Script {
    fn respond(&self, _: &Request) -> ResponseTemplate {
        let Some(Msg::Assistant {
            content,
            tool_calls,
        }) = self.0.lock().unwrap().pop_front()
        else {
            return ResponseTemplate::new(400)
                .set_body_string("unexpected original synthetic request");
        };
        let calls:Vec<Value>=tool_calls.iter().enumerate().map(|(index,c)|json!({"index":index,"id":c.id,"type":"function","function":{"name":c.name,"arguments":c.args.to_string()}})).collect();
        let stop = if calls.is_empty() {
            "stop"
        } else {
            "tool_calls"
        };
        ResponseTemplate::new(200)
            .insert_header("content-type", "text/event-stream")
            .set_body_string(format!(
                "data: {}\n\ndata: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
                json!({"choices":[{"delta":{"content":content,"tool_calls":calls}}]}),
                json!({"choices":[{"delta":{},"finish_reason":stop}]}),
                json!({"choices":[],"usage":{"prompt_tokens":20,"completion_tokens":5}})
            ))
    }
}
fn source(text: &str) -> Sources<'_> {
    Sources {
        prompt: Some(text.as_bytes()),
        prompt_file: None,
        files: &[],
    }
}
fn calls(calls: Vec<ToolCall>) -> Msg {
    Msg::Assistant {
        content: "ORIGINAL partial answer".into(),
        tool_calls: calls,
    }
}
fn call(id: &str, name: &str, args: Value) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: name.into(),
        args,
    }
}
fn context() -> Vec<Msg> {
    (0..128)
        .map(|n| {
            if n % 2 == 0 {
                Msg::user("ORIGINAL retained context")
            } else {
                Msg::assistant("ORIGINAL retained reply")
            }
        })
        .collect()
}
/// Independent test-only arithmetic over an ACTUAL received request. Preserve
/// original histories; calibration runs remain real runs, never reset to zero.
fn estimate_actual(body: &Value) -> u64 {
    let mut input = body.clone();
    let object = input.as_object_mut().unwrap();
    object.remove("max_tokens");
    object.remove("max_completion_tokens");
    let encoded = serde_json::to_string(&input).unwrap();
    assert!(encoded.len() <= 1024 * 1024);
    let bpe = tiktoken_rs::cl100k_base().unwrap();
    let mut text = encoded.as_str();
    let mut count = 0;
    while !text.is_empty() {
        let mut end = text.len().min(4096);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        count += bpe.count_ordinary(&text[..end]) as u64;
        text = &text[end..];
    }
    count
}

#[tokio::test]
async fn actual_input_refusal_retains_zero_calls_and_allocations_without_public_output_claim() {
    let f = Fixture::new().await;
    let run = f.invocation(1).await.run().await;
    assert!(matches!(run.outcome(), Err(RuntimeError::TokenReservation)));
    assert_eq!(run.request_counts().unwrap().admitted(), 0);
    let tokens = run.token_reservations().unwrap().unwrap();
    assert_eq!(
        (
            tokens.maximum(),
            tokens.allocated(),
            tokens.active(),
            tokens.unsettled()
        ),
        (1, 0, 0, 0)
    );
    let refusal = tokens.refusal().unwrap();
    assert_eq!(refusal.0, Role::Main);
    assert_eq!((refusal.1, refusal.2, refusal.4, refusal.5), (1, 0, 1, 1));
    assert!(refusal.3 > 1);
    assert!(f.history().await.is_empty());
    assert!(!run.retained_state().messages.is_empty());
    let terminal = run.terminal();
    let snapshot = terminal.snapshot().unwrap();
    let frame: Value = serde_json::from_slice(snapshot.bytes().unwrap()).unwrap();
    assert_eq!(frame["run_return"], "turn_failed");
    assert_eq!(
        frame["final_output"],
        Value::Null,
        "private preview is NOT #97's schema-backed initial empty string"
    );
}
#[tokio::test]
async fn actual_run_exposes_historical_unsettled_reservations_from_its_same_source_only() {
    let f = Fixture::new().await;
    f.append([Msg::assistant("ORIGINAL final answer")]);
    let run = f.invocation(u64::MAX).await.run().await;
    assert_eq!(
        run.outcome().as_ref().unwrap().final_text(),
        "ORIGINAL final answer"
    );
    assert_eq!(
        run.request_counts().unwrap().role(Role::Main),
        [1, 0, 1, 0, 0]
    );
    let a = run.token_reservations().unwrap().unwrap();
    let b = run.token_reservations().unwrap().unwrap();
    assert!(a.same_source(&b));
    let history = f.history().await;
    assert_eq!(history.len(), 1);
    let input = estimate_actual(&history[0]);
    assert_eq!(a.role(Role::Main), [input, 1, 0, input + 1]);
    assert_eq!(a.allocated(), input + 1);
    assert_eq!(a.unsettled(), input + 1);
    assert_eq!(a.active(), 0);
    assert!(a.refusal().is_none());
    assert!(
        a.allocated() > 25,
        "reservation is NOT the real supplied response Usage(20,5)"
    );
}
#[tokio::test]
async fn unchanged_default_input_run_selects_request_only_not_token_allocations() {
    let f = Fixture::new().await;
    f.append([Msg::assistant("ORIGINAL default final")]);
    let run = Invocation::prepare(
        f.options(Selectors::default()),
        source("ORIGINAL default instruction"),
    )
    .await
    .unwrap()
    .run()
    .await;
    assert!(run.outcome().is_ok());
    assert_eq!(run.request_counts().unwrap().admitted(), 1);
    assert!(run.token_reservations().unwrap().is_none());
    assert_eq!(f.history().await.len(), 1);
}
#[tokio::test]
async fn distinct_actual_runs_do_not_alias_equal_reservation_snapshots() {
    let f = Fixture::new().await;
    f.append([
        Msg::assistant("first actual final"),
        Msg::assistant("second actual final"),
    ]);
    let a = f.invocation(u64::MAX).await.run().await;
    let b = f.invocation(u64::MAX).await.run().await;
    let sa = a.token_reservations().unwrap().unwrap();
    let sb = b.token_reservations().unwrap().unwrap();
    assert_eq!(sa.allocated(), sb.allocated());
    assert!(!sa.same_source(&sb));
    assert_eq!(a.request_counts().unwrap().admitted(), 1);
    assert_eq!(b.request_counts().unwrap().admitted(), 1);
    assert_eq!(
        f.history().await.len(),
        2,
        "never reset ORIGINAL first-run history"
    );
}
#[tokio::test]
async fn real_general_purpose_child_and_main_use_the_same_reservation_meter() {
    let f = Fixture::new().await;
    f.append([
        calls(vec![call(
            "original-child",
            "task",
            json!({"subagent_type":"general-purpose","description":"ORIGINAL child instruction"}),
        )]),
        Msg::assistant("ORIGINAL child result"),
        Msg::assistant("ORIGINAL parent final"),
    ]);
    let runtime = f.runtime(u64::MAX, 50, Selectors::default()).await;
    assert_eq!(
        runtime
            .run_turn("ORIGINAL parent input".into())
            .await
            .unwrap()
            .final_text(),
        "ORIGINAL parent final"
    );
    let counts = runtime.request_snapshot().unwrap();
    assert_eq!(counts.role(Role::Main), [2, 0, 2, 0, 0]);
    assert_eq!(counts.role(Role::Subagent), [1, 0, 1, 0, 0]);
    let t = counts.tokens().unwrap();
    let history = f.history().await;
    assert_eq!(history.len(), 3);
    assert_eq!(t.role(Role::Main)[1], 2);
    assert_eq!(t.role(Role::Subagent)[1], 1);
    assert_eq!(
        t.allocated(),
        history.iter().map(|r| estimate_actual(r) + 1).sum::<u64>()
    );
    assert_eq!(t.active(), 0);
    assert_eq!(t.unsettled(), t.allocated());
    assert_eq!(
        *runtime.session.usage.lock().unwrap(),
        (40, 10),
        "keep real main usage separate, not a guessed all-role ledger"
    );
}
#[tokio::test]
async fn named_child_source_is_original_concrete_role_not_a_label_alias() {
    let f = Fixture::new().await;
    f.named_child();
    f.append([
        calls(vec![call(
            "original-named-child",
            "task",
            json!({"subagent_type":"original-analyst","description":"ORIGINAL named instruction"}),
        )]),
        Msg::assistant("ORIGINAL named result"),
        Msg::assistant("ORIGINAL parent final"),
    ]);
    let runtime = f.runtime(u64::MAX, 50, Selectors::default()).await;
    runtime.run_turn("ORIGINAL input".into()).await.unwrap();
    let counts = runtime.request_snapshot().unwrap();
    assert_eq!(counts.role(Role::Subagent), [1, 0, 1, 0, 0]);
    let t = counts.tokens().unwrap();
    assert_eq!(t.role(Role::Subagent)[1], 1);
    let history = f.history().await;
    assert_eq!(history.len(), 3);
    assert!(
        history[1]["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains("ORIGINAL-NAMED-CHILD-CONTEXT")
    );
}
#[tokio::test]
async fn real_manual_compaction_and_main_reserve_from_the_same_source() {
    let f = Fixture::new().await;
    f.append([
        Msg::assistant("ORIGINAL manual summary"),
        Msg::assistant("ORIGINAL final"),
    ]);
    let runtime = f.runtime(u64::MAX, 50, Selectors::default()).await;
    assert!(
        runtime
            .session
            .compaction
            .compact_now(&context())
            .await
            .unwrap()
            .is_some()
    );
    runtime.run_turn("ORIGINAL input".into()).await.unwrap();
    let counts = runtime.request_snapshot().unwrap();
    assert_eq!(counts.role(Role::Compaction), [1, 0, 1, 0, 0]);
    assert_eq!(counts.role(Role::Main), [1, 0, 1, 0, 0]);
    let t = counts.tokens().unwrap();
    let history = f.history().await;
    assert_eq!(history.len(), 2);
    assert_eq!(t.role(Role::Compaction)[1], 1);
    assert_eq!(
        t.allocated(),
        history.iter().map(|r| estimate_actual(r) + 1).sum::<u64>()
    );
}
#[tokio::test]
async fn caught_automatic_compaction_initial_token_refusal_cannot_be_completed() {
    let f = Fixture::new().await;
    let runtime = f.runtime(1, 50, Selectors::default()).await;
    runtime
        .session
        .agent
        .state
        .lock()
        .unwrap()
        .messages
        .extend(context());
    assert!(matches!(
        runtime.run_turn("ORIGINAL input".into()).await,
        Err(RuntimeError::TokenReservation)
    ));
    let counts = runtime.request_snapshot().unwrap();
    assert_eq!(counts.admitted(), 0);
    assert_eq!(
        counts.tokens().unwrap().refusal().unwrap().0,
        Role::Compaction
    );
    assert!(f.history().await.is_empty());
    assert!(runtime.gate.execution_closed());
    assert!(!runtime.gate.approval_required());
}
#[tokio::test]
async fn exact_first_compaction_allocation_stops_main_and_preserves_original_calibration_history() {
    let f = Fixture::new().await;
    f.append([
        Msg::assistant("ORIGINAL automatic summary"),
        Msg::assistant("ORIGINAL calibration final"),
    ]);
    let calibration = f.runtime(u64::MAX, 50, Selectors::default()).await;
    calibration
        .session
        .agent
        .state
        .lock()
        .unwrap()
        .messages
        .extend(context());
    calibration
        .run_turn("ORIGINAL same input".into())
        .await
        .unwrap();
    let before = f.history().await;
    assert_eq!(before.len(), 2);
    let first = estimate_actual(&before[0]);
    f.append([Msg::assistant("ORIGINAL automatic summary")]);
    let runtime = f.runtime(first + 1, 50, Selectors::default()).await;
    runtime
        .session
        .agent
        .state
        .lock()
        .unwrap()
        .messages
        .extend(context());
    assert!(matches!(
        runtime.run_turn("ORIGINAL same input".into()).await,
        Err(RuntimeError::TokenReservation)
    ));
    let history = f.history().await;
    assert_eq!(
        history.len(),
        3,
        "calibration's two ORIGINAL requests are not reset/hidden"
    );
    assert_eq!(&history[..2], &before);
    assert_eq!(history[2], before[0]);
    let counts = runtime.request_snapshot().unwrap();
    assert_eq!(counts.role(Role::Compaction), [1, 0, 1, 0, 0]);
    assert_eq!(counts.role(Role::Main), [0; 5]);
    let t = counts.tokens().unwrap();
    assert_eq!(t.allocated(), first + 1);
    assert_eq!(t.refusal().unwrap().0, Role::Main);
    assert_eq!(
        calibration.request_snapshot().unwrap().admitted(),
        2,
        "new meter cannot change the actual earlier run"
    );
}
#[tokio::test]
async fn child_token_stop_denies_same_response_explicitly_allowed_later_write() {
    let f = Fixture::new().await;
    let selectors = Selectors {
        allow: vec!["tool:write_file".into()],
        on_unapproved: OnUnapproved::Fail,
        ..Default::default()
    };
    f.append([Msg::assistant("ORIGINAL calibration response")]);
    let calibration = f.runtime(u64::MAX, 50, selectors.clone()).await;
    calibration
        .run_turn("ORIGINAL same input".into())
        .await
        .unwrap();
    let before = f.history().await;
    assert_eq!(before.len(), 1);
    let input = estimate_actual(&before[0]);
    f.append([calls(vec![
        call(
            "original-child",
            "task",
            json!({"subagent_type":"general-purpose","description":"ORIGINAL child instruction"}),
        ),
        call(
            "original-later",
            "write_file",
            json!({"file_path":"must-not-write.txt","content":"ORIGINAL forbidden later write"}),
        ),
    ])]);
    let runtime = f.runtime(input + 1, 50, selectors).await;
    assert!(matches!(
        runtime.run_turn("ORIGINAL same input".into()).await,
        Err(RuntimeError::TokenReservation)
    ));
    assert!(!f.workspace.join("must-not-write.txt").exists());
    assert!(runtime.gate.execution_closed());
    assert!(!runtime.gate.approval_required());
    let counts = runtime.request_snapshot().unwrap();
    assert_eq!(counts.role(Role::Main), [1, 0, 1, 0, 0]);
    assert_eq!(counts.role(Role::Subagent), [0; 5]);
    let t = counts.tokens().unwrap();
    assert_eq!(t.allocated(), input + 1);
    assert_eq!(t.refusal().unwrap().0, Role::Subagent);
    assert!(runtime.retained_state().messages.iter().any(|m|matches!(m,Msg::Tool{tool_call_id,is_error,..} if tool_call_id=="original-later"&&*is_error)));
    let history = f.history().await;
    assert_eq!(
        history.len(),
        2,
        "retain actual calibration plus actual new main, no manufactured zero"
    );
    assert_eq!(history[0], before[0]);
    assert_eq!(history[1], before[0]);
    assert_eq!(calibration.request_snapshot().unwrap().admitted(), 1);
}
#[tokio::test]
async fn actual_late_typed_approval_dominates_private_token_refusal_without_reset() {
    let f = Fixture::new().await;
    let runtime = f
        .runtime(
            1,
            50,
            Selectors {
                on_unapproved: OnUnapproved::Fail,
                ..Default::default()
            },
        )
        .await;
    assert!(matches!(
        runtime.run_turn("ORIGINAL input".into()).await,
        Err(RuntimeError::TokenReservation)
    ));
    let refused = runtime
        .request_snapshot()
        .unwrap()
        .tokens()
        .unwrap()
        .refusal();
    let old = runtime.retained_state().messages;
    runtime
        .gate
        .observe_error_kind(ToolErrorKind::NeedsApproval);
    assert!(matches!(
        runtime.run_turn("must not enter".into()).await,
        Err(RuntimeError::ApprovalRequired)
    ));
    assert_eq!(
        runtime
            .request_snapshot()
            .unwrap()
            .tokens()
            .unwrap()
            .refusal(),
        refused
    );
    assert_eq!(runtime.retained_state().messages, old);
    assert!(f.history().await.is_empty());
}
#[tokio::test]
async fn actual_poisoned_token_runtime_refuses_before_traffic_and_never_fabricates_snapshot() {
    let f = Fixture::new().await;
    let runtime = f.runtime(u64::MAX, 50, Selectors::default()).await;
    let before = runtime.request_snapshot().unwrap();
    runtime.request_meter.poison_for_tests();
    assert!(matches!(
        runtime.run_turn("ORIGINAL input".into()).await,
        Err(RuntimeError::RequestAccounting)
    ));
    assert!(runtime.request_snapshot().is_err());
    assert!(f.history().await.is_empty());
    assert_eq!(before.admitted(), 0);
    assert!(runtime.gate.execution_closed());
}
