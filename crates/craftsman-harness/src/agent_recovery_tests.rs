use super::*;
use crate::testing::{MemCheckpointer, ScriptedModel};
use std::sync::atomic::{AtomicUsize, Ordering};

fn calls(ids: &[&str]) -> Msg {
    Msg::Assistant {
        content: String::new(),
        tool_calls: ids
            .iter()
            .map(|id| ToolCall {
                id: (*id).into(),
                name: "effect".into(),
                args: serde_json::json!({}),
            })
            .collect(),
    }
}

struct Effect(Arc<AtomicUsize>);
#[async_trait::async_trait]
impl Tool for Effect {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "effect".into(),
            description: "test effect".into(),
            parameters: serde_json::json!({"type":"object"}),
        }
    }
    async fn call(
        &self,
        _: serde_json::Value,
        ctx: &mut ToolCtx<'_>,
    ) -> Result<ToolOutcome, AgentError> {
        let n = self.0.fetch_add(1, Ordering::SeqCst) + 1;
        ctx.state
            .custom
            .insert("effects".into(), serde_json::json!(n));
        Ok(ToolOutcome::ok(format!("effect {n}")))
    }
}

struct FailingStore {
    saved: MemCheckpointer,
    fail: fn(&AgentState) -> bool,
}
#[async_trait::async_trait]
impl Checkpointer for FailingStore {
    async fn save(
        &self,
        thread: &str,
        seq: u64,
        state: &AgentState,
        suspension: Option<&Suspension>,
    ) -> Result<(), AgentError> {
        if (self.fail)(state) {
            return Err(AgentError::State("synthetic-secret\u{1b}[2J".into()));
        }
        self.saved.save(thread, seq, state, suspension).await
    }
    async fn load_latest(
        &self,
        thread: &str,
    ) -> Result<Option<(u64, AgentState, Option<Suspension>)>, AgentError> {
        self.saved.load_latest(thread).await
    }
    async fn list_threads(&self) -> Result<Vec<crate::checkpoint::ThreadInfo>, AgentError> {
        Ok(vec![])
    }
    async fn delete_thread(&self, _: &str) -> Result<(), AgentError> {
        Ok(())
    }
}

#[tokio::test]
async fn durable_saves_user_intent_each_tool_result_and_final_completion() {
    let cp = Arc::new(MemCheckpointer::default());
    let model = Arc::new(ScriptedModel::new(vec![
        calls(&["a", "b"]),
        Msg::assistant("done"),
    ]));
    let effects = Arc::new(AtomicUsize::new(0));
    let agent = Agent::builder(model)
        .checkpointer(cp.clone())
        .durable_turn_checkpoints(true)
        .tool(Arc::new(Effect(effects.clone())))
        .build();
    agent.run_turn("build").await.unwrap();
    let saves = cp.saves.lock().unwrap();
    assert_eq!(saves.first().unwrap().2.messages, vec![Msg::user("build")]);
    let intent = saves
        .iter()
        .position(|s| s.2.messages.last() == Some(&calls(&["a", "b"])))
        .unwrap();
    let first_effect = saves
        .iter()
        .position(|s| s.2.messages.last() == Some(&Msg::tool("a", "effect 1")))
        .unwrap();
    let second_effect = saves
        .iter()
        .position(|s| s.2.messages.last() == Some(&Msg::tool("b", "effect 2")))
        .unwrap();
    assert!(intent < first_effect && first_effect < second_effect);
    assert_eq!(saves[first_effect].2.custom["effects"], 1);
    assert_eq!(saves[second_effect].2.custom["effects"], 2);
    assert!(
        saves[..saves.len() - 1]
            .iter()
            .all(|s| s.2.custom[RECOVERY_KEY]["in_progress"] == true)
    );
    assert_eq!(
        saves.last().unwrap().2.custom[RECOVERY_KEY]["in_progress"],
        false
    );
    assert_eq!(effects.load(Ordering::SeqCst), 2);
    assert!(!agent.checkpoint_failed());
    assert!(!agent.snapshot().custom.contains_key(RECOVERY_KEY));
}

#[tokio::test]
async fn missing_durable_store_refuses_before_any_model_work() {
    let model = Arc::new(ScriptedModel::new(vec![Msg::assistant("unused")]));
    let agent = Agent::builder(model.clone())
        .durable_turn_checkpoints(true)
        .build();
    assert!(
        agent
            .run_turn("go")
            .await
            .unwrap_err()
            .to_string()
            .contains("Further work was stopped")
    );
    assert!(model.requests.lock().unwrap().is_empty());
    assert!(agent.checkpoint_failed());
}

