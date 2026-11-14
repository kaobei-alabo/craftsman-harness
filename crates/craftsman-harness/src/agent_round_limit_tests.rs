//! Opted-in limit completion retains original effects, approval and durability.
use super::*;
use crate::testing::{MemCheckpointer, ScriptedModel};
use std::sync::atomic::{AtomicUsize, Ordering};

fn effect_call(id: &str) -> Msg {
    Msg::Assistant {
        content: String::new(),
        tool_calls: vec![ToolCall {
            id: id.into(),
            name: "effect".into(),
            args: serde_json::json!({}),
        }],
    }
}

struct Effect(Arc<AtomicUsize>);
#[async_trait::async_trait]
impl Tool for Effect {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "effect".into(),
            description: "fixture effect".into(),
            parameters: serde_json::json!({"type":"object"}),
        }
    }
    async fn call(
        &self,
        _: serde_json::Value,
        ctx: &mut ToolCtx<'_>,
    ) -> Result<ToolOutcome, AgentError> {
        let n = self.0.fetch_add(1, Ordering::SeqCst) + 1;
        if n == 1 {
            ctx.backend.write("effect.txt", "effect 1").await?;
        } else {
            ctx.backend
                .edit(
                    "effect.txt",
                    &format!("effect {}", n - 1),
                    &format!("effect {n}"),
                    false,
                )
                .await?;
        }
        ctx.state
            .custom
            .insert("effects".into(), serde_json::json!(n));
        Ok(ToolOutcome::ok(format!("effect {n}")))
    }
}

struct End(Arc<AtomicUsize>);
#[async_trait::async_trait]
impl AgentMiddleware for End {
    fn name(&self) -> &str {
        "round_limit_end"
    }
    async fn turn_end(&self, state: &mut AgentState) -> Result<(), AgentError> {
        let n = self.0.fetch_add(1, Ordering::SeqCst) + 1;
        state.custom.insert("ended".into(), serde_json::json!(n));
        Ok(())
    }
}

