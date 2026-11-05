//! Actual-builder scopes/clients and private admission facts; no fake usage.
use super::*;
use crate::exec::construction::{Options, Prepared, SessionSelection};
use crate::exec::meter::tokens::{admit_for_tests, input_for_tests};
use crate::exec::meter::{
    Fault, Meter, RequestLimit, Role, TestSnapshot, TestTokenSnapshot, TokenLimit,
};
use crate::exec::permissions::{OnUnapproved, Selectors};
use crate::exec::runtime::InvocationGate;
use craftsman_harness::ToolErrorKind;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

struct Fixture {
    _root: tempfile::TempDir,
    prepared: Prepared,
    _session: Session,
    gate: Arc<InvocationGate>,
    meter: Arc<Meter>,
    server: MockServer,
    options: BTreeMap<String, Value>,
}
impl Fixture {
    async fn new(tokens: Option<u64>, requests: u64, kwargs: Value, key: &str, fail: bool) -> Self {
        let limit = RequestLimit::new(requests).unwrap();
        let tokens = tokens.map(|n| TokenLimit::new(n).unwrap());
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("workspace");
        let config = root.path().join("selected-config");
        std::fs::create_dir(&workspace).unwrap();
        std::fs::create_dir(&config).unwrap();
        let server = MockServer::start().await;
        std::fs::write(
            config.join("config.json"),
            json!({
                "model":"synthetic-reservation-model", "api_key":key,
                "base_url":server.uri(), "model_kwargs":kwargs
            })
            .to_string(),
        )
        .unwrap();
        let prepared = Prepared::prepare(Options {
            workspace,
            config_dir: config,
            trust_workspace: false,
            session: SessionSelection::Ephemeral,
            selectors: Selectors {
                on_unapproved: if fail {
                    OnUnapproved::Fail
                } else {
                    OnUnapproved::Deny
                },
                ..Default::default()
            },
        })
        .await
        .unwrap();
        let (session, gate, meter) = build_prepared_session_with_limits(&prepared, limit, tokens)
            .await
            .unwrap();
        // Resolve the ORIGINAL effective options with the same profile functions,
        // not a caller alias or invented provider defaults.
        let cfg = prepared.config();
        let model = cfg.model.as_deref().unwrap();
        let resolved = crate::harness_profiles::resolve(cfg, prepared.credentials().route(), model);
        let mut caller = cfg.model_profile(model);
        if let Some(kwargs) = cfg.extra.get("model_kwargs").and_then(Value::as_object) {
            caller.extend(kwargs.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
        let spec = resolved.provider.as_ref().map_or_else(
            || resolved.model_id.clone(),
            |p| format!("{p}:{}", resolved.model_id),
        );
        let options = resolved
            .provider_profile
            .profile
            .apply(&spec, &caller)
            .unwrap_or(caller);
        Self {
            _root: root,
            prepared,
            _session: session,
            gate,
            meter,
            server,
            options,
        }
    }
    async fn usual(tokens: u64, reply: u64) -> Self {
        Self::new(
            Some(tokens),
            50,
            json!({"max_tokens":reply}),
            "synthetic-reservation-key",
            false,
        )
        .await
    }
    async fn rebuild(&mut self, tokens: u64, requests: u64) {
        let (session, gate, meter) = build_prepared_session_with_limits(
            &self.prepared,
            RequestLimit::new(requests).unwrap(),
            Some(TokenLimit::new(tokens).unwrap()),
        )
        .await
        .unwrap();
        self._session = session;
        self.gate = gate;
        self.meter = meter;
    }
    fn provider_scope(&self) -> ProviderScope<'_> {
        ProviderScope {
            prepared: &self.prepared,
            gate: self.gate.clone(),
            model: self.prepared.config().model.as_deref().unwrap(),
            options: &self.options,
            usage: Arc::new(Mutex::new((0, 0))),
            agents: &[],
        }
    }
    fn scope(&self, role: Role) -> TokenProviderScope {
        TokenProviderScope {
            meter: self.meter.clone(),
            gate: self.gate.clone(),
            role,
            original: crate::exec::provider::Provider::from_scope(self.provider_scope()).unwrap(),
        }
    }
    fn provider(&self, role: Role) -> Arc<dyn ModelProvider> {
        SessionConstruction::Headless {
            prepared: &self.prepared,
            gate: &self.gate,
        }
        .headless_provider(self.provider_scope(), Some(&self.meter), role, None)
        .unwrap()
    }
    fn input(&self, original: &ModelRequest) -> u64 {
        input_for_tests(self.scope(Role::Main), original).unwrap()
    }
    fn requests(&self) -> TestSnapshot {
        self.meter.snapshot_for_tests().unwrap()
    }
    fn tokens(&self) -> TestTokenSnapshot {
        self.requests().tokens().unwrap()
    }
    async fn reply(&self, delay: Duration, error: bool) {
        let body = if error {
            "synthetic original failure".to_owned()
        } else {
            format!(
                "data: {}\n\ndata: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
                json!({"choices":[{"delta":{"content":"actual supplied-inner answer"}}]}),
                json!({"choices":[{"delta":{},"finish_reason":"stop"}]}),
                json!({"choices":[],"usage":{"prompt_tokens":20,"completion_tokens":5}})
            )
        };
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(
                ResponseTemplate::new(if error { 400 } else { 200 })
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(body)
                    .set_delay(delay),
            )
            .mount(&self.server)
            .await;
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
    async fn wait_requests(&self, count: usize) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while self.history().await.len() < count {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    }
}
fn request() -> ModelRequest {
    ModelRequest {
        system: "ORIGINAL fixed system".into(),
        messages: vec![Msg::user("ORIGINAL instruction")],
        tools: vec![],
    }
}

#[tokio::test]
async fn default_request_only_builder_has_no_token_reservation_opt_in() {
    let f = Fixture::new(None, 1, json!({}), "synthetic-reservation-key", false).await;
    assert!(!f.meter.tokens_enabled());
    assert!(f.requests().tokens().is_none());
    f.reply(Duration::ZERO, false).await;
    f.provider(Role::Main).complete(request()).await.unwrap();
    assert_eq!(f.requests().admitted(), 1);
    let history = f.history().await;
    assert_eq!(history.len(), 1);
    assert!(history[0].get("max_tokens").is_none());
}
#[tokio::test]
async fn initial_insufficient_input_refuses_without_charge_allocation_or_traffic() {
    let f = Fixture::usual(1, 4096).await;
    let input = f.input(&request());
    assert!(input > 1);
    assert!(f.provider(Role::Main).complete(request()).await.is_err());
    let t = f.tokens();
    assert_eq!(
        (t.maximum(), t.allocated(), t.active(), t.unsettled()),
        (1, 0, 0, 0)
    );
    assert_eq!(t.refusal(), Some((Role::Main, 1, 0, input, 1, 1)));
    assert_eq!(f.requests().admitted(), 0);
    assert!(f.history().await.is_empty());
    assert!(f.gate.execution_closed());
    assert!(!f.gate.approval_required());
}
#[tokio::test]
async fn exact_minimum_fit_clamps_huge_ceiling_before_actual_send_and_keeps_unsettled() {
    let mut f = Fixture::usual(u64::MAX, u64::MAX).await;
    let input = f.input(&request());
    f.rebuild(input + 1, 50).await;
    f.reply(Duration::ZERO, false).await;
    let response = f.provider(Role::Main).complete(request()).await.unwrap();
    assert_eq!(
        (response.usage.input_tokens, response.usage.output_tokens),
        (20, 5)
    );
    let t = f.tokens();
    assert_eq!(t.allocated(), input + 1);
    assert_eq!(t.unsettled(), input + 1);
    assert_eq!(t.active(), 0);
    assert_eq!(t.role(Role::Main), [input, 1, 0, input + 1]);
    assert!(t.refusal().is_none());
    assert_eq!(f.requests().role(Role::Main), [1, 0, 1, 0, 0]);
    assert!(!f.gate.execution_closed());
    let history = f.history().await;
    assert_eq!(history.len(), 1);
    assert_eq!(history[0]["max_tokens"], 1);
    assert_eq!(
        history[0]["messages"][0]["content"],
        "ORIGINAL fixed system"
    );
    assert_ne!(
        t.allocated(),
        25,
        "allocation is NOT measured response usage"
    );
}
#[tokio::test]
async fn reported_small_return_never_releases_or_refunds_a_reservation() {
    let mut f = Fixture::usual(u64::MAX, 100).await;
    let input = f.input(&request());
    f.rebuild(input + 100, 50).await;
    f.reply(Duration::ZERO, false).await;
    let p = f.provider(Role::Main);
    p.complete(request()).await.unwrap();
    assert!(p.complete(request()).await.is_err());
    assert_eq!(f.requests().admitted(), 1);
    assert_eq!(f.history().await.len(), 1);
    assert_eq!(f.tokens().allocated(), input + 100);
    assert_eq!(
        f.tokens().refusal(),
        Some((Role::Main, input + 100, input + 100, input, 1, 0))
    );
}
#[tokio::test]
async fn all_concrete_roles_share_atomic_reservations_and_truthful_rows() {
    let f = Fixture::usual(u64::MAX, 1).await;
    let input = f.input(&request());
    f.reply(Duration::ZERO, false).await;
    for role in [Role::Main, Role::Subagent, Role::Compaction] {
        f.provider(role).complete(request()).await.unwrap();
    }
    let t = f.tokens();
    assert_eq!(f.requests().admitted(), 3);
    assert_eq!(t.allocated(), 3 * (input + 1));
    for role in [Role::Main, Role::Subagent, Role::Compaction] {
        assert_eq!(t.role(role), [input, 1, 0, input + 1]);
        assert_eq!(f.requests().role(role), [1, 0, 1, 0, 0]);
    }
    assert_eq!(f.history().await.len(), 3);
    assert!(t.refusal().is_none());
}
#[tokio::test]
async fn real_error_retains_unsettled_allocation_not_reported_zero_or_a_refund() {
    let f = Fixture::usual(u64::MAX, 1).await;
    let input = f.input(&request());
    f.reply(Duration::ZERO, true).await;
    assert!(
        f.provider(Role::Subagent)
            .complete(request())
            .await
            .is_err()
    );
    assert_eq!(f.requests().role(Role::Subagent), [1, 0, 0, 1, 0]);
    assert_eq!(f.tokens().role(Role::Subagent), [input, 1, 0, input + 1]);
    assert_eq!(f.history().await.len(), 1);
    assert!(f.tokens().refusal().is_none());
}
#[tokio::test]
async fn unpolled_token_future_has_no_request_allocation_or_send() {
    let f = Fixture::usual(u64::MAX, 1).await;
    let p = f.provider(Role::Main);
    let future = p.complete(request());
    drop(future);
    assert_eq!(f.requests().admitted(), 0);
    assert_eq!(f.tokens().allocated(), 0);
    assert!(f.history().await.is_empty());
}
#[tokio::test]
async fn actual_inflight_abort_moves_active_to_unsettled_without_refunding() {
    let f = Fixture::usual(u64::MAX, 1).await;
    let input = f.input(&request());
    f.reply(Duration::from_secs(30), false).await;
    let p = f.provider(Role::Main);
    let join = tokio::spawn(async move { p.complete(request()).await });
    f.wait_requests(1).await;
    assert_eq!(f.tokens().role(Role::Main), [input, 1, input + 1, 0]);
    join.abort();
    assert!(join.await.unwrap_err().is_cancelled());
    assert_eq!(f.requests().role(Role::Main), [1, 0, 0, 0, 1]);
    assert_eq!(f.tokens().role(Role::Main), [input, 1, 0, input + 1]);
    assert_eq!(f.history().await.len(), 1);
}
#[tokio::test]
async fn two_actual_pending_roles_leave_no_room_for_third_without_overbooking() {
    let mut f = Fixture::usual(u64::MAX, 1).await;
    let input = f.input(&request());
    f.rebuild(2 * (input + 1), 50).await;
    f.reply(Duration::from_secs(30), false).await;
    let a = f.provider(Role::Main);
    let a = tokio::spawn(async move { a.complete(request()).await });
    f.wait_requests(1).await;
    let b = f.provider(Role::Subagent);
    let b = tokio::spawn(async move { b.complete(request()).await });
    f.wait_requests(2).await;
    assert_eq!(f.tokens().active(), 2 * (input + 1));
    assert!(
        f.provider(Role::Compaction)
            .complete(request())
            .await
            .is_err()
    );
    assert_eq!(f.requests().admitted(), 2);
    assert_eq!(f.history().await.len(), 2);
    assert_eq!(
        f.tokens().refusal(),
        Some((
            Role::Compaction,
            2 * (input + 1),
            2 * (input + 1),
            input,
            1,
            0
        ))
    );
    a.abort();
    b.abort();
    assert!(a.await.unwrap_err().is_cancelled());
    assert!(b.await.unwrap_err().is_cancelled());
    assert_eq!(f.tokens().active(), 0);
    assert_eq!(f.tokens().unsettled(), 2 * (input + 1));
    assert_eq!(f.requests().role(Role::Main), [1, 0, 0, 0, 1]);
    assert_eq!(f.requests().role(Role::Subagent), [1, 0, 0, 0, 1]);
}
#[tokio::test]
async fn concurrent_all_role_admissions_cannot_exceed_either_atomic_ceiling() {
    let mut f = Fixture::usual(u64::MAX, 1).await;
    let input = f.input(&request());
    f.rebuild(5 * (input + 1), 50).await;
    f.reply(Duration::from_secs(30), false).await;
    let mut joins = Vec::new();
    for n in 0..32 {
        let p = f.provider([Role::Main, Role::Subagent, Role::Compaction][n % 3]);
        joins.push(tokio::spawn(async move { p.complete(request()).await }));
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        while !f.gate.execution_closed() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let before = f.requests();
    assert_eq!(before.admitted(), 5);
    assert_eq!(f.tokens().allocated(), 5 * (input + 1));
    assert_eq!(f.tokens().active(), 5 * (input + 1));
    for j in &joins {
        j.abort();
    }
    for j in joins {
        let _ = j.await;
    }
    let after = f.requests();
    assert_eq!(after.admitted(), 5);
    assert_eq!(f.tokens().active(), 0);
    assert_eq!(f.tokens().unsettled(), 5 * (input + 1));
    let totals = [Role::Main, Role::Subagent, Role::Compaction].map(|r| after.role(r));
    assert_eq!(totals.iter().map(|r| r[4]).sum::<u64>(), 5);
    assert!(
        f.history().await.len() <= 5,
        "logical admission is not fabricated HTTP traffic"
    );
}
#[tokio::test]
async fn repeated_other_role_attempts_preserve_the_first_token_refusal() {
    let f = Fixture::usual(1, 1).await;
    assert!(
        f.provider(Role::Subagent)
            .complete(request())
            .await
            .is_err()
    );
    let original = f.tokens().refusal();
    for role in [Role::Main, Role::Compaction, Role::Main] {
        assert!(crate::exec::meter::tokens::wrap_provider(f.scope(role)).is_err());
    }
    assert_eq!(f.tokens().refusal(), original);
    assert_eq!(f.requests().admitted(), 0);
    assert!(f.history().await.is_empty());
}
#[tokio::test]
async fn request_first_precedence_is_a_real_refusal_not_a_token_count_heuristic() {
    let mut f = Fixture::usual(u64::MAX, 1).await;
    let input = f.input(&request());
    f.rebuild(input + 1, 1).await;
    f.reply(Duration::ZERO, false).await;
    let p = f.provider(Role::Main);
    p.complete(request()).await.unwrap();
    assert!(p.complete(request()).await.is_err());
    assert_eq!(f.requests().refusal(), Some((Role::Main, 1, 1)));
    assert!(f.tokens().refusal().is_none());
    assert_eq!(f.requests().admitted(), 1);
    assert_eq!(f.history().await.len(), 1);
}
#[tokio::test]
async fn typed_approval_before_admission_keeps_true_zero_facts_and_no_false_limit() {
    let f = Fixture::new(
        Some(1),
        50,
        json!({"max_tokens":1}),
        "synthetic-reservation-key",
        true,
    )
    .await;
    let p = f.provider(Role::Main);
    f.gate.observe_error_kind(ToolErrorKind::NeedsApproval);
    assert!(p.complete(request()).await.is_err());
    assert_eq!(f.requests().admitted(), 0);
    assert_eq!(f.tokens().allocated(), 0);
    assert!(f.tokens().refusal().is_none());
    assert!(f.requests().refusal().is_none());
    assert!(f.history().await.is_empty());
}
#[tokio::test]
async fn late_real_approval_supersedes_but_does_not_erase_reservation_refusal() {
    let f = Fixture::new(
        Some(1),
        50,
        json!({"max_tokens":1}),
        "synthetic-reservation-key",
        true,
    )
    .await;
    assert!(f.provider(Role::Main).complete(request()).await.is_err());
    let refused = f.tokens().refusal();
    f.gate.observe_error_kind(ToolErrorKind::NeedsApproval);
    assert!(matches!(
        crate::exec::meter::tokens::wrap_provider(f.scope(Role::Subagent)),
        Err(Fault::ApprovalRequired)
    ));
    assert_eq!(f.tokens().refusal(), refused);
    assert!(f.gate.approval_required());
}
#[tokio::test]
async fn checked_whole_input_counts_system_tools_schema_defaults_and_call_arguments() {
    let f = Fixture::usual(u64::MAX, 1).await;
    let baseline = f.input(&request());
    let mut changed = request();
    changed.system.push_str(&" extra input".repeat(50));
    assert!(f.input(&changed) > baseline);
    let mut changed = request();
    changed.tools.push(ToolDef { name:"original_tool".into(), description:"schema context ".repeat(50), parameters:json!({"type":"object","properties":{"original_field":{"type":"string","description":"source annotation"}}}) });
    assert!(f.input(&changed) > baseline);
    let mut changed = request();
    changed.messages.push(Msg::Assistant {
        content: String::new(),
        tool_calls: vec![craftsman_harness::ToolCall {
            id: "original_call".into(),
            name: "original_tool".into(),
            args: json!({"original_field":"original argument ".repeat(100)}),
        }],
    });
    assert!(f.input(&changed) > baseline);
    let g=Fixture::new(Some(u64::MAX),50,json!({"max_tokens":1,"response_format":{"type":"json_schema","json_schema":{"name":"original_format","schema":{"type":"object","description":"original schema ".repeat(100)}}}}),"synthetic-reservation-key",false).await;
    assert!(g.input(&request()) > baseline);
    assert_eq!(f.requests().admitted(), 0);
    assert_eq!(g.requests().admitted(), 0);
}
#[tokio::test]
async fn output_ceiling_integer_does_not_inflate_the_input_estimate() {
    let a = Fixture::usual(u64::MAX, 1).await;
    let b = Fixture::usual(u64::MAX, u64::MAX).await;
    assert_eq!(a.input(&request()), b.input(&request()));
    assert_eq!(a.requests().admitted(), 0);
    assert_eq!(b.requests().admitted(), 0);
}
#[tokio::test]
async fn whole_overbound_original_request_refuses_before_either_charge_or_traffic() {
    let f = Fixture::usual(u64::MAX, 1).await;
    let mut original = request();
    original.system = "x".repeat(1024 * 1024 + 1);
    assert!(f.provider(Role::Main).complete(original).await.is_err());
    assert_eq!(f.requests().admitted(), 0);
    assert_eq!(f.tokens().allocated(), 0);
    assert!(f.tokens().refusal().is_none());
    assert!(f.history().await.is_empty());
}
#[tokio::test]
async fn invalid_ambiguous_multi_candidate_or_hidden_output_controls_fail_without_charge() {
    for kwargs in [
        json!({"max_tokens":0}),
        json!({"max_tokens":1.5}),
        json!({"max_tokens":"1"}),
        json!({"max_tokens":true}),
        json!({"max_tokens":-1}),
        json!({"max_tokens":1,"max_completion_tokens":1}),
        json!({"max_tokens":1,"n":2}),
        json!({"max_tokens":1,"extra_body":{"max_tokens":1}}),
        json!({"max_tokens":1,"max_output_tokens":1}),
    ] {
        let f = Fixture::new(
            Some(u64::MAX),
            50,
            kwargs,
            "synthetic-reservation-key",
            false,
        )
        .await;
        assert!(f.provider(Role::Main).complete(request()).await.is_err());
        assert_eq!(f.requests().admitted(), 0);
        assert_eq!(f.tokens().allocated(), 0);
        assert!(f.tokens().refusal().is_none());
        assert!(f.history().await.is_empty());
    }
}
#[tokio::test]
async fn alternative_reply_field_and_single_candidate_preserve_original_controls() {
    let f = Fixture::new(
        Some(u64::MAX),
        50,
        json!({"max_completion_tokens":7,"n":1,"temperature":0.25,"top_p":0.8}),
        "synthetic-reservation-key",
        false,
    )
    .await;
    f.reply(Duration::ZERO, false).await;
    f.provider(Role::Compaction)
        .complete(request())
        .await
        .unwrap();
    let history = f.history().await;
    assert_eq!(history.len(), 1);
    let body = &history[0];
    assert_eq!(body["max_completion_tokens"], 7);
    assert!(body.get("max_tokens").is_none());
    assert_eq!(body["n"], 1);
    assert_eq!(body["temperature"], 0.25);
    assert_eq!(body["top_p"], 0.8);
    assert_eq!(f.tokens().role(Role::Compaction)[1], 7);
}
#[tokio::test]
async fn absent_reply_field_uses_private_bounded_ceiling_not_unbounded_traffic() {
    let f = Fixture::new(
        Some(u64::MAX),
        50,
        json!({}),
        "synthetic-reservation-key",
        false,
    )
    .await;
    f.reply(Duration::ZERO, false).await;
    f.provider(Role::Main).complete(request()).await.unwrap();
    assert_eq!(f.history().await[0]["max_tokens"], 4096);
    assert_eq!(f.tokens().role(Role::Main)[1], 4096);
}
#[tokio::test]
async fn new_clamped_integer_collision_fails_before_charging_or_false_token_refusal() {
    let mut f = Fixture::new(
        Some(u64::MAX),
        50,
        json!({"max_tokens":900000}),
        "87654",
        false,
    )
    .await;
    let input = f.input(&request());
    f.rebuild(input + 87654, 50).await;
    assert!(f.provider(Role::Main).complete(request()).await.is_err());
    assert_eq!(f.requests().admitted(), 0);
    assert_eq!(f.tokens().allocated(), 0);
    assert!(f.tokens().refusal().is_none());
    assert!(!f.gate.execution_closed());
    assert!(f.history().await.is_empty());
}
#[tokio::test]
async fn foreign_actual_gate_meter_and_equal_snapshots_are_not_authority_aliases() {
    let a = Fixture::usual(1, 1).await;
    let b = Fixture::usual(1, 1).await;
    let sa = a.requests();
    let sb = b.requests();
    assert!(!sa.same_source(&sb));
    assert!(matches!(
        a.meter.bind_snapshot_for_tests(&sb, &a.gate),
        Err(Fault::Foreign)
    ));
    assert!(!a.tokens().same_source(&b.tokens()));
    let mut scope = a.scope(Role::Main);
    scope.meter = b.meter.clone();
    assert!(matches!(
        crate::exec::meter::tokens::wrap_provider(scope),
        Err(Fault::Foreign)
    ));
    assert_eq!(a.requests().admitted(), 0);
    assert_eq!(b.requests().admitted(), 0);
}
#[tokio::test]
async fn admitted_body_requires_its_exact_actual_client_source_not_equal_options() {
    let f = Fixture::usual(u64::MAX, 1).await;
    let scope = f.scope(Role::Main);
    let original_source = scope.original.request_source_for_tests();
    let admitted = admit_for_tests(scope, &request()).unwrap();
    admitted.ready_for(&original_source).unwrap();
    let other = crate::llm::LlmClient::headless(f.provider_scope()).unwrap();
    assert!(
        admitted
            .ready_for(&other.request_source_for_tests().unwrap())
            .is_err()
    );
    assert!(other.stream_admitted_model_turn(&admitted).await.is_err());
    assert!(f.history().await.is_empty());
    assert_eq!(f.requests().role(Role::Main), [1, 1, 0, 0, 0]);
    drop(admitted);
    assert_eq!(f.requests().role(Role::Main), [1, 0, 0, 0, 1]);
}
struct NeverEntered(AtomicUsize);
#[async_trait]
impl ModelProvider for NeverEntered {
    async fn complete(&self, _: ModelRequest) -> Result<ModelResponse, AgentError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(AgentError::Model("synthetic unexpected entry".into()))
    }
}
#[tokio::test]
async fn request_only_scope_cannot_bypass_a_token_enabled_meter() {
    let f = Fixture::usual(u64::MAX, 1).await;
    let inner = Arc::new(NeverEntered(AtomicUsize::new(0)));
    let scope = SessionConstruction::Headless {
        prepared: &f.prepared,
        gate: &f.gate,
    }
    .request_provider_scope(Some(&f.meter), Role::Main, inner.clone())
    .unwrap();
    assert!(matches!(
        crate::exec::meter::wrap_provider(scope),
        Err(Fault::Foreign)
    ));
    assert_eq!(inner.0.load(Ordering::SeqCst), 0);
    assert_eq!(f.requests().admitted(), 0);
    assert_eq!(f.tokens().allocated(), 0);
}
#[tokio::test]
async fn poisoned_actual_meter_never_returns_free_zero_allowance_or_enters_provider() {
    let f = Fixture::usual(u64::MAX, 1).await;
    let p = f.provider(Role::Main);
    let old = f.tokens();
    f.meter.poison_for_tests();
    assert!(p.complete(request()).await.is_err());
    assert!(f.meter.snapshot_for_tests().is_err());
    assert!(f.gate.execution_closed());
    assert!(f.history().await.is_empty());
    assert_eq!(old.allocated(), 0);
}

#[tokio::test]
async fn actual_deny_only_seal_hides_an_admitted_body_without_fake_token_or_approval_refusal() {
    let f = Fixture::usual(u64::MAX, 1).await;
    let scope = f.scope(Role::Main);
    let source = scope.original.request_source_for_tests();
    let admitted = admit_for_tests(scope, &request()).unwrap();
    admitted.ready_for(&source).unwrap();
    f.gate.close_execution();
    assert!(admitted.ready_for(&source).is_err());
    assert!(!f.gate.approval_required());
    assert!(f.tokens().refusal().is_none());
    assert!(f.requests().refusal().is_none());
    assert_eq!(f.requests().role(Role::Main), [1, 1, 0, 0, 0]);
    drop(admitted);
    assert_eq!(f.requests().role(Role::Main), [1, 0, 0, 0, 1]);
    assert_eq!(f.tokens().active(), 0);
    assert!(f.history().await.is_empty());
}

#[tokio::test]
async fn actual_late_approval_hides_a_permit_without_erasing_real_admission_or_allocation() {
    let f = Fixture::new(
        Some(u64::MAX),
        50,
        json!({"max_tokens": 1}),
        "synthetic-reservation-key",
        true,
    )
    .await;
    let scope = f.scope(Role::Subagent);
    let source = scope.original.request_source_for_tests();
    let admitted = admit_for_tests(scope, &request()).unwrap();
    let before = f.tokens().allocated();
    f.gate.observe_error_kind(ToolErrorKind::NeedsApproval);
    assert!(admitted.ready_for(&source).is_err());
    assert!(f.gate.approval_required());
    assert!(f.tokens().refusal().is_none());
    assert_eq!(f.requests().role(Role::Subagent), [1, 1, 0, 0, 0]);
    drop(admitted);
    assert_eq!(f.requests().role(Role::Subagent), [1, 0, 0, 0, 1]);
    assert_eq!(f.tokens().allocated(), before);
    assert_eq!(f.tokens().unsettled(), before);
    assert!(f.history().await.is_empty());
}