#[tokio::test]
async fn user_save_failure_stops_before_sampling_and_uses_fixed_error() {
    let model = Arc::new(ScriptedModel::new(vec![Msg::assistant("unused")]));
    let cp = Arc::new(FailingStore {
        saved: MemCheckpointer::default(),
        fail: |_| true,
    });
    let agent = Agent::builder(model.clone())
        .checkpointer(cp.clone())
        .durable_turn_checkpoints(true)
        .build();
    let error = agent
        .run_turn("keep my request")
        .await
        .unwrap_err()
        .to_string();
    assert!(!error.contains("synthetic-secret") && !error.contains('\u{1b}'));
    assert!(model.requests.lock().unwrap().is_empty());
    assert!(cp.saved.saves.lock().unwrap().is_empty());
    assert_eq!(
        agent.snapshot().messages,
        vec![Msg::user("keep my request")]
    );
}

#[tokio::test]
async fn intent_save_failure_prevents_all_tools_from_running() {
    let cp = Arc::new(FailingStore {
        saved: MemCheckpointer::default(),
        fail: |s| {
            s.messages
                .iter()
                .any(|m| matches!(m, Msg::Assistant { tool_calls, .. } if !tool_calls.is_empty()))
        },
    });
    let model = Arc::new(ScriptedModel::new(vec![calls(&["a", "b"])]));
    let effects = Arc::new(AtomicUsize::new(0));
    let agent = Agent::builder(model.clone())
        .checkpointer(cp)
        .durable_turn_checkpoints(true)
        .tool(Arc::new(Effect(effects.clone())))
        .build();
    assert!(agent.run_turn("go").await.is_err());
    assert_eq!(model.requests.lock().unwrap().len(), 1);
    assert_eq!(effects.load(Ordering::SeqCst), 0);
    assert!(agent.checkpoint_failed());
}

