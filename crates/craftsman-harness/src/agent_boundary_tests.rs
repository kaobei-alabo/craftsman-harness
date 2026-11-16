//! Actual boundary ownership, committed input, saves and sampling regressions.
use super::*;
use crate::builtin::{CompactionMiddleware, CompactionTrigger};
use crate::testing::{MemCheckpointer, ScriptedModel};
use crate::{AgentBoundaryPolicy, HumanInputKind, ModelBoundaryAction, ModelResponse, Usage};
use std::sync::Condvar;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::Notify;

const WAIT: Duration = Duration::from_secs(2);
const END_MARKER: &str = "boundary_fixture_end_count";
const INPUT_MARKER: &str = "boundary_fixture_last_observed_input";
const STOP_REASON: &str = "Actual local boundary policy stopped sampling.";
const INPUT_ERROR: &str = "Actual appended-input observer failed.";
const BOUNDARY_ERROR: &str = "Actual boundary observer failed.";
const COMPACTION_ERROR: &str = "Actual idle-compaction observer failed.";

fn human_messages(state: &AgentState) -> Vec<String> {
    state
        .messages
        .iter()
        .filter_map(|message| match message {
            Msg::User { content } => Some(content.clone()),
            _ => None,
        })
        .collect()
}

#[derive(Default)]
struct Observations {
    inputs: Mutex<Vec<(HumanInputKind, usize, String)>>,
    input_states: Mutex<Vec<Vec<String>>>,
    input_tool_results: Mutex<Vec<Vec<String>>>,
    boundaries: Mutex<Vec<Vec<String>>>,
    runtime: AtomicUsize,
    receipts: AtomicUsize,
    compactions: AtomicUsize,
}

struct SyncGate {
    entered: Notify,
    released: Mutex<bool>,
    release: Condvar,
}

impl SyncGate {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            entered: Notify::new(),
            released: Mutex::new(false),
            release: Condvar::new(),
        })
    }
    fn block(&self) {
        self.entered.notify_one();
        let mut released = self.released.lock().unwrap();
        while !*released {
            released = self.release.wait(released).unwrap();
        }
    }
    fn open(&self) {
        *self.released.lock().unwrap() = true;
        self.release.notify_all();
    }
}

struct AsyncGate {
    entered: Notify,
    release: Notify,
}

impl AsyncGate {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            entered: Notify::new(),
            release: Notify::new(),
        })
    }
}

enum ReleaseOnDrop {
    Sync(Arc<SyncGate>),
    Async(Arc<AsyncGate>),
}

impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        match self {
            Self::Sync(gate) => gate.open(),
            Self::Async(gate) => gate.release.notify_one(),
        }
    }
}

enum Decision {
    Continue,
    Stop,
    StopAfter {
        effects: Arc<AtomicUsize>,
        count: usize,
    },
    Error,
}

struct ProbePolicy {
    seen: Arc<Observations>,
    decision: Decision,
    input_error: Option<HumanInputKind>,
    first_boundary_gate: Option<Arc<SyncGate>>,
    stop_gate: Option<Arc<SyncGate>>,
    store: Option<Arc<MemCheckpointer>>,
    compaction_error: bool,
}

impl ProbePolicy {
    fn new(seen: Arc<Observations>, decision: Decision) -> Self {
        Self {
            seen,
            decision,
            input_error: None,
            first_boundary_gate: None,
            stop_gate: None,
            store: None,
            compaction_error: false,
        }
    }
}

impl AgentBoundaryPolicy for ProbePolicy {
    fn user_input(
        &self,
        state: &mut AgentState,
        message_index: usize,
        kind: HumanInputKind,
    ) -> Result<(), AgentError> {
        let Some(Msg::User { content }) = state.messages.get(message_index) else {
            panic!("Observer must receive the exact already appended human index");
        };
        let original = content.clone();
        self.seen
            .inputs
            .lock()
            .unwrap()
            .push((kind, message_index, original.clone()));
        self.seen
            .input_states
            .lock()
            .unwrap()
            .push(human_messages(state));
        self.seen.input_tool_results.lock().unwrap().push(
            state
                .messages
                .iter()
                .filter_map(|message| match message {
                    Msg::Tool { tool_call_id, .. } => Some(tool_call_id.clone()),
                    _ => None,
                })
                .collect(),
        );
        state
            .custom
            .insert(INPUT_MARKER.into(), serde_json::json!(original));
        if self.input_error == Some(kind) {
            Err(AgentError::State(INPUT_ERROR.into()))
        } else {
            Ok(())
        }
    }

    fn before_model(&self, state: &AgentState) -> Result<ModelBoundaryAction, AgentError> {
        let first = {
            let mut snapshots = self.seen.boundaries.lock().unwrap();
            let first = snapshots.is_empty();
            snapshots.push(human_messages(state));
            first
        };
        if first {
            if let Some(gate) = &self.first_boundary_gate {
                gate.block();
            }
        }
        match &self.decision {
            Decision::Continue => Ok(ModelBoundaryAction::Continue),
            Decision::Stop => Ok(ModelBoundaryAction::Stop {
                reason: STOP_REASON.into(),
            }),
            Decision::StopAfter { effects, count } if effects.load(Ordering::SeqCst) >= *count => {
                Ok(ModelBoundaryAction::Stop {
                    reason: STOP_REASON.into(),
                })
            }
            Decision::StopAfter { .. } => Ok(ModelBoundaryAction::Continue),
            Decision::Error => Err(AgentError::State(BOUNDARY_ERROR.into())),
        }
    }

    fn runtime_input(&self) {
        self.seen.runtime.fetch_add(1, Ordering::SeqCst);
    }

    fn idle_compaction_committed(&self, state: &mut AgentState) -> Result<(), AgentError> {
        self.seen.compactions.fetch_add(1, Ordering::SeqCst);
        assert!(state.messages.iter().any(|message| match message {
            Msg::User { content } | Msg::Assistant { content, .. } =>
                content.contains("actual replacement summary"),
            _ => false,
        }));
        state.custom.insert(
            "actual_compaction_observer_entered".into(),
            serde_json::json!(true),
        );
        if self.compaction_error {
            Err(AgentError::State(COMPACTION_ERROR.into()))
        } else {
            Ok(())
        }
    }