#[tokio::test]
async fn opted_round_limit_closes_once_preserves_actual_results_and_continues_without_replay() {
    let model = Arc::new(ScriptedModel::new(vec![
        effect_call("one"),
        effect_call("two"),
        Msg::assistant("done"),
    ]));
    let cp = Arc::new(MemCheckpointer::default());
    let effects = Arc::new(AtomicUsize::new(0));
    let ends = Arc::new(AtomicUsize::new(0));
    let agent = Agent::builder(model.clone())
        .max_rounds(2)
        .finish_on_round_limit(true)
        .checkpointer(cp.clone())
        .durable_turn_checkpoints(true)
        .tool(Arc::new(Effect(effects.clone())))
        .middleware(Arc::new(End(ends.clone())))
        .build();
    assert!(matches!(
        agent.run_turn("build").await,
        Err(AgentError::MaxRounds(2))
    ));
    assert_eq!(model.requests.lock().unwrap().len(), 2);
    assert_eq!(effects.load(Ordering::SeqCst), 2);
    assert_eq!(ends.load(Ordering::SeqCst), 1);
    let state = agent.snapshot();
    assert_eq!(state.files["effect.txt"].content, "effect 2");
    assert_eq!(
        state.messages,
        vec![
            Msg::user("build"),
            effect_call("one"),
            Msg::tool("one", "effect 1"),
            effect_call("two"),
            Msg::tool("two", "effect 2")
        ]
    );
    let saved = cp.saves.lock().unwrap().last().unwrap().2.clone();
    assert_eq!(saved.messages, state.messages);
    assert_eq!(saved.custom[RECOVERY_KEY]["in_progress"], false);
    assert!(!agent.checkpoint_failed() && !agent.is_turn_active());
    agent.load_thread("default").await.unwrap();
    assert!(agent.recovery_info().is_none());
    assert_eq!(
        model.requests.lock().unwrap().len(),
        2,
        "restore does not sample or dispatch"
    );
    let TurnResult::Done(output) = agent.run_turn("continue").await.unwrap() else {
        panic!("follow-up suspended")
    };
    assert_eq!(output.final_text(), "done");
    assert_eq!(model.requests.lock().unwrap().len(), 3);
    assert_eq!(effects.load(Ordering::SeqCst), 2);
    assert_eq!(ends.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn legacy_round_limit_retains_original_error_without_end_hook() {
    let model = Arc::new(ScriptedModel::new(vec![effect_call("one")]));
    let cp = Arc::new(MemCheckpointer::default());
    let ends = Arc::new(AtomicUsize::new(0));
    let agent = Agent::builder(model)
        .max_rounds(1)
        .checkpointer(cp.clone())
        .durable_turn_checkpoints(true)
        .tool(Arc::new(Effect(Arc::new(AtomicUsize::new(0)))))
        .middleware(Arc::new(End(ends.clone())))
        .build();
    assert!(matches!(
        agent.run_turn("build").await,
        Err(AgentError::MaxRounds(1))
    ));
    assert_eq!(ends.load(Ordering::SeqCst), 0);
    assert_eq!(
        cp.saves.lock().unwrap().last().unwrap().2.custom[RECOVERY_KEY]["in_progress"],
        true
    );
}

struct RequireApproval;
#[async_trait::async_trait]
impl AgentMiddleware for RequireApproval {
    fn name(&self) -> &str {
        "round_limit_approval"
    }
    async fn around_tool(
        &self,
        call: ToolCall,
        _: &mut ToolCtx<'_>,
        _: crate::middleware::ToolNext<'_>,
    ) -> Result<ToolOutcome, AgentError> {
        Err(AgentError::Suspended(Box::new(Suspension {
            frames: vec![],
            request: crate::interrupt::HitlRequest {
                tool_call: call,
                summary: "approve the fixture effect".into(),
            },
        })))
    }
}

#[tokio::test]
async fn round_limit_after_last_approval_preserves_permission_and_result() {
    for approved in [false, true] {
        let model = Arc::new(ScriptedModel::new(vec![effect_call("gated")]));
        let effects = Arc::new(AtomicUsize::new(0));
        let ends = Arc::new(AtomicUsize::new(0));
        let agent = Agent::builder(model.clone())
            .max_rounds(1)
            .finish_on_round_limit(true)
            .tool(Arc::new(Effect(effects.clone())))
            .middleware(Arc::new(RequireApproval))
            .middleware(Arc::new(End(ends.clone())))
            .build();
        assert!(matches!(
            agent.run_turn("build").await,
            Ok(TurnResult::Suspended(_))
        ));
        assert_eq!(effects.load(Ordering::SeqCst), 0);
        assert_eq!(ends.load(Ordering::SeqCst), 0);
        let action = if approved {
            crate::interrupt::HitlAction::Approve
        } else {
            crate::interrupt::HitlAction::Deny {
                feedback: "no".into(),
            }
        };
        assert!(matches!(
            agent.resume(action, "denied").await,
            Err(AgentError::MaxRounds(1))
        ));
        assert_eq!(effects.load(Ordering::SeqCst), usize::from(approved));
        assert_eq!(ends.load(Ordering::SeqCst), 1);
        assert_eq!(model.requests.lock().unwrap().len(), 1);
        assert!(agent.pending_suspension().is_none());
        assert!(
            matches!(agent.snapshot().messages.last(), Some(Msg::Tool { tool_call_id, .. }) if tool_call_id == "gated")
        );
    }
}

struct Interact {
    agent: Arc<Mutex<Option<Agent>>>,
    cancel: bool,
}
#[async_trait::async_trait]
impl Tool for Interact {
    fn def(&self) -> ToolDef {
        Effect(Arc::new(AtomicUsize::new(0))).def()
    }
    async fn call(
        &self,
        _: serde_json::Value,
        _: &mut ToolCtx<'_>,
    ) -> Result<ToolOutcome, AgentError> {
        let agent = self.agent.lock().unwrap().as_ref().unwrap().clone();
        if self.cancel {
            assert!(agent.request_cancel());
        } else {
            assert!(agent.steer("last-round correction"));
        }
        Ok(ToolOutcome::ok("actual effect returned"))
    }
}

#[tokio::test]
async fn cancel_during_last_tool_wins_over_round_limit() {
    let holder = Arc::new(Mutex::new(None));
    let model = Arc::new(ScriptedModel::new(vec![effect_call("last")]));
    let ends = Arc::new(AtomicUsize::new(0));
    let agent = Agent::builder(model.clone())
        .max_rounds(1)
        .finish_on_round_limit(true)
        .tool(Arc::new(Interact {
            agent: holder.clone(),
            cancel: true,
        }))
        .middleware(Arc::new(End(ends.clone())))
        .build();
    *holder.lock().unwrap() = Some(agent.clone());
    let TurnResult::Done(output) = agent.run_turn("build").await.unwrap() else {
        panic!("cancel suspended")
    };
    assert_eq!(output.final_text(), "Stopped by the user.");
    assert!(
        agent
            .snapshot()
            .messages
            .contains(&Msg::tool("last", "actual effect returned"))
    );
    assert_eq!(ends.load(Ordering::SeqCst), 1);
    assert_eq!(model.requests.lock().unwrap().len(), 1);
    holder.lock().unwrap().take();
}

#[tokio::test]
async fn last_round_steering_is_saved_and_not_sampled_before_follow_up() {
    let holder = Arc::new(Mutex::new(None));
    let model = Arc::new(ScriptedModel::new(vec![
        effect_call("last"),
        Msg::assistant("done"),
    ]));
    let cp = Arc::new(MemCheckpointer::default());
    let agent = Agent::builder(model.clone())
        .max_rounds(1)
        .finish_on_round_limit(true)
        .checkpointer(cp.clone())
        .durable_turn_checkpoints(true)
        .tool(Arc::new(Interact {
            agent: holder.clone(),
            cancel: false,
        }))
        .build();
    *holder.lock().unwrap() = Some(agent.clone());
    assert!(matches!(
        agent.run_turn("build").await,
        Err(AgentError::MaxRounds(1))
    ));
    assert_eq!(
        cp.saves.lock().unwrap().last().unwrap().2.messages.last(),
        Some(&Msg::user("last-round correction"))
    );
    assert_eq!(model.requests.lock().unwrap().len(), 1);
    agent.run_turn("continue").await.unwrap();
    let requests = model.requests.lock().unwrap();
    assert!(
        requests[1]
            .messages
            .contains(&Msg::user("last-round correction"))
    );
    assert_eq!(
        requests[1]
            .messages
            .iter()
            .filter(|m| **m == Msg::user("last-round correction"))
            .count(),
        1
    );
    holder.lock().unwrap().take();
}

struct FinalStore {
    saved: MemCheckpointer,
    entered: Option<Arc<tokio::sync::Notify>>,
    release: Option<Arc<tokio::sync::Notify>>,
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
        if state.custom.get("ended").is_some() {
            if let (Some(entered), Some(release)) = (&self.entered, &self.release) {
                entered.notify_one();
                release.notified().await;
            } else {
                return Err(AgentError::State("synthetic-secret".into()));
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
    async fn list_threads(&self) -> Result<Vec<crate::checkpoint::ThreadInfo>, AgentError> {
        Ok(vec![])
    }
    async fn delete_thread(&self, _: &str) -> Result<(), AgentError> {
        Ok(())
    }
}

#[tokio::test]
async fn failed_round_limit_checkpoint_remains_an_error_and_refuses_later_sampling() {
    let model = Arc::new(ScriptedModel::new(vec![
        effect_call("last"),
        Msg::assistant("unused"),
    ]));
    let cp = Arc::new(FinalStore {
        saved: MemCheckpointer::default(),
        entered: None,
        release: None,
    });
    let effects = Arc::new(AtomicUsize::new(0));
    let ends = Arc::new(AtomicUsize::new(0));
    let agent = Agent::builder(model.clone())
        .max_rounds(1)
        .finish_on_round_limit(true)
        .checkpointer(cp.clone())
        .durable_turn_checkpoints(true)
        .tool(Arc::new(Effect(effects.clone())))
        .middleware(Arc::new(End(ends.clone())))
        .build();
    let error = agent.run_turn("build").await.unwrap_err();
    assert_eq!(error.to_string(), checkpoint_save_error().to_string());
    assert!(agent.checkpoint_failed());
    assert_eq!(effects.load(Ordering::SeqCst), 1);
    assert_eq!(ends.load(Ordering::SeqCst), 1);
    assert_eq!(
        cp.saved.saves.lock().unwrap().last().unwrap().2.custom[RECOVERY_KEY]["in_progress"],
        true
    );
    assert!(agent.run_turn("continue").await.is_err());
    assert_eq!(model.requests.lock().unwrap().len(), 1);
    assert_eq!(effects.load(Ordering::SeqCst), 1);
    assert_eq!(ends.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn limited_turn_rejects_new_work_until_its_actual_final_save_returns() {
    let model = Arc::new(ScriptedModel::new(vec![effect_call("last")]));
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let cp = Arc::new(FinalStore {
        saved: MemCheckpointer::default(),
        entered: Some(entered.clone()),
        release: Some(release.clone()),
    });
    let agent = Agent::builder(model.clone())
        .max_rounds(1)
        .finish_on_round_limit(true)
        .checkpointer(cp)
        .durable_turn_checkpoints(true)
        .tool(Arc::new(Effect(Arc::new(AtomicUsize::new(0)))))
        .middleware(Arc::new(End(Arc::new(AtomicUsize::new(0)))))
        .build();
    let running = agent.clone();
    let task = tokio::spawn(async move { running.run_turn("build").await });
    tokio::time::timeout(std::time::Duration::from_secs(2), entered.notified())
        .await
        .unwrap();
    assert!(agent.is_turn_active());
    assert!(!agent.steer("late correction"));
    assert!(matches!(
        agent.run_turn("overlap").await,
        Err(AgentError::State(_))
    ));
    assert_eq!(model.requests.lock().unwrap().len(), 1);
    release.notify_one();
    assert!(matches!(
        tokio::time::timeout(std::time::Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap(),
        Err(AgentError::MaxRounds(1))
    ));
    assert!(!agent.is_turn_active());
}