#[tokio::test]
async fn completed_effect_save_failure_keeps_actual_result_and_stops_sibling_dispatch() {
    let cp = Arc::new(FailingStore {
        saved: MemCheckpointer::default(),
        fail: |s| s.messages.iter().any(|m| matches!(m, Msg::Tool { .. })),
    });
    let model = Arc::new(ScriptedModel::new(vec![
        calls(&["a", "b"]),
        Msg::assistant("must not run"),
    ]));
    let effects = Arc::new(AtomicUsize::new(0));
    let agent = Agent::builder(model.clone())
        .checkpointer(cp.clone())
        .durable_turn_checkpoints(true)
        .tool(Arc::new(Effect(effects.clone())))
        .build();
    assert!(agent.run_turn("go").await.is_err());
    assert_eq!(effects.load(Ordering::SeqCst), 1);
    assert_eq!(model.requests.lock().unwrap().len(), 1);
    assert_eq!(
        agent.snapshot().messages.last(),
        Some(&Msg::tool("a", "effect 1"))
    );
    let recovered = Agent::builder(Arc::new(ScriptedModel::new(vec![])))
        .checkpointer(cp)
        .durable_turn_checkpoints(true)
        .build();
    assert!(recovered.load_thread("default").await.unwrap());
    assert_eq!(recovered.recovery_info().unwrap().unresolved_tool_calls, 2);
    assert_eq!(effects.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn reopening_keeps_completed_results_and_never_replays_unknown_calls() {
    let cp = Arc::new(MemCheckpointer::default());
    let mut state = AgentState::default();
    state.messages = vec![
        Msg::user("work"),
        calls(&["a", "b"]),
        Msg::tool("a", "real result"),
    ];
    state.custom.insert(
        RECOVERY_KEY.into(),
        serde_json::json!({"version":1,"in_progress":true}),
    );
    cp.save("thread", 41, &state, None).await.unwrap();
    let effects = Arc::new(AtomicUsize::new(0));
    let model = Arc::new(ScriptedModel::new(vec![Msg::assistant("reviewed")]));
    let agent = Agent::builder(model.clone())
        .checkpointer(cp.clone())
        .durable_turn_checkpoints(true)
        .tool(Arc::new(Effect(effects.clone())))
        .build();
    agent.load_thread("thread").await.unwrap();
    assert_eq!(
        agent.recovery_info(),
        Some(crate::checkpoint::RecoveryInfo {
            unfinished_turn: true,
            unresolved_tool_calls: 1
        })
    );
    assert_eq!(effects.load(Ordering::SeqCst), 0);
    assert!(model.requests.lock().unwrap().is_empty());
    assert_eq!(agent.snapshot().messages[2], Msg::tool("a", "real result"));
    assert!(
        matches!(&agent.snapshot().messages[3], Msg::Tool { tool_call_id, content, is_error: true } if tool_call_id == "b" && content.contains("may have had effects"))
    );
    agent.run_turn("inspect only").await.unwrap();
    let requests = model.requests.lock().unwrap();
    assert_eq!(requests[0].messages[2], Msg::tool("a", "real result"));
    assert_eq!(requests[0].messages[4], Msg::user("inspect only"));
    assert_eq!(effects.load(Ordering::SeqCst), 0);
    assert!(cp.saves.lock().unwrap().iter().skip(1).all(|s| s.1 > 41));
}

#[tokio::test]
async fn completed_checkpoint_reopens_without_warning_or_marker_in_live_state() {
    let cp = Arc::new(MemCheckpointer::default());
    let agent = Agent::builder(Arc::new(ScriptedModel::new(vec![Msg::assistant("done")])))
        .checkpointer(cp.clone())
        .durable_turn_checkpoints(true)
        .build();
    agent.run_turn("go").await.unwrap();
    let recovered = Agent::builder(Arc::new(ScriptedModel::new(vec![])))
        .checkpointer(cp)
        .durable_turn_checkpoints(true)
        .build();
    recovered.load_thread("default").await.unwrap();
    assert_eq!(recovered.recovery_info(), None);
    assert!(!recovered.snapshot().custom.contains_key(RECOVERY_KEY));
}

#[test]
fn old_checkpoint_reconciliation_respects_order_and_reused_call_ids() {
    let mut state = AgentState::default();
    state.messages = vec![
        calls(&["a"]),
        Msg::tool("a", "first round"),
        Msg::user("next"),
        calls(&["a", "b"]),
        Msg::tool("b", "second round"),
        Msg::user("after crash"),
    ];
    assert_eq!(reconcile_recovered_calls(&mut state, None), 1);
    assert_eq!(state.messages[1], Msg::tool("a", "first round"));
    assert!(
        matches!(&state.messages[5], Msg::Tool { tool_call_id, is_error:true, .. } if tool_call_id == "a")
    );
    assert_eq!(state.messages[6], Msg::user("after crash"));
    let before = state.messages.clone();
    assert_eq!(reconcile_recovered_calls(&mut state, None), 0);
    assert_eq!(state.messages, before);
}

#[tokio::test]
async fn parked_approval_stays_parked_while_sibling_completion_becomes_unknown() {
    use crate::interrupt::{HitlAction, HitlRequest, SuspensionFrame};
    struct ApprovedEffect {
        cp: Arc<MemCheckpointer>,
        effects: Arc<AtomicUsize>,
    }
    #[async_trait::async_trait]
    impl Tool for ApprovedEffect {
        fn def(&self) -> ToolDef {
            Effect(self.effects.clone()).def()
        }
        async fn call(
            &self,
            args: serde_json::Value,
            ctx: &mut ToolCtx<'_>,
        ) -> Result<ToolOutcome, AgentError> {
            let (_, state, suspension) = self.cp.load_latest("t").await.unwrap().unwrap();
            assert!(
                suspension.is_none(),
                "approval must be durably consumed before effects"
            );
            assert_eq!(state.custom[RECOVERY_KEY]["in_progress"], true);
            Effect(self.effects.clone()).call(args, ctx).await
        }
    }
    let cp = Arc::new(MemCheckpointer::default());
    let mut state = AgentState::default();
    state.messages.push(calls(&["a", "b"]));
    state.custom.insert(
        RECOVERY_KEY.into(),
        serde_json::json!({"version":1,"in_progress":true}),
    );
    let suspension = Suspension {
        frames: vec![SuspensionFrame {
            agent_name: "main".into(),
        }],
        request: HitlRequest {
            tool_call: ToolCall {
                id: "a".into(),
                name: "effect".into(),
                args: serde_json::json!({}),
            },
            summary: "approval".into(),
        },
    };
    cp.save("t", 1, &state, Some(&suspension)).await.unwrap();
    let effects = Arc::new(AtomicUsize::new(0));
    let agent = Agent::builder(Arc::new(ScriptedModel::new(vec![Msg::assistant("done")])))
        .checkpointer(cp.clone())
        .durable_turn_checkpoints(true)
        .tool(Arc::new(ApprovedEffect {
            cp,
            effects: effects.clone(),
        }))
        .build();
    agent.load_thread("t").await.unwrap();
    assert_eq!(agent.pending_suspension(), Some(suspension));
    assert_eq!(agent.recovery_info().unwrap().unresolved_tool_calls, 1);
    assert_eq!(effects.load(Ordering::SeqCst), 0);
    assert!(agent.run_turn("must not bypass approval").await.is_err());
    assert_eq!(effects.load(Ordering::SeqCst), 0);
    agent.resume(HitlAction::Approve, "denied").await.unwrap();
    assert_eq!(effects.load(Ordering::SeqCst), 1);
    assert!(agent.snapshot().messages.iter().any(|m| matches!(m, Msg::Tool { tool_call_id, content, .. } if tool_call_id == "b" && content.contains("may have had effects"))));
}

#[tokio::test]
async fn malformed_recovery_marker_does_not_replace_current_session() {
    for marker in [
        serde_json::json!({"version":2,"in_progress":true}),
        serde_json::json!({"version":1,"in_progress":"true"}),
        serde_json::json!({"version":1,"in_progress":true,"extra":0}),
    ] {
        let cp = Arc::new(MemCheckpointer::default());
        let mut state = AgentState::default();
        state.custom.insert(RECOVERY_KEY.into(), marker);
        cp.save("foreign", 0, &state, None).await.unwrap();
        let agent = Agent::builder(Arc::new(ScriptedModel::new(vec![])))
            .checkpointer(cp)
            .durable_turn_checkpoints(true)
            .thread_id("current")
            .build();
        agent.state.lock().unwrap().messages.push(Msg::user("keep"));
        assert!(agent.load_thread("foreign").await.is_err());
        assert_eq!(agent.thread(), "current");
        assert_eq!(agent.snapshot().messages, vec![Msg::user("keep")]);
    }
}

#[tokio::test]
async fn recovery_metadata_collision_and_sequence_overflow_refuse_without_sampling() {
    let model = Arc::new(ScriptedModel::new(vec![]));
    let cp = Arc::new(MemCheckpointer::default());
    let agent = Agent::builder(model.clone())
        .checkpointer(cp.clone())
        .durable_turn_checkpoints(true)
        .build();
    agent
        .state
        .lock()
        .unwrap()
        .custom
        .insert(RECOVERY_KEY.into(), serde_json::json!("foreign"));
    assert!(agent.run_turn("go").await.is_err());
    assert_eq!(agent.snapshot().custom[RECOVERY_KEY], "foreign");
    agent.state.lock().unwrap().custom.remove(RECOVERY_KEY);
    agent.seq.store(u64::MAX, Ordering::SeqCst);
    assert!(agent.run_turn("go again").await.is_err());
    assert_eq!(agent.seq.load(Ordering::SeqCst), u64::MAX);
    assert!(cp.saves.lock().unwrap().is_empty());
    assert!(model.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn legacy_save_failure_keeps_original_best_effort_behavior() {
    let cp = Arc::new(FailingStore {
        saved: MemCheckpointer::default(),
        fail: |_| true,
    });
    let agent = Agent::builder(Arc::new(ScriptedModel::new(vec![Msg::assistant("done")])))
        .checkpointer(cp)
        .build();
    assert!(matches!(
        agent.run_turn("go").await.unwrap(),
        TurnResult::Done(_)
    ));
    assert!(agent.checkpoint_failed());
}

#[tokio::test]
async fn failed_final_save_never_replays_turn_end_effects() {
    struct End(Arc<AtomicUsize>);
    #[async_trait::async_trait]
    impl AgentMiddleware for End {
        fn name(&self) -> &str {
            "recovery-end-effect"
        }
        async fn turn_end(&self, _: &mut AgentState) -> Result<(), AgentError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }
    let cp = Arc::new(FailingStore {
        saved: MemCheckpointer::default(),
        fail: |s| s.custom[RECOVERY_KEY]["in_progress"] == false,
    });
    let ends = Arc::new(AtomicUsize::new(0));
    let agent = Agent::builder(Arc::new(ScriptedModel::new(vec![Msg::assistant(
        "actual answer",
    )])))
    .checkpointer(cp)
    .durable_turn_checkpoints(true)
    .middleware(Arc::new(End(ends.clone())))
    .build();
    assert!(agent.run_turn("go").await.is_err());
    assert_eq!(ends.load(Ordering::SeqCst), 1);
    assert_eq!(
        agent.snapshot().messages.last(),
        Some(&Msg::assistant("actual answer"))
    );
    assert!(agent.checkpoint_failed());
}