    fn stop_committed(&self, state: &AgentState) {
        assert_eq!(state.custom[END_MARKER], 1);
        if let Some(store) = &self.store {
            let saves = store.saves.lock().unwrap();
            let saved = &saves.last().unwrap().2;
            assert_eq!(saved.messages, state.messages);
            assert_eq!(saved.files, state.files);
            assert_eq!(saved.custom[END_MARKER], 1);
        }
        self.seen.receipts.fetch_add(1, Ordering::SeqCst);
        if let Some(gate) = &self.stop_gate {
            gate.block();
        }
    }
}

struct EndHook(Arc<AtomicUsize>);

#[async_trait::async_trait]
impl AgentMiddleware for EndHook {
    fn name(&self) -> &str {
        "boundary-end-fixture"
    }
    async fn turn_end(&self, state: &mut AgentState) -> Result<(), AgentError> {
        let count = self.0.fetch_add(1, Ordering::SeqCst) + 1;
        state
            .custom
            .insert(END_MARKER.into(), serde_json::json!(count));
        Ok(())
    }
}

struct Effects(Arc<AtomicUsize>);

struct WaitingEffect {
    calls: Arc<AtomicUsize>,
    gate: Arc<AsyncGate>,
}

#[async_trait::async_trait]
impl Tool for WaitingEffect {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "boundary_effect".into(),
            description: "owned effectful cancellation fixture".into(),
            parameters: serde_json::json!({"type":"object"}),
        }
    }
    async fn call(
        &self,
        _: serde_json::Value,
        ctx: &mut ToolCtx<'_>,
    ) -> Result<ToolOutcome, AgentError> {
        assert_eq!(
            self.calls.fetch_add(1, Ordering::SeqCst),
            0,
            "The committed sibling must never dispatch after cancellation"
        );
        ctx.backend
            .write(
                "actual-cancellation-effect.txt",
                "actual retained effect before cancellation",
            )
            .await?;
        self.gate.entered.notify_one();
        self.gate.release.notified().await;
        Ok(ToolOutcome::ok(
            "Actual effectful operation returned after owned cancellation.",
        ))
    }
}

#[async_trait::async_trait]
impl Tool for Effects {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "boundary_effect".into(),
            description: "actual boundary fixture effects".into(),
            parameters: serde_json::json!({"type":"object"}),
        }
    }
    async fn call(
        &self,
        _: serde_json::Value,
        ctx: &mut ToolCtx<'_>,
    ) -> Result<ToolOutcome, AgentError> {
        let number = self.0.fetch_add(1, Ordering::SeqCst) + 1;
        ctx.backend
            .write(
                &format!("actual-{number}.txt"),
                &format!("actual effect {number}"),
            )
            .await?;
        Ok(ToolOutcome::err(format!(
            "Actual returned error after effect {number}."
        )))
    }
}

fn effect_group() -> Msg {
    Msg::Assistant {
        content: "actual sampled tool group".into(),
        tool_calls: (1..=2)
            .map(|number| ToolCall {
                id: format!("actual-call-{number}"),
                name: "boundary_effect".into(),
                args: serde_json::json!({"number":number}),
            })
            .collect(),
    }
}

struct WaitingModel {
    requests: Mutex<Vec<ModelRequest>>,
    gate: Arc<AsyncGate>,
}

#[async_trait::async_trait]
impl ModelProvider for WaitingModel {
    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, AgentError> {
        let first = {
            let mut requests = self.requests.lock().unwrap();
            requests.push(request);
            requests.len() == 1
        };
        if !first {
            return Err(AgentError::Model(
                "Unexpected additional boundary fixture sample.".into(),
            ));
        }
        self.gate.entered.notify_one();
        self.gate.release.notified().await;
        Ok(ModelResponse {
            message: Msg::assistant("actual candidate answer"),
            usage: Usage::default(),
        })
    }
}

struct FinalStore {
    saved: Arc<MemCheckpointer>,
    attempts: Arc<AtomicUsize>,
    fail: bool,
    gate: Option<Arc<AsyncGate>>,
}

#[async_trait::async_trait]
impl Checkpointer for FinalStore {
    async fn save(
        &self,
        thread: &str,
        seq: u64,
        state: &AgentState,
        suspension: Option<&Suspension>,
    ) -> Result<(), AgentError> {
        if state
            .custom
            .get(END_MARKER)
            .and_then(serde_json::Value::as_u64)
            == Some(1)
        {
            self.attempts.fetch_add(1, Ordering::SeqCst);
            if let Some(gate) = &self.gate {
                gate.entered.notify_one();
                gate.release.notified().await;
            }
            if self.fail {
                return Err(AgentError::State(
                    "Configured final fixture save failed.".into(),
                ));
            }
        }
        self.saved.save(thread, seq, state, suspension).await
    }
    async fn load_latest(
        &self,
        thread: &str,
    ) -> Result<Option<(u64, AgentState, Option<Suspension>)>, AgentError> {
        self.saved.load_latest(thread).await
    }
    async fn list_threads(&self) -> Result<Vec<crate::ThreadInfo>, AgentError> {
        Ok(Vec::new())
    }
    async fn delete_thread(&self, _: &str) -> Result<(), AgentError> {
        Ok(())
    }
}

async fn completed(
    handle: tokio::task::JoinHandle<Result<TurnResult, AgentError>>,
) -> Result<TurnResult, AgentError> {
    tokio::time::timeout(WAIT, handle)
        .await
        .expect("Boundary fixture did not finish within its bound")
        .expect("Boundary fixture task panicked")
}

