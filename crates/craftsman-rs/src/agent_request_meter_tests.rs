//! Synthetic providers with actual shared-builder source/gate/role bindings.
use super::*;
use crate::exec::construction::{Options, Prepared, SessionSelection};
use crate::exec::meter::{Fault, Meter, RequestLimit, Role, TestSnapshot as Snapshot};
use crate::exec::permissions::{OnUnapproved, Selectors};
use crate::exec::runtime::InvocationGate;
use craftsman_harness::ToolErrorKind;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::Notify;

struct Fixture {
    _root: tempfile::TempDir,
    prepared: Prepared,
    _session: Session,
    gate: Arc<InvocationGate>,
    meter: Arc<Meter>,
}
impl Fixture {
    async fn new(maximum: u64, fail: bool) -> Self {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("workspace");
        let config = root.path().join("selected-config");
        std::fs::create_dir(&workspace).unwrap();
        std::fs::create_dir(&config).unwrap();
        std::fs::write(
            config.join("config.json"),
            json!({"model":"identical-synthetic-model", "api_key":"synthetic-meter-key",
                "base_url":"http://127.0.0.1:1"})
            .to_string(),
        )
        .unwrap();
        // Choose the pure validated cap before config/runtime/provider work.
        let limit = RequestLimit::new(maximum).unwrap();
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
        let (session, gate, meter) = build_prepared_session_with_request_limit(&prepared, limit)
            .await
            .unwrap();
        Self {
            _root: root,
            prepared,
            _session: session,
            gate,
            meter,
        }
    }
    fn scope(&self, role: Role, inner: Arc<dyn ModelProvider>) -> RequestProviderScope {
        SessionConstruction::Headless {
            prepared: &self.prepared,
            gate: &self.gate,
        }
        .request_provider_scope(Some(&self.meter), role, inner)
        .unwrap()
    }
    fn provider(&self, role: Role, inner: Arc<dyn ModelProvider>) -> Arc<dyn ModelProvider> {
        crate::exec::meter::wrap_provider(self.scope(role, inner)).unwrap()
    }
    fn snapshot(&self) -> Snapshot {
        let snapshot = self.meter.snapshot_for_tests().unwrap();
        self.meter
            .bind_snapshot_for_tests(&snapshot, &self.gate)
            .unwrap();
        snapshot
    }
}
fn request() -> ModelRequest {
    ModelRequest {
        system: "original fixed system".into(),
        messages: vec![Msg::user("original input")],
        tools: vec![],
    }
}
fn response(text: &str) -> ModelResponse {
    ModelResponse {
        message: Msg::assistant(text),
        usage: Usage {
            input_tokens: 13,
            output_tokens: 2,
        },
    }
}
struct Immediate {
    calls: AtomicUsize,
    fail: bool,
    text: &'static str,
    signal: Option<Arc<InvocationGate>>,
}
impl Immediate {
    fn new(fail: bool) -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
            fail,
            text: "synthetic result",
            signal: None,
        })
    }
}
#[async_trait]
impl ModelProvider for Immediate {
    async fn complete(&self, original: ModelRequest) -> Result<ModelResponse, AgentError> {
        assert_eq!(original.system, "original fixed system");
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(gate) = &self.signal {
            gate.observe_error_kind(ToolErrorKind::NeedsApproval);
        }
        if self.fail {
            Err(AgentError::Model(self.text.into()))
        } else {
            Ok(response(self.text))
        }
    }
}
struct Pending {
    calls: AtomicUsize,
    changed: Notify,
    release: Notify,
}
impl Pending {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
            changed: Notify::new(),
            release: Notify::new(),
        })
    }
    async fn wait_calls(&self, count: usize) {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let changed = self.changed.notified();
                tokio::pin!(changed);
                changed.as_mut().enable();
                if self.calls.load(Ordering::SeqCst) >= count {
                    break;
                }
                changed.await;
            }
        })
        .await
        .unwrap();
    }
}
#[async_trait]
impl ModelProvider for Pending {
    async fn complete(&self, _: ModelRequest) -> Result<ModelResponse, AgentError> {
        // Register actual waiting before the actual entry marker, never a fake readiness signal.
        let released = self.release.notified();
        tokio::pin!(released);
        released.as_mut().enable();
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.changed.notify_waiters();
        released.await;
        Ok(response("actual supplied-inner return"))
    }
}

