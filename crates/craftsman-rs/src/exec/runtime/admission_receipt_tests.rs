//! Actual loopback/client/role transport observations; no fabricated billing.
use super::*;
use crate::exec::construction::{Options, SessionSelection};
use crate::exec::input::{Invocation, Sources};
use crate::exec::meter::receipts::{Observations, Return, SendState as S};
use crate::exec::meter::{RequestLimit, Role, TokenLimit};
use crate::exec::permissions::{OnUnapproved, Selectors};
use craftsman_harness::{Msg, ToolCall, ToolErrorKind};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

struct Fixture {
    _root: tempfile::TempDir,
    workspace: PathBuf,
    config: PathBuf,
    server: MockServer,
    responses: Arc<Mutex<VecDeque<ResponseTemplate>>>,
}
impl Fixture {
    async fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("workspace");
        let config = root.path().join("selected-config");
        std::fs::create_dir(&workspace).unwrap();
        std::fs::create_dir(&config).unwrap();
        let server = MockServer::start().await;
        let responses = Arc::new(Mutex::new(VecDeque::new()));
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(Script(responses.clone()))
            .mount(&server)
            .await;
        let f = Self {
            _root: root,
            workspace,
            config,
            server,
            responses,
        };
        f.route(&f.server.uri());
        f
    }
    fn route(&self, route: &str) {
        std::fs::write(self.config.join("config.json"),json!({"model":"synthetic-receipt-model","api_key":"synthetic-receipt-key","base_url":route,"model_kwargs":{"max_tokens":1}}).to_string()).unwrap();
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
    async fn runtime(&self, budget: u64, selectors: Selectors) -> Runtime {
        Runtime::build_with_token_limit(
            Prepared::prepare(self.options(selectors)).await.unwrap(),
            RequestLimit::default(),
            TokenLimit::new(budget).unwrap(),
        )
        .await
        .unwrap()
    }
    async fn invocation(&self, budget: u64) -> Invocation {
        Invocation::prepare_with_token_limit(
            self.options(Selectors::default()),
            source("ORIGINAL receipt instruction"),
            RequestLimit::default(),
            TokenLimit::new(budget).unwrap(),
        )
        .await
        .unwrap()
    }
    fn append(&self, response: ResponseTemplate) {
        self.responses.lock().unwrap().push_back(response);
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
    async fn wait_request(&self, n: usize) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while self.history().await.len() < n {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    }
}
struct Script(Arc<Mutex<VecDeque<ResponseTemplate>>>);
impl Respond for Script {
    fn respond(&self, _: &Request) -> ResponseTemplate {
        self.0.lock().unwrap().pop_front().unwrap_or_else(|| {
            ResponseTemplate::new(400).set_body_string("unexpected synthetic request")
        })
    }
}
fn source(text: &str) -> Sources<'_> {
    Sources {
        prompt: Some(text.as_bytes()),
        prompt_file: None,
        files: &[],
    }
}
fn reply(message: Msg, usage: bool) -> ResponseTemplate {
    let Msg::Assistant {
        content,
        tool_calls,
    } = message
    else {
        panic!("fixture expects original assistant")
    };
    let calls:Vec<Value>=tool_calls.iter().enumerate().map(|(index,c)|json!({"index":index,"id":c.id,"type":"function","function":{"name":c.name,"arguments":c.args.to_string()}})).collect();
    let stop = if calls.is_empty() {
        "stop"
    } else {
        "tool_calls"
    };
    let mut text = format!(
        "data: {}\n\ndata: {}\n\n",
        json!({"choices":[{"delta":{"content":content,"tool_calls":calls}}]}),
        json!({"choices":[{"delta":{},"finish_reason":stop}]})
    );
    if usage {
        text.push_str(&format!(
            "data: {}\n\n",
            json!({"choices":[],"usage":{"prompt_tokens":20,"completion_tokens":5}})
        ));
    }
    text.push_str("data: [DONE]\n\n");
    ResponseTemplate::new(200)
        .insert_header("content-type", "text/event-stream")
        .set_body_string(text)
}
fn receipts(runtime: &Runtime) -> Observations {
    runtime.request_snapshot().unwrap().receipts().unwrap()
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
        .map(|i| {
            if i % 2 == 0 {
                Msg::user("ORIGINAL receipt context")
            } else {
                Msg::assistant("ORIGINAL receipt reply")
            }
        })
        .collect()
}