#[tokio::test]
async fn boundary_stop_waits_for_the_actual_group_and_final_save_without_another_sample() {
    for durable in [false, true] {
        let effects = Arc::new(AtomicUsize::new(0));
        let ends = Arc::new(AtomicUsize::new(0));
        let store = Arc::new(MemCheckpointer::default());
        let seen = Arc::new(Observations::default());
        let mut policy = ProbePolicy::new(
            seen.clone(),
            Decision::StopAfter {
                effects: effects.clone(),
                count: 2,
            },
        );
        policy.store = Some(store.clone());
        let model = Arc::new(ScriptedModel::new(vec![
            effect_group(),
            Msg::assistant("must remain unused"),
        ]));
        let agent = Agent::builder(model.clone())
            .boundary_policy(Arc::new(policy))
            .middleware(Arc::new(EndHook(ends.clone())))
            .tool(Arc::new(Effects(effects.clone())))
            .checkpointer(store.clone())
            .durable_turn_checkpoints(durable)
            .build();
        assert!(
            matches!(agent.run_turn("perform the actual work").await, Err(AgentError::State(reason)) if reason == STOP_REASON)
        );
        assert_eq!(model.requests.lock().unwrap().len(), 1);
        assert_eq!(effects.load(Ordering::SeqCst), 2);
        assert_eq!(ends.load(Ordering::SeqCst), 1);
        assert_eq!(seen.receipts.load(Ordering::SeqCst), 1);
        assert!(!agent.is_turn_active());
        let saves = store.saves.lock().unwrap();
        assert_eq!(
            saves
                .iter()
                .filter(|save| save
                    .2
                    .custom
                    .get(END_MARKER)
                    .and_then(serde_json::Value::as_u64)
                    == Some(1))
                .count(),
            1
        );
        let saved = &saves.last().unwrap().2;
        assert_eq!(saved.files["actual-1.txt"].content, "actual effect 1");
        assert_eq!(saved.files["actual-2.txt"].content, "actual effect 2");
        let results: Vec<_> = saved
            .messages
            .iter()
            .filter_map(|message| match message {
                Msg::Tool {
                    content, is_error, ..
                } => Some((content.as_str(), *is_error)),
                _ => None,
            })
            .collect();
        assert_eq!(
            results,
            vec![
                ("Actual returned error after effect 1.", true),
                ("Actual returned error after effect 2.", true)
            ]
        );
        assert!(
            !saved
                .messages
                .contains(&Msg::assistant("Stopped by the user."))
        );
        if durable {
            assert_eq!(saved.custom[RECOVERY_KEY]["in_progress"], false);
        }
    }
}

