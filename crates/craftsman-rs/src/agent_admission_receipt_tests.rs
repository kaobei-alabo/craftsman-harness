//! Actual builder/client/admission identity checks, no caller receipt statuses.
use super::*;
use crate::exec::construction::{Options, Prepared, SessionSelection};
use crate::exec::meter::receipts::{Return, SendState as S};
use crate::exec::meter::tokens::{Admitted, observation_admit_for_tests};
use crate::exec::meter::{Meter, RequestLimit, Role, TestObservations, TokenLimit};
use crate::exec::permissions::Selectors;
use crate::exec::runtime::InvocationGate;
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
    async fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("workspace");
        let config = root.path().join("selected-config");
        std::fs::create_dir(&workspace).unwrap();
        std::fs::create_dir(&config).unwrap();
        let server = MockServer::start().await;
        std::fs::write(config.join("config.json"),json!({"model":"synthetic-identity-receipt-model","api_key":"synthetic-identity-receipt-key","base_url":server.uri(),"model_kwargs":{"max_tokens":1}}).to_string()).unwrap();
        let prepared = Prepared::prepare(Options {
            workspace,
            config_dir: config,
            trust_workspace: false,
            session: SessionSelection::Ephemeral,
            selectors: Selectors::default(),
        })
        .await
        .unwrap();
        let (session, gate, meter) = build_prepared_session_with_limits(
            &prepared,
            RequestLimit::default(),
            Some(TokenLimit::new(u64::MAX).unwrap()),
        )
        .await
        .unwrap();
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
    async fn limit_to(&mut self, budget: u64) {
        let (session, gate, meter) = build_prepared_session_with_limits(
            &self.prepared,
            RequestLimit::default(),
            Some(TokenLimit::new(budget).unwrap()),
        )
        .await
        .unwrap();
        self._session = session;
        self.gate = gate;
        self.meter = meter;
    }
    fn admit(&self, role: Role) -> (crate::exec::provider::Provider, Admitted) {
        observation_admit_for_tests(
            TokenProviderScope {
                meter: self.meter.clone(),
                gate: self.gate.clone(),
                role,
                original: crate::exec::provider::Provider::from_scope(self.provider_scope())
                    .unwrap(),
            },
            &request(),
        )
        .unwrap()
    }
    fn observations(&self) -> TestObservations {
        let snapshot = self.meter.snapshot_for_tests().unwrap();
        self.meter
            .bind_snapshot_for_tests(&snapshot, &self.gate)
            .unwrap();
        snapshot.receipts().unwrap()
    }
    async fn response(&self, delay: Duration) {
        Mock::given(method("POST")).and(path("/chat/completions")).respond_with(ResponseTemplate::new(200).insert_header("content-type","text/event-stream").set_body_string("data: {\"choices\":[{\"delta\":{\"content\":\"ORIGINAL result\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n").set_delay(delay)).mount(&self.server).await;
    }
    async fn history_len(&self) -> usize {
        self.server.received_requests().await.unwrap().len()
    }
}
fn request() -> ModelRequest {
    ModelRequest {
        system: "ORIGINAL receipt source system".into(),
        messages: vec![Msg::user("ORIGINAL receipt source instruction")],
        tools: vec![],
    }
}