#[test]
fn default_cap_is_accepted_fifty() {
    assert_eq!(RequestLimit::default().maximum(), 50);
}
#[test]
fn explicit_cap_accepts_one_fifty_and_ten_thousand() {
    for n in [1, 50, 10_000] {
        assert_eq!(RequestLimit::new(n).unwrap().maximum(), n);
    }
}
#[test]
fn explicit_cap_rejects_zero_overbound_and_u64_max_without_repair() {
    for n in [0, 10_001, u64::MAX] {
        assert!(RequestLimit::new(n).is_err());
    }
}

#[tokio::test]
async fn exact_last_response_succeeds_without_a_refusal_heuristic() {
    let f = Fixture::new(1, false).await;
    let inner = Immediate::new(false);
    let p = f.provider(Role::Main, inner.clone());
    assert!(p.complete(request()).await.is_ok());
    let s = f.snapshot();
    assert_eq!(s.admitted(), 1);
    assert_eq!(s.role(Role::Main), [1, 0, 1, 0, 0]);
    assert!(s.refusal().is_none());
    assert!(!f.gate.execution_closed());
    assert!(!f.meter.limit_refused().unwrap());
    assert_eq!(inner.calls.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn all_three_roles_share_one_cap_and_keep_exact_rows() {
    let f = Fixture::new(3, false).await;
    let inner = Immediate::new(false);
    let providers =
        [Role::Main, Role::Subagent, Role::Compaction].map(|role| f.provider(role, inner.clone()));
    for p in &providers {
        assert!(p.complete(request()).await.is_ok());
    }
    let before = f.snapshot();
    assert_eq!(before.admitted(), 3);
    for role in [Role::Main, Role::Subagent, Role::Compaction] {
        assert_eq!(before.role(role), [1, 0, 1, 0, 0]);
    }
    assert!(providers[1].complete(request()).await.is_err());
    let after = f.snapshot();
    assert_eq!(after.admitted(), 3);
    assert_eq!(after.refusal(), Some((Role::Subagent, 3, 3)));
    assert_eq!(inner.calls.load(Ordering::SeqCst), 3);
    assert!(f.gate.execution_closed());
    assert!(!f.gate.approval_required());
}
#[tokio::test]
async fn ten_thousand_actual_calls_hit_the_ceiling_without_wrap() {
    let f = Fixture::new(10_000, false).await;
    let inner = Immediate::new(false);
    let p = f.provider(Role::Compaction, inner.clone());
    for _ in 0..10_000 {
        assert!(p.complete(request()).await.is_ok());
    }
    assert!(p.complete(request()).await.is_err());
    let s = f.snapshot();
    assert_eq!(s.maximum(), 10_000);
    assert_eq!(s.role(Role::Compaction), [10_000, 0, 10_000, 0, 0]);
    assert_eq!(s.refusal(), Some((Role::Compaction, 10_000, 10_000)));
    assert_eq!(inner.calls.load(Ordering::SeqCst), 10_000);
}
#[tokio::test]
async fn repeated_rejections_preserve_first_role_and_do_not_enter_inner() {
    let f = Fixture::new(1, false).await;
    let inner = Immediate::new(false);
    let main = f.provider(Role::Main, inner.clone());
    let child = f.provider(Role::Subagent, inner.clone());
    let compact = f.provider(Role::Compaction, inner.clone());
    assert!(main.complete(request()).await.is_ok());
    assert!(compact.complete(request()).await.is_err());
    for _ in 0..8 {
        assert!(child.complete(request()).await.is_err());
    }
    let s = f.snapshot();
    assert_eq!(s.admitted(), 1);
    assert_eq!(s.refusal(), Some((Role::Compaction, 1, 1)));
    assert_eq!(s.role(Role::Subagent), [0; 5]);
    assert_eq!(inner.calls.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn actual_inner_error_is_not_refunded_or_classified_as_usage() {
    let f = Fixture::new(1, false).await;
    let inner = Immediate::new(true);
    let p = f.provider(Role::Main, inner.clone());
    assert!(p.complete(request()).await.is_err());
    let s = f.snapshot();
    assert_eq!(s.role(Role::Main), [1, 0, 0, 1, 0]);
    assert!(s.refusal().is_none());
    assert!(p.complete(request()).await.is_err());
    assert_eq!(f.snapshot().refusal(), Some((Role::Main, 1, 1)));
    assert_eq!(inner.calls.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn diagnostic_words_and_success_text_cannot_latch_authority() {
    let f = Fixture::new(2, true).await;
    let inner = Arc::new(Immediate {
        calls: AtomicUsize::new(0),
        fail: true,
        text: "limit max_tokens NeedsApproval approval required",
        signal: None,
    });
    assert!(
        f.provider(Role::Main, inner.clone())
            .complete(request())
            .await
            .is_err()
    );
    let success = Arc::new(Immediate {
        calls: AtomicUsize::new(0),
        fail: false,
        text: "limit NeedsApproval",
        signal: None,
    });
    assert!(
        f.provider(Role::Subagent, success.clone())
            .complete(request())
            .await
            .is_ok()
    );
    assert!(!f.gate.approval_required());
    assert!(!f.gate.execution_closed());
    assert!(f.snapshot().refusal().is_none());
    assert_eq!(f.snapshot().admitted(), 2);
}
#[tokio::test]
async fn unpolled_future_does_not_manufacture_an_admission() {
    let f = Fixture::new(1, false).await;
    let inner = Immediate::new(false);
    let p = f.provider(Role::Main, inner.clone());
    drop(p.complete(request()));
    assert_eq!(f.snapshot().admitted(), 0);
    assert_eq!(inner.calls.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn abandoned_polled_future_releases_active_but_never_refunds_calls() {
    let f = Fixture::new(2, false).await;
    let inner = Pending::new();
    let p = f.provider(Role::Subagent, inner.clone());
    let join = tokio::spawn(async move { p.complete(request()).await });
    inner.wait_calls(1).await;
    assert_eq!(f.snapshot().role(Role::Subagent), [1, 1, 0, 0, 0]);
    join.abort();
    assert!(join.await.unwrap_err().is_cancelled());
    assert_eq!(f.snapshot().role(Role::Subagent), [1, 0, 0, 0, 1]);
    let next = Immediate::new(false);
    assert!(
        f.provider(Role::Main, next.clone())
            .complete(request())
            .await
            .is_ok()
    );
    assert_eq!(f.snapshot().admitted(), 2);
    assert_eq!(inner.calls.load(Ordering::SeqCst), 1);
    assert_eq!(next.calls.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn late_refusal_keeps_actual_inner_return_but_blocks_final_success() {
    let f = Fixture::new(1, false).await;
    let inner = Pending::new();
    let pending = f.provider(Role::Main, inner.clone());
    let other_inner = Immediate::new(false);
    let other = f.provider(Role::Compaction, other_inner.clone());
    let join = tokio::spawn(async move { pending.complete(request()).await });
    inner.wait_calls(1).await;
    assert!(other.complete(request()).await.is_err());
    assert_eq!(f.snapshot().role(Role::Main), [1, 1, 0, 0, 0]);
    inner.release.notify_waiters();
    assert!(join.await.unwrap().is_err());
    let s = f.snapshot();
    assert_eq!(s.role(Role::Main), [1, 0, 1, 0, 0]);
    assert_eq!(s.refusal(), Some((Role::Compaction, 1, 1)));
    assert_eq!(other_inner.calls.load(Ordering::SeqCst), 0);
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_all_role_calls_are_linearized_at_one_global_ceiling() {
    let f = Fixture::new(5, false).await;
    let inner = Pending::new();
    let providers =
        [Role::Main, Role::Subagent, Role::Compaction].map(|role| f.provider(role, inner.clone()));
    let mut joins = Vec::new();
    for n in 0..16 {
        let p = providers[n % 3].clone();
        joins.push(tokio::spawn(async move { p.complete(request()).await }));
    }
    inner.wait_calls(5).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while !f.meter.limit_refused().unwrap() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let active = f.snapshot();
    assert_eq!(active.admitted(), 5);
    assert_eq!(
        [Role::Main, Role::Subagent, Role::Compaction]
            .into_iter()
            .map(|r| active.role(r)[1])
            .sum::<u64>(),
        5
    );
    inner.release.notify_waiters();
    for join in joins {
        assert!(join.await.unwrap().is_err());
    }
    let done = f.snapshot();
    assert_eq!(done.admitted(), 5);
    assert_eq!(inner.calls.load(Ordering::SeqCst), 5);
    assert_eq!(
        [Role::Main, Role::Subagent, Role::Compaction]
            .into_iter()
            .map(|r| done.role(r)[2])
            .sum::<u64>(),
        5
    );
    assert_eq!(
        [Role::Main, Role::Subagent, Role::Compaction]
            .into_iter()
            .map(|r| done.role(r)[1])
            .sum::<u64>(),
        0
    );
}
#[tokio::test]
async fn already_closed_execution_does_not_invent_a_cap_refusal() {
    let f = Fixture::new(1, false).await;
    let inner = Immediate::new(false);
    let p = f.provider(Role::Main, inner.clone());
    f.gate.close_execution();
    assert!(p.complete(request()).await.is_err());
    assert_eq!(f.snapshot().admitted(), 0);
    assert!(f.snapshot().refusal().is_none());
    assert_eq!(inner.calls.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn actual_approval_before_admission_keeps_zero_counts() {
    let f = Fixture::new(1, true).await;
    let inner = Immediate::new(false);
    let p = f.provider(Role::Main, inner.clone());
    f.gate.observe_error_kind(ToolErrorKind::NeedsApproval);
    assert!(p.complete(request()).await.is_err());
    assert!(f.gate.approval_required());
    assert_eq!(f.snapshot().admitted(), 0);
    assert!(f.snapshot().refusal().is_none());
    assert_eq!(inner.calls.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn deny_policy_typed_unapproved_is_not_a_false_stop() {
    let f = Fixture::new(1, false).await;
    let inner = Immediate::new(false);
    let p = f.provider(Role::Main, inner.clone());
    f.gate.observe_error_kind(ToolErrorKind::NeedsApproval);
    assert!(p.complete(request()).await.is_ok());
    assert!(!f.gate.approval_required());
    assert!(f.snapshot().refusal().is_none());
}
#[tokio::test]
async fn late_true_approval_dominates_and_preserves_real_inner_observation() {
    let f = Fixture::new(1, true).await;
    let inner = Arc::new(Immediate {
        calls: AtomicUsize::new(0),
        fail: false,
        text: "actual response",
        signal: Some(f.gate.clone()),
    });
    let p = f.provider(Role::Main, inner.clone());
    assert!(p.complete(request()).await.is_err());
    assert_eq!(f.snapshot().role(Role::Main), [1, 0, 1, 0, 0]);
    assert!(f.snapshot().refusal().is_none());
    assert!(matches!(
        crate::exec::meter::wrap_provider(f.scope(Role::Compaction, inner)),
        Err(Fault::ApprovalRequired)
    ));
}
#[tokio::test]
async fn true_approval_after_real_refusal_does_not_erase_limit_facts() {
    let f = Fixture::new(1, true).await;
    let inner = Immediate::new(false);
    let p = f.provider(Role::Main, inner.clone());
    assert!(p.complete(request()).await.is_ok());
    assert!(p.complete(request()).await.is_err());
    f.gate.observe_error_kind(ToolErrorKind::NeedsApproval);
    assert!(matches!(
        crate::exec::meter::wrap_provider(f.scope(Role::Subagent, inner)),
        Err(Fault::ApprovalRequired)
    ));
    assert_eq!(f.snapshot().refusal(), Some((Role::Main, 1, 1)));
    assert_eq!(f.snapshot().admitted(), 1);
}
#[tokio::test]
async fn snapshots_are_immutable_point_in_time_not_current_or_final_receipts() {
    let f = Fixture::new(2, false).await;
    let old = f.snapshot();
    let inner = Immediate::new(false);
    assert!(
        f.provider(Role::Main, inner)
            .complete(request())
            .await
            .is_ok()
    );
    let new = f.snapshot();
    assert_eq!(old.admitted(), 0);
    assert_eq!(new.admitted(), 1);
    assert!(old.same_source(&new));
    f.meter.bind_snapshot_for_tests(&old, &f.gate).unwrap();
}
#[tokio::test]
async fn equal_fresh_meters_and_snapshots_are_not_foreign_authority_aliases() {
    let a = Fixture::new(2, false).await;
    let b = Fixture::new(2, false).await;
    let sa = a.snapshot();
    let sb = b.snapshot();
    assert_eq!(sa.admitted(), sb.admitted());
    assert!(!sa.same_source(&sb));
    assert!(matches!(
        a.meter.bind_snapshot_for_tests(&sb, &a.gate),
        Err(Fault::Foreign)
    ));
    assert!(matches!(
        b.meter.bind_snapshot_for_tests(&sa, &b.gate),
        Err(Fault::Foreign)
    ));
}
#[tokio::test]
async fn separately_captured_equal_permission_gate_is_not_the_actual_source() {
    let f = Fixture::new(1, false).await;
    let other = Arc::new(InvocationGate::captured(&f.prepared));
    assert!(!f.meter.matches_gate(&other));
    assert!(matches!(
        f.meter.bind_snapshot_for_tests(&f.snapshot(), &other),
        Err(Fault::Foreign)
    ));
}
#[tokio::test]
async fn a_foreign_meter_gate_binding_refuses_before_inner_entry() {
    let a = Fixture::new(1, false).await;
    let b = Fixture::new(1, false).await;
    let inner = Immediate::new(false);
    let scope = SessionConstruction::Headless {
        prepared: &a.prepared,
        gate: &a.gate,
    }
    .request_provider_scope(Some(&b.meter), Role::Main, inner.clone())
    .unwrap();
    assert!(matches!(
        crate::exec::meter::wrap_provider(scope),
        Err(Fault::Foreign)
    ));
    assert_eq!(inner.calls.load(Ordering::SeqCst), 0);
    assert_eq!(a.snapshot().admitted(), 0);
    assert_eq!(b.snapshot().admitted(), 0);
}
#[tokio::test]
async fn missing_headless_meter_has_no_independently_excludable_fallback() {
    let f = Fixture::new(1, false).await;
    let result = build_session_with_profile(
        SessionConstruction::Headless {
            prepared: &f.prepared,
            gate: &f.gate,
        },
        Arc::new(Mutex::new(ApprovalState::default())),
        "",
        true,
        false,
        None,
        SessionExtras::default(),
        None,
    )
    .await;
    assert!(result.is_none());
    assert_eq!(f.snapshot().admitted(), 0);
}
#[tokio::test]
async fn frontend_cannot_select_a_private_meter_through_core_arguments() {
    let f = Fixture::new(1, false).await;
    let result = build_session_with_profile(
        SessionConstruction::Frontend(f.prepared.config()),
        Arc::new(Mutex::new(ApprovalState::default())),
        "",
        true,
        false,
        None,
        SessionExtras::default(),
        Some(f.meter.clone()),
    )
    .await;
    assert!(result.is_none());
    assert_eq!(f.snapshot().admitted(), 0);
}
#[tokio::test]
async fn foreign_headless_meter_is_rejected_before_core_effects() {
    let a = Fixture::new(1, false).await;
    let b = Fixture::new(1, false).await;
    let result = build_session_with_profile(
        SessionConstruction::Headless {
            prepared: &a.prepared,
            gate: &a.gate,
        },
        Arc::new(Mutex::new(ApprovalState::default())),
        "",
        true,
        false,
        None,
        SessionExtras::default(),
        Some(b.meter.clone()),
    )
    .await;
    assert!(result.is_none());
    assert_eq!(a.snapshot().admitted(), 0);
    assert_eq!(b.snapshot().admitted(), 0);
}
#[tokio::test]
async fn poisoned_actual_accounting_fails_closed_never_zero_or_reset() {
    let f = Fixture::new(2, false).await;
    let inner = Immediate::new(false);
    let p = f.provider(Role::Main, inner.clone());
    assert!(p.complete(request()).await.is_ok());
    let before = f.snapshot();
    f.meter.poison_for_tests();
    assert!(matches!(
        f.meter.snapshot_for_tests(),
        Err(Fault::Accounting)
    ));
    assert!(p.complete(request()).await.is_err());
    assert!(f.gate.execution_closed());
    assert!(!f.gate.approval_required());
    assert_eq!(before.admitted(), 1);
    assert_eq!(inner.calls.load(Ordering::SeqCst), 1);
    assert!(matches!(f.meter.limit_refused(), Err(Fault::Accounting)));
}
#[tokio::test]
async fn new_role_wrapper_does_not_reset_the_invocation_counter() {
    let f = Fixture::new(2, false).await;
    let inner = Immediate::new(false);
    assert!(
        f.provider(Role::Main, inner.clone())
            .complete(request())
            .await
            .is_ok()
    );
    assert!(
        f.provider(Role::Compaction, inner.clone())
            .complete(request())
            .await
            .is_ok()
    );
    assert!(
        f.provider(Role::Subagent, inner.clone())
            .complete(request())
            .await
            .is_err()
    );
    assert_eq!(f.snapshot().admitted(), 2);
    assert_eq!(inner.calls.load(Ordering::SeqCst), 2);
}