#[tokio::test]
async fn initial_input_callback_failure_saves_the_actual_input_in_both_checkpoint_modes() {
    for durable in [false, true] {
        let seen = Arc::new(Observations::default());
        let mut policy = ProbePolicy::new(seen.clone(), Decision::Continue);
        policy.input_error = Some(HumanInputKind::Initial);
        let model = Arc::new(ScriptedModel::new(vec![Msg::assistant("unused")]));
        let store = Arc::new(MemCheckpointer::default());
        let ends = Arc::new(AtomicUsize::new(0));
        let agent = Agent::builder(model.clone())
            .boundary_policy(Arc::new(policy))
            .checkpointer(store.clone())
            .durable_turn_checkpoints(durable)
            .middleware(Arc::new(EndHook(ends.clone())))
            .build();
        assert!(
            matches!(agent.run_turn("actual initial intent").await, Err(AgentError::State(reason)) if reason == INPUT_ERROR)
        );
        assert!(model.requests.lock().unwrap().is_empty());
        assert_eq!(
            seen.inputs.lock().unwrap().as_slice(),
            &[(
                HumanInputKind::Initial,
                0,
                "actual initial intent".to_owned()
            )]
        );
        assert_eq!(seen.receipts.load(Ordering::SeqCst), 0);
        assert_eq!(ends.load(Ordering::SeqCst), 0);
        let saves = store.saves.lock().unwrap();
        assert_eq!(saves.len(), 1);
        assert_eq!(human_messages(&saves[0].2), vec!["actual initial intent"]);
        assert_eq!(saves[0].2.custom[INPUT_MARKER], "actual initial intent");
        if durable {
            assert_eq!(saves[0].2.custom[RECOVERY_KEY]["in_progress"], true);
        }
        assert!(!agent.is_turn_active());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn first_drained_steering_callback_failure_keeps_the_entire_accepted_batch() {
    for durable in [false, true] {
        let gate = AsyncGate::new();
        let _release = ReleaseOnDrop::Async(gate.clone());
        let seen = Arc::new(Observations::default());
        let mut policy = ProbePolicy::new(seen.clone(), Decision::Continue);
        policy.input_error = Some(HumanInputKind::Steering);
        let model = Arc::new(WaitingModel {
            requests: Mutex::new(Vec::new()),
            gate: gate.clone(),
        });
        let store = Arc::new(MemCheckpointer::default());
        let ends = Arc::new(AtomicUsize::new(0));
        let agent = Agent::builder(model.clone())
            .boundary_policy(Arc::new(policy))
            .checkpointer(store.clone())
            .durable_turn_checkpoints(durable)
            .middleware(Arc::new(EndHook(ends.clone())))
            .build();
        let runner = agent.clone();
        let handle = tokio::spawn(async move { runner.run_turn("initial").await });
        tokio::time::timeout(WAIT, gate.entered.notified())
            .await
            .unwrap();
        for input in ["steer one", "steer two", "steer three"] {
            assert!(agent.steer(input));
        }
        gate.release.notify_one();
        assert!(
            matches!(completed(handle).await, Err(AgentError::State(reason)) if reason == INPUT_ERROR)
        );
        let expected = vec!["initial", "steer one", "steer two", "steer three"];
        assert_eq!(human_messages(&agent.snapshot()), expected);
        let saves = store.saves.lock().unwrap();
        assert_eq!(human_messages(&saves.last().unwrap().2), expected);
        assert_eq!(saves.last().unwrap().2.custom[INPUT_MARKER], "steer one");
        let inputs = seen.inputs.lock().unwrap();
        assert_eq!(inputs.len(), 2);
        assert_eq!(inputs[1].0, HumanInputKind::Steering);
        assert_eq!(inputs[1].2, "steer one");
        assert_eq!(seen.input_states.lock().unwrap()[1], expected);
        assert_eq!(model.requests.lock().unwrap().len(), 1);
        assert_eq!(ends.load(Ordering::SeqCst), 0);
        assert_eq!(seen.receipts.load(Ordering::SeqCst), 0);
        if durable {
            assert_eq!(
                saves.last().unwrap().2.custom[RECOVERY_KEY]["in_progress"],
                true
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn before_model_error_retains_and_checkpoints_steering_raced_with_the_callback() {
    for durable in [false, true] {
        let gate = SyncGate::new();
        let _release = ReleaseOnDrop::Sync(gate.clone());
        let seen = Arc::new(Observations::default());
        let mut policy = ProbePolicy::new(seen.clone(), Decision::Error);
        policy.first_boundary_gate = Some(gate.clone());
        let model = Arc::new(ScriptedModel::new(vec![Msg::assistant("unused")]));
        let store = Arc::new(MemCheckpointer::default());
        let agent = Agent::builder(model.clone())
            .boundary_policy(Arc::new(policy))
            .checkpointer(store.clone())
            .durable_turn_checkpoints(durable)
            .build();
        let runner = agent.clone();
        let handle = tokio::spawn(async move { runner.run_turn("initial").await });
        tokio::time::timeout(WAIT, gate.entered.notified())
            .await
            .unwrap();
        assert!(agent.steer("raced one"));
        assert!(agent.steer("raced two"));
        gate.open();
        assert!(
            matches!(completed(handle).await, Err(AgentError::State(reason)) if reason == BOUNDARY_ERROR)
        );
        let expected = vec!["initial", "raced one", "raced two"];
        assert_eq!(human_messages(&agent.snapshot()), expected);
        assert_eq!(
            human_messages(&store.saves.lock().unwrap().last().unwrap().2),
            expected
        );
        assert!(model.requests.lock().unwrap().is_empty());
        assert_eq!(seen.inputs.lock().unwrap().len(), 1);
        assert_eq!(seen.receipts.load(Ordering::SeqCst), 0);
        assert!(!agent.is_turn_active());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn persistent_stop_raced_with_input_is_re_evaluated_before_any_model_admission() {
    for durable in [false, true] {
        let gate = SyncGate::new();
        let _release = ReleaseOnDrop::Sync(gate.clone());
        let seen = Arc::new(Observations::default());
        let mut policy = ProbePolicy::new(seen.clone(), Decision::Stop);
        policy.first_boundary_gate = Some(gate.clone());
        let model = Arc::new(ScriptedModel::new(vec![Msg::assistant(
            "must not be sampled",
        )]));
        let store = Arc::new(MemCheckpointer::default());
        let ends = Arc::new(AtomicUsize::new(0));
        let agent = Agent::builder(model.clone())
            .boundary_policy(Arc::new(policy))
            .checkpointer(store.clone())
            .durable_turn_checkpoints(durable)
            .middleware(Arc::new(EndHook(ends.clone())))
            .build();
        let runner = agent.clone();
        let handle = tokio::spawn(async move { runner.run_turn("initial").await });
        tokio::time::timeout(WAIT, gate.entered.notified())
            .await
            .unwrap();
        assert!(agent.steer("raced direction"));
        gate.open();
        assert!(
            matches!(completed(handle).await, Err(AgentError::State(reason)) if reason == STOP_REASON)
        );
        let boundaries = seen.boundaries.lock().unwrap();
        assert_eq!(
            boundaries.as_slice(),
            &[
                vec!["initial".to_owned()],
                vec!["initial".to_owned(), "raced direction".to_owned()]
            ]
        );
        assert!(model.requests.lock().unwrap().is_empty());
        assert_eq!(seen.inputs.lock().unwrap().len(), 2);
        assert_eq!(seen.receipts.load(Ordering::SeqCst), 1);
        assert_eq!(ends.load(Ordering::SeqCst), 1);
        assert_eq!(
            human_messages(&store.saves.lock().unwrap().last().unwrap().2),
            vec!["initial", "raced direction"]
        );
    }
}

#[tokio::test]
async fn configured_final_save_failure_prevents_a_stop_receipt_in_both_modes() {
    for durable in [false, true] {
        let saved = Arc::new(MemCheckpointer::default());
        let attempts = Arc::new(AtomicUsize::new(0));
        let seen = Arc::new(Observations::default());
        let ends = Arc::new(AtomicUsize::new(0));
        let model = Arc::new(ScriptedModel::new(vec![Msg::assistant("unused")]));
        let agent = Agent::builder(model.clone())
            .boundary_policy(Arc::new(ProbePolicy::new(seen.clone(), Decision::Stop)))
            .checkpointer(Arc::new(FinalStore {
                saved,
                attempts: attempts.clone(),
                fail: true,
                gate: None,
            }))
            .durable_turn_checkpoints(durable)
            .middleware(Arc::new(EndHook(ends.clone())))
            .build();
        assert_eq!(
            agent
                .run_turn("retained original")
                .await
                .unwrap_err()
                .to_string(),
            checkpoint_save_error().to_string()
        );
        assert!(agent.checkpoint_failed());
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        assert_eq!(ends.load(Ordering::SeqCst), 1);
        assert_eq!(seen.receipts.load(Ordering::SeqCst), 0);
        assert!(model.requests.lock().unwrap().is_empty());
        assert_eq!(human_messages(&agent.snapshot()), vec!["retained original"]);
        assert!(!agent.is_turn_active());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn accepted_cancellation_during_final_save_cannot_create_a_stop_receipt() {
    for durable in [false, true] {
        let gate = AsyncGate::new();
        let _release = ReleaseOnDrop::Async(gate.clone());
        let saved = Arc::new(MemCheckpointer::default());
        let attempts = Arc::new(AtomicUsize::new(0));
        let seen = Arc::new(Observations::default());
        let ends = Arc::new(AtomicUsize::new(0));
        let model = Arc::new(ScriptedModel::new(vec![Msg::assistant("unused")]));
        let agent = Agent::builder(model.clone())
            .boundary_policy(Arc::new(ProbePolicy::new(seen.clone(), Decision::Stop)))
            .checkpointer(Arc::new(FinalStore {
                saved: saved.clone(),
                attempts: attempts.clone(),
                fail: false,
                gate: Some(gate.clone()),
            }))
            .durable_turn_checkpoints(durable)
            .cancellation_session(crate::SessionCancellation::new())
            .middleware(Arc::new(EndHook(ends.clone())))
            .build();
        let runner = agent.clone();
        let handle = tokio::spawn(async move { runner.run_turn("actual retained input").await });
        tokio::time::timeout(WAIT, gate.entered.notified())
            .await
            .unwrap();
        assert!(agent.is_turn_active());
        assert!(agent.request_cancel());
        gate.release.notify_one();
        assert!(matches!(
            completed(handle).await,
            Err(AgentError::Cancelled)
        ));
        assert_eq!(seen.receipts.load(Ordering::SeqCst), 0);
        assert_eq!(ends.load(Ordering::SeqCst), 1);
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        assert_eq!(
            human_messages(&saved.saves.lock().unwrap().last().unwrap().2),
            vec!["actual retained input"]
        );
        assert!(
            !agent
                .snapshot()
                .messages
                .contains(&Msg::assistant("Stopped by the user."))
        );
        assert!(model.requests.lock().unwrap().is_empty());
        assert!(!agent.is_turn_active());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn stop_observer_retains_activity_and_rejects_late_cancel_and_overlapping_turns() {
    for durable in [false, true] {
        let gate = SyncGate::new();
        let _release = ReleaseOnDrop::Sync(gate.clone());
        let seen = Arc::new(Observations::default());
        let mut policy = ProbePolicy::new(seen.clone(), Decision::Stop);
        policy.stop_gate = Some(gate.clone());
        let store = Arc::new(MemCheckpointer::default());
        policy.store = Some(store.clone());
        let model = Arc::new(ScriptedModel::new(vec![Msg::assistant("unused")]));
        let ends = Arc::new(AtomicUsize::new(0));
        let agent = Agent::builder(model.clone())
            .boundary_policy(Arc::new(policy))
            .checkpointer(store)
            .durable_turn_checkpoints(durable)
            .middleware(Arc::new(EndHook(ends.clone())))
            .build();
        let runner = agent.clone();
        let handle = tokio::spawn(async move { runner.run_turn("initial").await });
        tokio::time::timeout(WAIT, gate.entered.notified())
            .await
            .unwrap();
        assert_eq!(seen.receipts.load(Ordering::SeqCst), 1);
        assert!(agent.is_turn_active());
        assert!(matches!(
            agent.run_turn("must be rejected").await,
            Err(AgentError::State(_))
        ));
        assert!(!agent.steer("must be queued by the frontend"));
        assert!(!agent.request_cancel());
        gate.open();
        assert!(
            matches!(completed(handle).await, Err(AgentError::State(reason)) if reason == STOP_REASON)
        );
        assert!(!agent.is_turn_active());
        assert_eq!(ends.load(Ordering::SeqCst), 1);
        assert_eq!(human_messages(&agent.snapshot()), vec!["initial"]);
        assert!(model.requests.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn idle_compaction_observer_error_saves_the_real_replacement_without_turn_hooks() {
    let seen = Arc::new(Observations::default());
    let mut policy = ProbePolicy::new(seen.clone(), Decision::Continue);
    policy.compaction_error = true;
    let store = Arc::new(MemCheckpointer::default());
    let main = Arc::new(ScriptedModel::new(vec![Msg::assistant(
        "unused main sample",
    )]));
    let summary = Arc::new(ScriptedModel::new(vec![Msg::assistant(
        "actual replacement summary",
    )]));
    let ends = Arc::new(AtomicUsize::new(0));
    let agent = Agent::builder(main.clone())
        .boundary_policy(Arc::new(policy))
        .checkpointer(store.clone())
        .durable_turn_checkpoints(true)
        .middleware(Arc::new(EndHook(ends.clone())))
        .build();
    {
        let mut state = agent.state.lock().unwrap();
        state.messages = (0..5)
            .flat_map(|number| {
                [
                    Msg::user(format!("original {number}")),
                    Msg::assistant(format!("actual answer {number}")),
                ]
            })
            .collect();
        state.files.insert(
            "retained.txt".into(),
            crate::FileData::text("actual retained effect"),
        );
    }
    let original = agent.snapshot();
    let compaction = CompactionMiddleware::new(
        summary.clone(),
        CompactionTrigger::MessageCount {
            threshold_msgs: 100,
            keep_tail_msgs: 20,
        },
    );
    assert!(
        matches!(agent.compact_idle(&compaction).await, Err(AgentError::State(reason)) if reason == COMPACTION_ERROR)
    );
    let current = agent.snapshot();
    assert_ne!(current.messages, original.messages);
    assert_eq!(current.files, original.files);
    assert_eq!(seen.compactions.load(Ordering::SeqCst), 1);
    assert_eq!(seen.receipts.load(Ordering::SeqCst), 0);
    assert!(seen.inputs.lock().unwrap().is_empty());
    assert!(seen.boundaries.lock().unwrap().is_empty());
    assert_eq!(ends.load(Ordering::SeqCst), 0);
    assert!(main.requests.lock().unwrap().is_empty());
    assert_eq!(summary.requests.lock().unwrap().len(), 1);
    let saves = store.saves.lock().unwrap();
    assert_eq!(saves.len(), 2);
    assert_eq!(saves[0].2.messages, original.messages);
    assert_eq!(saves[1].2.messages, current.messages);
    assert_eq!(
        saves[1].2.custom["actual_compaction_observer_entered"],
        true
    );
    assert_eq!(saves[1].2.custom[RECOVERY_KEY]["in_progress"], false);
    assert!(!agent.is_turn_active());
}

#[tokio::test]
async fn selected_notifications_and_inbox_reset_runtime_pressure_without_human_authority() {
    for durable in [false, true] {
        let queue = crate::subagent::NotificationQueue::default();
        let seen = Arc::new(Observations::default());
        let model = Arc::new(ScriptedModel::new(vec![Msg::assistant("unused")]));
        let store = Arc::new(MemCheckpointer::default());
        let ends = Arc::new(AtomicUsize::new(0));
        let agent = Agent::builder(model.clone())
            .name("main")
            .notifications(queue.clone())
            .boundary_policy(Arc::new(ProbePolicy::new(seen.clone(), Decision::Stop)))
            .checkpointer(store.clone())
            .durable_turn_checkpoints(durable)
            .middleware(Arc::new(EndHook(ends)))
            .build();
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        sender.send("actual runtime inbox".into()).unwrap();
        agent.reset_task_plumbing(Some(receiver));
        queue.push(crate::subagent::Notification {
            from: "peer".into(),
            to: "main".into(),
            kind: crate::subagent::NotificationKind::Message("actual runtime notification".into()),
        });
        queue.push(crate::subagent::Notification {
            from: "peer".into(),
            to: "unrelated".into(),
            kind: crate::subagent::NotificationKind::Message("unrelated notification".into()),
        });
        assert!(
            matches!(agent.run_turn("actual human request").await, Err(AgentError::State(reason)) if reason == STOP_REASON)
        );
        assert_eq!(seen.runtime.load(Ordering::SeqCst), 2);
        let inputs = seen.inputs.lock().unwrap();
        assert_eq!(inputs.len(), 1);
        assert_eq!(inputs[0].0, HumanInputKind::Initial);
        assert_eq!(inputs[0].2, "actual human request");
        let snapshots = seen.boundaries.lock().unwrap();
        assert_eq!(snapshots.len(), 1);
        assert!(
            snapshots[0]
                .iter()
                .any(|input| input.contains("actual runtime notification"))
        );
        assert!(
            snapshots[0]
                .iter()
                .any(|input| input == "actual runtime inbox")
        );
        assert!(
            !snapshots[0]
                .iter()
                .any(|input| input.contains("unrelated notification"))
        );
        assert_eq!(
            store.saves.lock().unwrap().last().unwrap().2.custom[INPUT_MARKER],
            "actual human request"
        );
        assert!(model.requests.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn absent_boundary_policy_retains_normal_completion_and_legacy_save_failure_behavior() {
    let model = Arc::new(ScriptedModel::new(vec![Msg::assistant(
        "actual legacy completion",
    )]));
    let saved = Arc::new(MemCheckpointer::default());
    let attempts = Arc::new(AtomicUsize::new(0));
    let ends = Arc::new(AtomicUsize::new(0));
    let agent = Agent::builder(model.clone())
        .checkpointer(Arc::new(FinalStore {
            saved: saved.clone(),
            attempts: attempts.clone(),
            fail: true,
            gate: None,
        }))
        .middleware(Arc::new(EndHook(ends.clone())))
        .build();
    let TurnResult::Done(output) = agent.run_turn("ordinary legacy request").await.unwrap() else {
        panic!("Unexpected suspension");
    };
    assert_eq!(output.final_text(), "actual legacy completion");
    assert_eq!(model.requests.lock().unwrap().len(), 1);
    assert_eq!(ends.load(Ordering::SeqCst), 1);
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    assert!(agent.checkpoint_failed());
    assert_eq!(saved.saves.lock().unwrap().len(), 1);
    assert!(agent.snapshot().custom.get(INPUT_MARKER).is_none());
    assert_eq!(
        human_messages(&agent.snapshot()),
        vec!["ordinary legacy request"]
    );
    assert!(!agent.is_turn_active());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn inbox_raced_with_a_stop_is_committed_before_re_evaluation_without_human_promotion() {
    for durable in [false, true] {
        let gate = SyncGate::new();
        let _release = ReleaseOnDrop::Sync(gate.clone());
        let seen = Arc::new(Observations::default());
        let mut policy = ProbePolicy::new(seen.clone(), Decision::Stop);
        policy.first_boundary_gate = Some(gate.clone());
        let model = Arc::new(ScriptedModel::new(vec![Msg::assistant("must stay unused")]));
        let store = Arc::new(MemCheckpointer::default());
        let ends = Arc::new(AtomicUsize::new(0));
        let agent = Agent::builder(model.clone())
            .boundary_policy(Arc::new(policy))
            .checkpointer(store.clone())
            .durable_turn_checkpoints(durable)
            .middleware(Arc::new(EndHook(ends.clone())))
            .build();
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        agent.reset_task_plumbing(Some(receiver));
        let runner = agent.clone();
        let handle = tokio::spawn(async move { runner.run_turn("actual human initial").await });
        tokio::time::timeout(WAIT, gate.entered.notified())
            .await
            .unwrap();
        sender
            .send("runtime arrived during the stop decision".into())
            .unwrap();
        gate.open();
        assert!(
            matches!(completed(handle).await, Err(AgentError::State(reason)) if reason == STOP_REASON)
        );
        let snapshots = seen.boundaries.lock().unwrap();
        assert_eq!(snapshots.len(), 2);
        assert_eq!(snapshots[0], vec!["actual human initial"]);
        assert_eq!(
            snapshots[1],
            vec![
                "actual human initial",
                "runtime arrived during the stop decision"
            ]
        );
        assert_eq!(seen.runtime.load(Ordering::SeqCst), 1);
        let inputs = seen.inputs.lock().unwrap();
        assert_eq!(inputs.len(), 1);
        assert_eq!(inputs[0].0, HumanInputKind::Initial);
        assert_eq!(inputs[0].2, "actual human initial");
        assert_eq!(seen.receipts.load(Ordering::SeqCst), 1);
        assert_eq!(ends.load(Ordering::SeqCst), 1);
        let saved = store.saves.lock().unwrap();
        assert_eq!(human_messages(&saved.last().unwrap().2), snapshots[1]);
        assert_eq!(
            saved.last().unwrap().2.custom[INPUT_MARKER],
            "actual human initial"
        );
        assert!(model.requests.lock().unwrap().is_empty());
        assert!(!agent.is_turn_active());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn selected_sampling_cancellation_retains_all_accepted_steering_and_callback_failures() {
    for selected in [false, true] {
        for durable in [false, true] {
            for observer_failure in [false, true] {
                if !selected && observer_failure {
                    continue;
                }
                let gate = AsyncGate::new();
                let _release = ReleaseOnDrop::Async(gate.clone());
                let model = Arc::new(WaitingModel {
                    requests: Mutex::new(Vec::new()),
                    gate: gate.clone(),
                });
                let saved = Arc::new(MemCheckpointer::default());
                let seen = Arc::new(Observations::default());
                let ends = Arc::new(AtomicUsize::new(0));
                let mut builder = Agent::builder(model.clone())
                    .checkpointer(saved.clone())
                    .durable_turn_checkpoints(durable)
                    .interrupt_model_on_cancel(true)
                    .cancellation_session(crate::SessionCancellation::new())
                    .middleware(Arc::new(EndHook(ends.clone())));
                if selected {
                    let mut policy = ProbePolicy::new(seen.clone(), Decision::Continue);
                    if observer_failure {
                        policy.input_error = Some(HumanInputKind::Steering);
                    }
                    builder = builder.boundary_policy(Arc::new(policy));
                }
                let agent = builder.build();
                let runner = agent.clone();
                let handle =
                    tokio::spawn(async move { runner.run_turn("initial cancellation task").await });
                tokio::time::timeout(WAIT, gate.entered.notified())
                    .await
                    .unwrap();
                assert!(agent.steer("accepted first direction"));
                assert!(agent.steer("accepted second direction"));
                assert!(agent.request_cancel());
                if selected {
                    assert!(!agent.steer("late direction must be queued separately"));
                }
                let result = completed(handle).await;
                let expected = if selected {
                    vec![
                        "initial cancellation task".to_owned(),
                        "accepted first direction".to_owned(),
                        "accepted second direction".to_owned(),
                    ]
                } else {
                    vec!["initial cancellation task".to_owned()]
                };
                assert_eq!(human_messages(&agent.snapshot()), expected);
                let saves = saved.saves.lock().unwrap();
                let stored = &saves.last().unwrap().2;
                assert_eq!(human_messages(stored), expected);
                assert_eq!(model.requests.lock().unwrap().len(), 1);
                assert_eq!(seen.receipts.load(Ordering::SeqCst), 0);
                assert!(!agent.is_turn_active());
                if observer_failure {
                    assert!(
                        matches!(result, Err(AgentError::State(reason)) if reason == INPUT_ERROR)
                    );
                    assert_eq!(ends.load(Ordering::SeqCst), 0);
                    assert_eq!(seen.inputs.lock().unwrap().len(), 2);
                    assert_eq!(seen.input_states.lock().unwrap()[1], expected);
                    assert!(
                        !stored
                            .messages
                            .contains(&Msg::assistant("Stopped by the user."))
                    );
                    if durable {
                        assert_eq!(stored.custom[RECOVERY_KEY]["in_progress"], true);
                    }
                } else {
                    let TurnResult::Done(output) = result.unwrap() else {
                        panic!("Unexpected cancellation suspension");
                    };
                    assert_eq!(output.final_text(), "Stopped by the user.");
                    assert_eq!(ends.load(Ordering::SeqCst), 1);
                    assert_eq!(
                        stored
                            .messages
                            .iter()
                            .filter(|message| **message == Msg::assistant("Stopped by the user."))
                            .count(),
                        1
                    );
                    assert_eq!(
                        saves
                            .iter()
                            .filter(|save| save
                                .2
                                .custom
                                .get(END_MARKER)
                                .and_then(serde_json::Value::as_u64)
                                == Some(1))
                            .count(),
                        1
                    );
                    if selected {
                        assert_eq!(seen.inputs.lock().unwrap().len(), 3);
                        assert_eq!(stored.custom[INPUT_MARKER], "accepted second direction");
                    } else {
                        assert!(stored.custom.get(INPUT_MARKER).is_none());
                    }
                    if durable {
                        assert_eq!(stored.custom[RECOVERY_KEY]["in_progress"], false);
                    }
                }
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn selected_effectful_and_final_round_cancellation_keeps_input_after_real_tool_results() {
    for selected in [false, true] {
        for durable in [false, true] {
            for group_size in [1, 2] {
                let gate = AsyncGate::new();
                let _release = ReleaseOnDrop::Async(gate.clone());
                let calls = Arc::new(AtomicUsize::new(0));
                let saved = Arc::new(MemCheckpointer::default());
                let seen = Arc::new(Observations::default());
                let ends = Arc::new(AtomicUsize::new(0));
                let mut group = effect_group();
                if let Msg::Assistant { tool_calls, .. } = &mut group {
                    tool_calls.truncate(group_size);
                }
                let model = Arc::new(ScriptedModel::new(vec![
                    group,
                    Msg::assistant("must remain unused"),
                ]));
                let mut builder = Agent::builder(model.clone())
                    .checkpointer(saved.clone())
                    .durable_turn_checkpoints(durable)
                    .interrupt_model_on_cancel(true)
                    .cancellation_session(crate::SessionCancellation::new())
                    .max_rounds(1)
                    .finish_on_round_limit(true)
                    .tool(Arc::new(WaitingEffect {
                        calls: calls.clone(),
                        gate: gate.clone(),
                    }))
                    .middleware(Arc::new(EndHook(ends.clone())));
                if selected {
                    builder = builder.boundary_policy(Arc::new(ProbePolicy::new(
                        seen.clone(),
                        Decision::Continue,
                    )));
                }
                let agent = builder.build();
                let runner = agent.clone();
                let handle =
                    tokio::spawn(async move { runner.run_turn("initial effectful task").await });
                tokio::time::timeout(WAIT, gate.entered.notified())
                    .await
                    .unwrap();
                assert!(agent.steer("accepted effectful direction one"));
                assert!(agent.steer("accepted effectful direction two"));
                assert!(agent.request_cancel());
                if selected {
                    assert!(!agent.steer("late effectful direction"));
                }
                assert!(
                    agent.is_turn_active(),
                    "Actual tool ownership must survive the stop request"
                );
                gate.release.notify_one();
                let TurnResult::Done(output) = completed(handle).await.unwrap() else {
                    panic!("Unexpected cancellation suspension");
                };
                assert_eq!(output.final_text(), "Stopped by the user.");
                assert_eq!(calls.load(Ordering::SeqCst), 1);
                assert_eq!(model.requests.lock().unwrap().len(), 1);
                assert_eq!(ends.load(Ordering::SeqCst), 1);
                assert_eq!(seen.receipts.load(Ordering::SeqCst), 0);
                let saves = saved.saves.lock().unwrap();
                let stored = &saves.last().unwrap().2;
                assert_eq!(
                    stored.files["actual-cancellation-effect.txt"].content,
                    "actual retained effect before cancellation"
                );
                let tools: Vec<_> = stored
                    .messages
                    .iter()
                    .filter_map(|message| match message {
                        Msg::Tool {
                            tool_call_id,
                            content,
                            is_error,
                        } => Some((tool_call_id.clone(), content.clone(), *is_error)),
                        _ => None,
                    })
                    .collect();
                assert_eq!(tools.len(), group_size);
                assert_eq!(
                    tools[0],
                    (
                        "actual-call-1".to_owned(),
                        "Actual effectful operation returned after owned cancellation.".to_owned(),
                        false
                    )
                );
                if group_size == 2 {
                    assert_eq!(
                        tools[1],
                        (
                            "actual-call-2".to_owned(),
                            "Tool call cancelled by the user.".to_owned(),
                            true
                        )
                    );
                }
                let expected = if selected {
                    vec![
                        "initial effectful task".to_owned(),
                        "accepted effectful direction one".to_owned(),
                        "accepted effectful direction two".to_owned(),
                    ]
                } else {
                    vec!["initial effectful task".to_owned()]
                };
                assert_eq!(human_messages(stored), expected);
                if selected {
                    assert_eq!(seen.inputs.lock().unwrap().len(), 3);
                    let expected_ids: Vec<_> = (1..=group_size)
                        .map(|number| format!("actual-call-{number}"))
                        .collect();
                    assert_eq!(seen.input_tool_results.lock().unwrap()[1], expected_ids);
                    let last_tool = stored
                        .messages
                        .iter()
                        .rposition(|message| matches!(message, Msg::Tool { .. }))
                        .unwrap();
                    let first_steer = stored
                        .messages
                        .iter()
                        .position(|message| {
                            *message == Msg::user("accepted effectful direction one")
                        })
                        .unwrap();
                    assert!(
                        last_tool < first_steer,
                        "Accepted steering must follow the balanced committed tool group"
                    );
                    assert_eq!(
                        stored.custom[INPUT_MARKER],
                        "accepted effectful direction two"
                    );
                } else {
                    assert!(stored.custom.get(INPUT_MARKER).is_none());
                }
                assert_eq!(
                    saves
                        .iter()
                        .filter(|save| save
                            .2
                            .custom
                            .get(END_MARKER)
                            .and_then(serde_json::Value::as_u64)
                            == Some(1))
                        .count(),
                    1
                );
                if durable {
                    assert_eq!(stored.custom[RECOVERY_KEY]["in_progress"], false);
                }
                assert!(!agent.is_turn_active());
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn selected_cancellation_reports_its_actual_final_save_failure_in_both_modes() {
    for selected in [false, true] {
        for durable in [false, true] {
            let gate = AsyncGate::new();
            let _release = ReleaseOnDrop::Async(gate.clone());
            let model = Arc::new(WaitingModel {
                requests: Mutex::new(Vec::new()),
                gate: gate.clone(),
            });
            let saved = Arc::new(MemCheckpointer::default());
            let attempts = Arc::new(AtomicUsize::new(0));
            let ends = Arc::new(AtomicUsize::new(0));
            let seen = Arc::new(Observations::default());
            let mut builder = Agent::builder(model.clone())
                .checkpointer(Arc::new(FinalStore {
                    saved: saved.clone(),
                    attempts: attempts.clone(),
                    fail: true,
                    gate: None,
                }))
                .durable_turn_checkpoints(durable)
                .interrupt_model_on_cancel(true)
                .cancellation_session(crate::SessionCancellation::new())
                .middleware(Arc::new(EndHook(ends.clone())));
            if selected {
                builder = builder
                    .boundary_policy(Arc::new(ProbePolicy::new(seen.clone(), Decision::Continue)));
            }
            let agent = builder.build();
            let runner = agent.clone();
            let handle =
                tokio::spawn(
                    async move { runner.run_turn("actual initial save-failure task").await },
                );
            tokio::time::timeout(WAIT, gate.entered.notified())
                .await
                .unwrap();
            assert!(agent.steer("accepted save-failure direction one"));
            assert!(agent.steer("accepted save-failure direction two"));
            assert!(agent.request_cancel());
            let result = completed(handle).await;
            if selected || durable {
                assert_eq!(
                    result.unwrap_err().to_string(),
                    checkpoint_save_error().to_string()
                );
            } else {
                let TurnResult::Done(output) = result.unwrap() else {
                    panic!("Unexpected legacy cancellation suspension");
                };
                assert_eq!(output.final_text(), "Stopped by the user.");
            }
            assert!(agent.checkpoint_failed());
            assert_eq!(ends.load(Ordering::SeqCst), 1);
            assert_eq!(attempts.load(Ordering::SeqCst), 1);
            assert_eq!(seen.receipts.load(Ordering::SeqCst), 0);
            assert_eq!(model.requests.lock().unwrap().len(), 1);
            let expected = if selected {
                vec![
                    "actual initial save-failure task".to_owned(),
                    "accepted save-failure direction one".to_owned(),
                    "accepted save-failure direction two".to_owned(),
                ]
            } else {
                vec!["actual initial save-failure task".to_owned()]
            };
            assert_eq!(human_messages(&agent.snapshot()), expected);
            if selected {
                assert_eq!(seen.inputs.lock().unwrap().len(), 3);
                assert_eq!(
                    agent.snapshot().custom[INPUT_MARKER],
                    "accepted save-failure direction two"
                );
            } else {
                assert!(agent.snapshot().custom.get(INPUT_MARKER).is_none());
            }
            let successful = saved.saves.lock().unwrap();
            assert!(
                successful.iter().all(|save| save
                    .2
                    .custom
                    .get(END_MARKER)
                    .and_then(serde_json::Value::as_u64)
                    != Some(1)),
                "The rejected final save must not appear as a successful checkpoint"
            );
            assert!(!agent.is_turn_active());
        }
    }
}