#[tokio::test]
async fn admitted_but_unpolled_client_future_has_no_prepared_send_or_fake_error() {
    let f = Fixture::new().await;
    let (p, a) = f.admit(Role::Main);
    let future = p
        .observation_client_for_tests()
        .stream_admitted_model_turn(&a);
    drop(future);
    let o = f.observations();
    assert_eq!(o.len(), 1);
    assert_eq!(o.totals(), [0; 4]);
    assert_eq!(
        o.call(0),
        Some((
            1,
            Role::Main,
            None,
            [S::Unprepared, S::Unprepared],
            [None, None]
        ))
    );
    drop(a);
    assert_eq!(
        f.observations().call(0),
        Some((
            1,
            Role::Main,
            Some(Return::Abandoned),
            [S::Unprepared, S::Unprepared],
            [None, None]
        ))
    );
    assert_eq!(f.history_len().await, 0);
}
#[tokio::test]
async fn real_client_preparation_drop_is_never_polled_not_a_send_or_receipt() {
    let f = Fixture::new().await;
    let (p, a) = f.admit(Role::Main);
    p.observation_client_for_tests()
        .prepare_only_for_observation_tests(&a)
        .unwrap();
    assert_eq!(
        f.observations().call(0),
        Some((
            1,
            Role::Main,
            None,
            [S::NeverPolled, S::Unprepared],
            [None, None]
        ))
    );
    assert_eq!(f.observations().totals(), [0; 4]);
    drop(a);
    assert_eq!(f.history_len().await, 0);
}
#[tokio::test]
async fn replaying_prepared_slot_fails_closed_without_polling_or_resetting_facts() {
    let f = Fixture::new().await;
    let (p, a) = f.admit(Role::Main);
    p.observation_client_for_tests()
        .prepare_only_for_observation_tests(&a)
        .unwrap();
    let old = f.observations();
    assert!(
        p.observation_client_for_tests()
            .prepare_only_for_observation_tests(&a)
            .is_err()
    );
    assert!(f.gate.execution_closed());
    assert!(f.meter.snapshot_for_tests().is_err());
    assert_eq!(old.call(0).unwrap().3, [S::NeverPolled, S::Unprepared]);
    assert_eq!(f.history_len().await, 0);
}
#[tokio::test]
async fn replaying_exact_admitted_request_after_real_receipt_never_sends_again() {
    let f = Fixture::new().await;
    f.response(Duration::ZERO).await;
    let (p, a) = f.admit(Role::Main);
    p.observation_client_for_tests()
        .stream_admitted_model_turn(&a)
        .await
        .unwrap();
    let old = f.observations();
    assert_eq!(old.totals(), [1, 1, 0, 0]);
    assert!(
        p.observation_client_for_tests()
            .stream_admitted_model_turn(&a)
            .await
            .is_err()
    );
    assert!(f.gate.execution_closed());
    assert!(f.meter.snapshot_for_tests().is_err());
    assert_eq!(old.call(0).unwrap().4, [Some(200), None]);
    assert_eq!(
        f.history_len().await,
        1,
        "retain actual original request; no reset or duplicate wire send"
    );
}
#[tokio::test]
async fn foreign_actual_client_request_lease_cannot_bind_an_observation_scope() {
    let a = Fixture::new().await;
    let b = Fixture::new().await;
    let (_p, admitted) = a.admit(Role::Main);
    let foreign = crate::llm::LlmClient::headless(b.provider_scope()).unwrap();
    let checked = foreign.prepare_reservation(&request()).unwrap();
    let lease = foreign
        .request_source_for_tests()
        .unwrap()
        .lease(&checked)
        .unwrap();
    assert!(admitted.observation_scope(lease).is_err());
    assert_eq!(a.observations().totals(), [0; 4]);
    assert_eq!(b.observations().len(), 0);
    assert!(!a.gate.execution_closed());
    assert_eq!(a.history_len().await, 0);
    assert_eq!(b.history_len().await, 0);
}
#[tokio::test]
async fn equal_fresh_request_from_same_client_is_not_the_admitted_request_identity() {
    let f = Fixture::new().await;
    let (p, a) = f.admit(Role::Main);
    let client = p.observation_client_for_tests();
    let checked = client.prepare_reservation(&request()).unwrap();
    let lease = client
        .request_source_for_tests()
        .unwrap()
        .lease(&checked)
        .unwrap();
    assert!(a.observation_scope(lease).is_err());
    assert_eq!(f.observations().totals(), [0; 4]);
    assert_eq!(f.history_len().await, 0);
}
#[tokio::test]
async fn separate_same_number_snapshot_sources_are_not_authority_aliases() {
    let a = Fixture::new().await;
    let b = Fixture::new().await;
    let sa = a.observations();
    let sb = b.observations();
    assert_eq!(sa.len(), sb.len());
    assert!(!sa.same_source(&sb));
    let foreign = b.meter.snapshot_for_tests().unwrap();
    assert!(a.meter.bind_snapshot_for_tests(&foreign, &a.gate).is_err());
}
#[tokio::test]
async fn actual_execution_seal_refuses_preparation_without_fabricating_approval_or_poll() {
    let f = Fixture::new().await;
    let (p, a) = f.admit(Role::Main);
    f.gate.close_execution();
    assert!(
        p.observation_client_for_tests()
            .prepare_only_for_observation_tests(&a)
            .is_err()
    );
    assert!(!f.gate.approval_required());
    assert_eq!(
        f.observations().call(0).unwrap().3,
        [S::Unprepared, S::Unprepared]
    );
    assert_eq!(f.history_len().await, 0);
}
#[tokio::test]
async fn distinct_concurrent_admissions_correlate_results_independent_of_completion_order() {
    let f = Fixture::new().await;
    f.response(Duration::from_millis(150)).await;
    let (a, admitted_a) = f.admit(Role::Main);
    let (b, admitted_b) = f.admit(Role::Subagent);
    let (x, y) = tokio::join!(
        a.observation_client_for_tests()
            .stream_admitted_model_turn(&admitted_a),
        b.observation_client_for_tests()
            .stream_admitted_model_turn(&admitted_b)
    );
    x.unwrap();
    y.unwrap();
    let o = f.observations();
    assert_eq!(o.totals(), [2, 2, 0, 0]);
    assert_eq!(
        (o.call(0).unwrap().0, o.call(0).unwrap().1),
        (1, Role::Main)
    );
    assert_eq!(
        (o.call(1).unwrap().0, o.call(1).unwrap().1),
        (2, Role::Subagent)
    );
    drop(admitted_a);
    drop(admitted_b);
    assert_eq!(f.observations().call(0).unwrap().2, Some(Return::Abandoned));
    assert_eq!(f.observations().call(1).unwrap().2, Some(Return::Abandoned));
    assert_eq!(f.history_len().await, 2);
}