#[tokio::test]
async fn actual_run_selects_one_correlated_header_not_usage_or_body_completion() {
    let f = Fixture::new().await;
    f.append(reply(Msg::assistant("ORIGINAL final"), true));
    let run = f.invocation(u64::MAX).await.run().await;
    assert!(run.outcome().is_ok());
    let o = run.admission_receipts().unwrap().unwrap();
    assert_eq!(o.len(), 1);
    assert_eq!(o.totals(), [1, 1, 0, 0]);
    assert_eq!(
        o.call(0),
        Some((
            1,
            Role::Main,
            Some(Return::Ok),
            [S::Received, S::Unprepared],
            [Some(200), None]
        ))
    );
    assert!(o.call(1).is_none());
    let t = run.token_reservations().unwrap().unwrap();
    assert_eq!(t.allocated(), t.unsettled());
    assert_eq!(t.active(), 0);
    assert!(t.allocated() > 25);
    assert_eq!(f.history().await.len(), 1);
}
#[tokio::test]
async fn default_request_only_selection_is_none_not_a_fabricated_zero_ledger() {
    let f = Fixture::new().await;
    f.append(reply(Msg::assistant("ORIGINAL default"), true));
    let run = Invocation::prepare(
        f.options(Selectors::default()),
        source("ORIGINAL default instruction"),
    )
    .await
    .unwrap()
    .run()
    .await;
    assert!(run.outcome().is_ok());
    assert!(run.admission_receipts().unwrap().is_none());
    assert_eq!(run.request_counts().unwrap().admitted(), 1);
    assert_eq!(f.history().await.len(), 1);
}
#[tokio::test]
async fn initial_token_refusal_has_true_empty_selected_facts_and_no_request() {
    let f = Fixture::new().await;
    let run = f.invocation(1).await.run().await;
    assert!(matches!(run.outcome(), Err(RuntimeError::TokenReservation)));
    let o = run.admission_receipts().unwrap().unwrap();
    assert_eq!(o.len(), 0);
    assert_eq!(o.totals(), [0; 4]);
    assert_eq!(run.request_counts().unwrap().admitted(), 0);
    assert_eq!(run.token_reservations().unwrap().unwrap().allocated(), 0);
    assert!(f.history().await.is_empty());
}
#[tokio::test]
async fn missing_provider_usage_does_not_turn_header_into_reported_zero_or_release() {
    let f = Fixture::new().await;
    f.append(reply(Msg::assistant("ORIGINAL no usage"), false));
    let runtime = f.runtime(u64::MAX, Selectors::default()).await;
    runtime.run_turn("ORIGINAL input".into()).await.unwrap();
    assert_eq!(receipts(&runtime).totals(), [1, 1, 0, 0]);
    assert_eq!(
        *runtime.session.usage.lock().unwrap(),
        (0, 0),
        "preserve ORIGINAL indistinguishable legacy fields, not a reporting witness"
    );
    let t = runtime.request_snapshot().unwrap().tokens().unwrap();
    assert_eq!(t.allocated(), t.unsettled());
    assert!(t.allocated() > 0);
    assert_eq!(f.history().await.len(), 1);
}
#[tokio::test]
async fn genuine_http_error_retains_header_and_actual_call_error_without_fake_send_error() {
    let f = Fixture::new().await;
    f.append(ResponseTemplate::new(400).set_body_string("ORIGINAL synthetic failure"));
    let runtime = f.runtime(u64::MAX, Selectors::default()).await;
    assert!(runtime.run_turn("ORIGINAL input".into()).await.is_err());
    assert_eq!(
        receipts(&runtime).call(0),
        Some((
            1,
            Role::Main,
            Some(Return::Error),
            [S::Received, S::Unprepared],
            [Some(400), None]
        ))
    );
    assert_eq!(receipts(&runtime).totals(), [1, 1, 0, 0]);
    assert_eq!(f.history().await.len(), 1);
}
#[tokio::test]
async fn retry_receipts_stay_with_one_logical_admission_not_two_request_charges() {
    let f = Fixture::new().await;
    f.append(ResponseTemplate::new(503).set_body_string("ORIGINAL first error"));
    f.append(reply(Msg::assistant("ORIGINAL retried final"), true));
    let runtime = f.runtime(u64::MAX, Selectors::default()).await;
    runtime.run_turn("ORIGINAL input".into()).await.unwrap();
    assert_eq!(runtime.request_snapshot().unwrap().admitted(), 1);
    assert_eq!(
        receipts(&runtime).call(0),
        Some((
            1,
            Role::Main,
            Some(Return::Ok),
            [S::Received, S::Received],
            [Some(503), Some(200)]
        ))
    );
    assert_eq!(receipts(&runtime).totals(), [2, 2, 0, 0]);
    let h = f.history().await;
    assert_eq!(h.len(), 2);
    assert_eq!(h[0], h[1]);
    let t = runtime.request_snapshot().unwrap().tokens().unwrap();
    assert_eq!(t.role(Role::Main)[1], 1);
    assert_eq!(t.allocated(), t.unsettled());
}
#[tokio::test]
async fn both_real_failed_http_receipts_remain_without_third_attempt_or_usage_classification() {
    let f = Fixture::new().await;
    for _ in 0..2 {
        f.append(ResponseTemplate::new(503).set_body_string("ORIGINAL repeated error"));
    }
    let runtime = f.runtime(u64::MAX, Selectors::default()).await;
    assert!(runtime.run_turn("ORIGINAL input".into()).await.is_err());
    assert_eq!(
        receipts(&runtime).call(0),
        Some((
            1,
            Role::Main,
            Some(Return::Error),
            [S::Received, S::Received],
            [Some(503), Some(503)]
        ))
    );
    assert_eq!(receipts(&runtime).totals(), [2, 2, 0, 0]);
    assert_eq!(runtime.request_snapshot().unwrap().admitted(), 1);
    assert_eq!(f.history().await.len(), 2);
}
#[tokio::test]
async fn incomplete_unshown_stream_retry_preserves_two_original_headers_not_body_success() {
    let f = Fixture::new().await;
    f.append(
        ResponseTemplate::new(200)
            .insert_header("content-type", "text/event-stream")
            .set_body_string("data: {\"choices\":[]}\n\n"),
    );
    f.append(reply(Msg::assistant("ORIGINAL final"), true));
    let runtime = f.runtime(u64::MAX, Selectors::default()).await;
    runtime.run_turn("ORIGINAL input".into()).await.unwrap();
    assert_eq!(receipts(&runtime).totals(), [2, 2, 0, 0]);
    assert_eq!(
        receipts(&runtime).call(0).unwrap().4,
        [Some(200), Some(200)]
    );
    assert_eq!(f.history().await.len(), 2);
}
#[tokio::test]
async fn visible_incomplete_stream_header_survives_real_call_failure_without_fake_eof() {
    let f = Fixture::new().await;
    f.append(
        ResponseTemplate::new(200)
            .insert_header("content-type", "text/event-stream")
            .set_body_string(
                "data: {\"choices\":[{\"delta\":{\"content\":\"ORIGINAL partial\"}}]}\n\n",
            ),
    );
    let runtime = f.runtime(u64::MAX, Selectors::default()).await;
    assert!(runtime.run_turn("ORIGINAL input".into()).await.is_err());
    assert_eq!(
        receipts(&runtime).call(0),
        Some((
            1,
            Role::Main,
            Some(Return::Error),
            [S::Received, S::Unprepared],
            [Some(200), None]
        ))
    );
    assert_eq!(f.history().await.len(), 1);
}
#[tokio::test]
async fn actual_send_errors_are_polled_results_not_fabricated_http_headers() {
    use tokio::io::AsyncReadExt;
    let f = Fixture::new().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    f.route(&format!("http://{}", listener.local_addr().unwrap()));
    let server = tokio::spawn(async move {
        for _ in 0..2 {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 1024];
            let _ = s.read(&mut buf).await;
            drop(s);
        }
    });
    let runtime = f.runtime(u64::MAX, Selectors::default()).await;
    assert!(runtime.run_turn("ORIGINAL input".into()).await.is_err());
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(receipts(&runtime).totals(), [2, 0, 2, 0]);
    assert_eq!(
        receipts(&runtime).call(0),
        Some((
            1,
            Role::Main,
            Some(Return::Error),
            [S::SendError, S::SendError],
            [None, None]
        ))
    );
    assert_eq!(runtime.request_snapshot().unwrap().admitted(), 1);
}
#[tokio::test]
async fn actual_pending_send_drop_records_abandonment_not_a_response_or_failed_return() {
    let f = Fixture::new().await;
    f.append(reply(Msg::assistant("ORIGINAL pending"), true).set_delay(Duration::from_secs(30)));
    let runtime = f.runtime(u64::MAX, Selectors::default()).await;
    let meter = runtime.request_meter.clone();
    let task = tokio::spawn(async move { runtime.run_turn("ORIGINAL input".into()).await });
    f.wait_request(1).await;
    assert_eq!(
        meter
            .snapshot()
            .unwrap()
            .receipts()
            .unwrap()
            .call(0)
            .unwrap()
            .3,
        [S::Polled, S::Unprepared]
    );
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(
        meter.snapshot().unwrap().receipts().unwrap().call(0),
        Some((
            1,
            Role::Main,
            Some(Return::Abandoned),
            [S::Abandoned, S::Unprepared],
            [None, None]
        ))
    );
    assert_eq!(
        meter.snapshot().unwrap().receipts().unwrap().totals(),
        [1, 0, 0, 1]
    );
    assert_eq!(f.history().await.len(), 1);
}
#[tokio::test]
async fn real_header_survives_abandoned_body_without_claiming_send_abandonment_or_eof() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let f = Fixture::new().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    f.route(&format!("http://{}", listener.local_addr().unwrap()));
    let server = tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 1024];
        let mut original_headers = Vec::new();
        loop {
            let n = s.read(&mut buf).await.unwrap();
            assert!(n > 0);
            assert!(original_headers.len() + n <= 16 * 1024);
            original_headers.extend_from_slice(&buf[..n]);
            if original_headers.windows(4).any(|b| b == b"\r\n\r\n") {
                break;
            }
        }
        s.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: 10000\r\nConnection: close\r\n\r\n").await.unwrap();
        s.flush().await.unwrap();
        tokio::time::sleep(Duration::from_secs(30)).await;
    });
    let runtime = f.runtime(u64::MAX, Selectors::default()).await;
    let meter = runtime.request_meter.clone();
    let task = tokio::spawn(async move { runtime.run_turn("ORIGINAL input".into()).await });
    tokio::time::timeout(Duration::from_secs(5), async {
        while meter.snapshot().unwrap().receipts().unwrap().totals()[1] != 1 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    server.abort();
    let _ = server.await;
    assert_eq!(
        meter.snapshot().unwrap().receipts().unwrap().call(0),
        Some((
            1,
            Role::Main,
            Some(Return::Abandoned),
            [S::Received, S::Unprepared],
            [Some(200), None]
        ))
    );
    assert_eq!(
        meter.snapshot().unwrap().receipts().unwrap().totals(),
        [1, 1, 0, 0]
    );
}
#[tokio::test]
async fn genuine_header_after_typed_stop_is_preserved_while_approval_dominates_return() {
    let f = Fixture::new().await;
    f.append(
        reply(Msg::assistant("ORIGINAL late response"), true).set_delay(Duration::from_millis(250)),
    );
    let runtime = f
        .runtime(
            u64::MAX,
            Selectors {
                on_unapproved: OnUnapproved::Fail,
                ..Default::default()
            },
        )
        .await;
    let gate = runtime.gate.clone();
    let task = tokio::spawn(async move {
        let result = runtime.run_turn("ORIGINAL input".into()).await;
        (runtime, result)
    });
    f.wait_request(1).await;
    gate.observe_error_kind(ToolErrorKind::NeedsApproval);
    let (runtime, result) = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(result, Err(RuntimeError::ApprovalRequired)));
    assert_eq!(
        receipts(&runtime).call(0),
        Some((
            1,
            Role::Main,
            Some(Return::Error),
            [S::Received, S::Unprepared],
            [Some(200), None]
        ))
    );
    assert!(runtime.gate.approval_required());
    assert_eq!(f.history().await.len(), 1);
}
#[tokio::test]
async fn deny_only_seal_after_poll_preserves_genuine_header_without_false_approval() {
    let f = Fixture::new().await;
    f.append(
        reply(Msg::assistant("ORIGINAL late response"), true).set_delay(Duration::from_millis(250)),
    );
    let runtime = f.runtime(u64::MAX, Selectors::default()).await;
    let gate = runtime.gate.clone();
    let task = tokio::spawn(async move {
        let result = runtime.run_turn("ORIGINAL input".into()).await;
        (runtime, result)
    });
    f.wait_request(1).await;
    gate.close_execution();
    let (runtime, result) = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
    assert!(result.is_err());
    assert!(!runtime.gate.approval_required());
    assert_eq!(receipts(&runtime).call(0).unwrap().4, [Some(200), None]);
    assert_eq!(f.history().await.len(), 1);
}
#[tokio::test]
async fn real_main_child_main_headers_bind_concrete_roles_and_monotonic_logical_keys() {
    let f = Fixture::new().await;
    f.append(reply(Msg::Assistant{content:"ORIGINAL partial".into(),tool_calls:vec![call("original-child","task",json!({"subagent_type":"general-purpose","description":"ORIGINAL child instruction"}))]},true));
    f.append(reply(Msg::assistant("ORIGINAL child result"), true));
    f.append(reply(Msg::assistant("ORIGINAL parent final"), true));
    let runtime = f.runtime(u64::MAX, Selectors::default()).await;
    runtime.run_turn("ORIGINAL input".into()).await.unwrap();
    let o = receipts(&runtime);
    assert_eq!(o.len(), 3);
    assert_eq!(o.totals(), [3, 3, 0, 0]);
    for (i, role) in [Role::Main, Role::Subagent, Role::Main]
        .into_iter()
        .enumerate()
    {
        assert_eq!(
            o.call(i),
            Some((
                i as u64 + 1,
                role,
                Some(Return::Ok),
                [S::Received, S::Unprepared],
                [Some(200), None]
            ))
        );
    }
    assert_eq!(f.history().await.len(), 3);
}
#[tokio::test]
async fn actual_compaction_and_main_use_same_ledger_without_model_name_role_inference() {
    let f = Fixture::new().await;
    f.append(reply(Msg::assistant("ORIGINAL summary"), true));
    f.append(reply(Msg::assistant("ORIGINAL final"), true));
    let runtime = f.runtime(u64::MAX, Selectors::default()).await;
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
    let o = receipts(&runtime);
    assert_eq!(o.call(0).unwrap().1, Role::Compaction);
    assert_eq!(o.call(1).unwrap().1, Role::Main);
    assert_eq!(o.totals(), [2, 2, 0, 0]);
    assert_eq!(f.history().await.len(), 2);
}
#[tokio::test]
async fn snapshots_are_historical_source_bound_not_current_or_equal_fresh_aliases() {
    let f = Fixture::new().await;
    f.append(reply(Msg::assistant("ORIGINAL one"), true));
    f.append(reply(Msg::assistant("ORIGINAL two"), true));
    let a = f.runtime(u64::MAX, Selectors::default()).await;
    let old = receipts(&a);
    a.run_turn("ORIGINAL input".into()).await.unwrap();
    let now = receipts(&a);
    assert!(old.same_source(&now));
    assert_eq!(old.len(), 0);
    assert_eq!(now.len(), 1);
    let b = f.runtime(u64::MAX, Selectors::default()).await;
    b.run_turn("ORIGINAL input".into()).await.unwrap();
    let foreign = receipts(&b);
    assert_eq!(now.totals(), foreign.totals());
    assert!(!now.same_source(&foreign));
    assert_eq!(
        f.history().await.len(),
        2,
        "retain original prior request, never reset history"
    );
}
#[tokio::test]
async fn multiple_actual_turns_keep_admission_keys_without_resetting_prior_receipts() {
    let f = Fixture::new().await;
    f.append(reply(Msg::assistant("ORIGINAL first"), true));
    f.append(reply(Msg::assistant("ORIGINAL second"), true));
    let runtime = f.runtime(u64::MAX, Selectors::default()).await;
    runtime
        .run_turn("ORIGINAL first instruction".into())
        .await
        .unwrap();
    let old = receipts(&runtime);
    runtime
        .run_turn("ORIGINAL second instruction".into())
        .await
        .unwrap();
    let now = receipts(&runtime);
    assert_eq!(old.len(), 1);
    assert_eq!(now.len(), 2);
    assert_eq!(now.call(0), old.call(0));
    assert_eq!(now.call(1).unwrap().0, 2);
    assert_eq!(f.history().await.len(), 2);
}
#[tokio::test]
async fn actual_poison_refuses_all_observation_snapshots_without_free_zero_facts() {
    let f = Fixture::new().await;
    let runtime = f.runtime(u64::MAX, Selectors::default()).await;
    let old = receipts(&runtime);
    runtime.request_meter.poison_for_tests();
    assert!(runtime.request_snapshot().is_err());
    assert!(matches!(
        runtime.run_turn("ORIGINAL input".into()).await,
        Err(RuntimeError::RequestAccounting)
    ));
    assert_eq!(old.len(), 0);
    assert!(runtime.gate.execution_closed());
    assert!(f.history().await.is_empty());
}