#[tokio::test]
async fn real_other_role_token_refusal_preserves_late_actual_header_without_extra_admission() {
    let mut f = Fixture::new().await;
    let estimate =
        crate::exec::meter::tokens::input_for_tests(f.scope(Role::Main), &request()).unwrap();
    f.limit_to(estimate + 1).await;
    f.response(Duration::from_millis(250)).await;
    let construction = SessionConstruction::Headless {
        prepared: &f.prepared,
        gate: &f.gate,
    };
    let main = construction
        .headless_provider(f.provider_scope(), Some(&f.meter), Role::Main, None)
        .unwrap();
    let child = construction
        .headless_provider(f.provider_scope(), Some(&f.meter), Role::Subagent, None)
        .unwrap();
    let task = tokio::spawn(async move { main.complete(request()).await });
    tokio::time::timeout(Duration::from_secs(5), async {
        while f.history_len().await != 1 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert!(child.complete(request()).await.is_err());
    assert!(f.meter.tokens_refused().unwrap());
    assert!(
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    let o = f.observations();
    assert_eq!(o.len(), 1);
    assert_eq!(
        o.call(0),
        Some((
            1,
            Role::Main,
            Some(Return::Error),
            [S::Received, S::Unprepared],
            [Some(200), None]
        ))
    );
    assert_eq!(o.totals(), [1, 1, 0, 0]);
    assert_eq!(f.meter.snapshot_for_tests().unwrap().admitted(), 1);
    assert!(!f.gate.approval_required());
    assert_eq!(f.history_len().await, 1);
}