#[tokio::test]
async fn original_named_child_header_has_concrete_subagent_role_not_an_identity_alias() {
    let f = Fixture::new().await;
    let plugins = f.config.join("plugins");
    std::fs::create_dir_all(&plugins).unwrap();
    std::fs::write(plugins.join("receipt-analyst.json"),json!({"version":1,"name":"receipt-analysis-bundle","agents":[{"name":"original-receipt-analyst","description":"ORIGINAL receipt analyst","system_prompt":"ORIGINAL-NAMED-RECEIPT-CONTEXT","tools":[]}]}).to_string()).unwrap();
    f.append(reply(Msg::Assistant{content:"ORIGINAL partial".into(),tool_calls:vec![call("original-named","task",json!({"subagent_type":"original-receipt-analyst","description":"ORIGINAL named child instruction"}))]},true));
    f.append(reply(Msg::assistant("ORIGINAL named answer"), true));
    f.append(reply(Msg::assistant("ORIGINAL parent answer"), true));
    let runtime = f.runtime(u64::MAX, Selectors::default()).await;
    runtime.run_turn("ORIGINAL input".into()).await.unwrap();
    let o = receipts(&runtime);
    assert_eq!(o.len(), 3);
    assert_eq!(o.call(1).unwrap().1, Role::Subagent);
    assert_eq!(o.call(1).unwrap().4, [Some(200), None]);
    let h = f.history().await;
    assert_eq!(h.len(), 3);
    assert!(
        h[1]["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains("ORIGINAL-NAMED-RECEIPT-CONTEXT")
    );
}
