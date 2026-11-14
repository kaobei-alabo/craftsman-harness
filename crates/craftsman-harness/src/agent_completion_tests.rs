//! Actual completion dispatch, durable ordering, permissions and cancellation.
use super::*;
use crate::{
    CompletionAction, CompletionAfterCheck, CompletionCheck,
    testing::{MemCheckpointer, ScriptedModel},
};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Once {
    calls: Arc<AtomicUsize>,
    policies: Arc<AtomicUsize>,
}
#[async_trait::async_trait]
impl CompletionCheck for Once {
    async fn before_finish(&self, _: &AgentState) -> Result<CompletionAction, AgentError> {
        self.policies.fetch_add(1, Ordering::SeqCst);
        if self.calls.load(Ordering::SeqCst) == 0 {
            Ok(CompletionAction::Check {
                tool: "check".into(),
                args: serde_json::json!({}),
            })
        } else {
            Ok(CompletionAction::Ready)
        }
    }
    async fn after_check(&self, _: &AgentState) -> Result<CompletionAfterCheck, AgentError> {
        if self.calls.load(Ordering::SeqCst) == 0 {
            Ok(CompletionAfterCheck::Stop {
                reason: "Actual check did not run.".into(),
            })
        } else {
            Ok(CompletionAfterCheck::Continue)
        }
    }
}

struct Check {
    calls: Arc<AtomicUsize>,
    store: Option<Arc<MemCheckpointer>>,
    signal: Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>,
}
#[async_trait::async_trait]
impl Tool for Check {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "check".into(),
            description: "actual check fixture".into(),
            parameters: serde_json::json!({"type":"object"}),
        }
    }
    async fn call(
        &self,
        _: serde_json::Value,
        ctx: &mut ToolCtx<'_>,
    ) -> Result<ToolOutcome, AgentError> {
        if let Some(store) = &self.store {
            let saved = store.saves.lock().unwrap().last().unwrap().2.clone();
            assert!(
                matches!(saved.messages.last(), Some(Msg::Assistant { tool_calls, .. }) if tool_calls.len()==1 && tool_calls[0].name=="check")
            );
        }
        self.calls.fetch_add(1, Ordering::SeqCst);
        ctx.backend
            .write("actual.txt", "completed actual check")
            .await?;
        if let Some((entered, release)) = &self.signal {
            entered.notify_one();
            release.notified().await;
        }
        let mut meta = crate::ToolMeta::new();
        meta.exit_code = Some(0);
        Ok(ToolOutcome::ok("actual check output").with_meta(meta))
    }
}

#[tokio::test]
async fn completion_check_commits_before_dispatch_and_samples_real_results_once() {
    let calls = Arc::new(AtomicUsize::new(0));
    let policies = Arc::new(AtomicUsize::new(0));
    let store = Arc::new(MemCheckpointer::default());
    let model = Arc::new(ScriptedModel::new(vec![
        Msg::assistant("premature answer"),
        Msg::assistant("verified answer"),
    ]));
    let agent = Agent::builder(model.clone())
        .tool(Arc::new(Check {
            calls: calls.clone(),
            store: Some(store.clone()),
            signal: None,
        }))
        .completion_check(Arc::new(Once {
            calls: calls.clone(),
            policies: policies.clone(),
        }))
        .checkpointer(store.clone())
        .durable_turn_checkpoints(true)
        .build();
    let TurnResult::Done(result) = agent.run_turn("make the change").await.unwrap() else {
        panic!("suspended");
    };
    assert_eq!(result.final_text(), "verified answer");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(policies.load(Ordering::SeqCst), 2);
    let requests = model.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(
        requests[1].messages.iter().any(
            |message| matches!(message,Msg::Tool{content,..} if content=="actual check output")
        )
    );
    let saved = store.saves.lock().unwrap().last().unwrap().2.clone();
    assert_eq!(saved.custom[RECOVERY_KEY]["in_progress"], false);
    assert_eq!(saved.files["actual.txt"].content, "completed actual check");
    assert_eq!(
        saved
            .messages
            .iter()
            .filter(|m| matches!(m, Msg::Tool { .. }))
            .count(),
        1
    );
}

struct Deny;
#[async_trait::async_trait]
impl AgentMiddleware for Deny {
    fn name(&self) -> &str {
        "completion-denial"
    }
    async fn around_tool(
        &self,
        _: ToolCall,
        _: &mut ToolCtx<'_>,
        _: crate::ToolNext<'_>,
    ) -> Result<ToolOutcome, AgentError> {
        Ok(ToolOutcome::err("actual refusal"))
    }
}
#[tokio::test]
async fn completion_permission_denial_stops_without_extra_model_work_or_effects() {
    let calls = Arc::new(AtomicUsize::new(0));
    let model = Arc::new(ScriptedModel::new(vec![
        Msg::assistant("premature"),
        Msg::assistant("unused"),
    ]));
    let agent = Agent::builder(model.clone())
        .tool(Arc::new(Check {
            calls: calls.clone(),
            store: None,
            signal: None,
        }))
        .middleware(Arc::new(Deny))
        .completion_check(Arc::new(Once {
            calls: calls.clone(),
            policies: Arc::new(AtomicUsize::new(0)),
        }))
        .build();
    assert!(
        matches!(agent.run_turn("work").await,Err(AgentError::State(reason)) if reason=="Actual check did not run.")
    );
    assert_eq!(model.requests.lock().unwrap().len(), 1);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(agent.snapshot().files.is_empty());
    assert!(
        agent
            .snapshot()
            .messages
            .iter()
            .any(|m| matches!(m,Msg::Tool{content,is_error:true,..}if content=="actual refusal"))
    );
}

struct FailBeforeEffect {
    saved: MemCheckpointer,
}
#[async_trait::async_trait]
impl Checkpointer for FailBeforeEffect {
    async fn save(
        &self,
        thread: &str,
        seq: u64,
        state: &AgentState,
        suspension: Option<&Suspension>,
    ) -> Result<(), AgentError> {
        if matches!(state.messages.last(),Some(Msg::Assistant{tool_calls,..})if tool_calls.iter().any(|c|c.name=="check"))
        {
            return Err(AgentError::State("synthetic secret".into()));
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
#[tokio::test]
async fn completion_checkpoint_failure_refuses_check_effects_and_further_sampling() {
    let calls = Arc::new(AtomicUsize::new(0));
    let model = Arc::new(ScriptedModel::new(vec![Msg::assistant("premature")]));
    let agent = Agent::builder(model.clone())
        .tool(Arc::new(Check {
            calls: calls.clone(),
            store: None,
            signal: None,
        }))
        .completion_check(Arc::new(Once {
            calls: calls.clone(),
            policies: Arc::new(AtomicUsize::new(0)),
        }))
        .checkpointer(Arc::new(FailBeforeEffect {
            saved: MemCheckpointer::default(),
        }))
        .durable_turn_checkpoints(true)
        .build();
    assert_eq!(
        agent.run_turn("work").await.unwrap_err().to_string(),
        checkpoint_save_error().to_string()
    );
    assert!(agent.checkpoint_failed());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(model.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn completion_check_keeps_turn_active_for_actual_cancellation_and_cleanup() {
    let calls = Arc::new(AtomicUsize::new(0));
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let model = Arc::new(ScriptedModel::new(vec![
        Msg::assistant("premature"),
        Msg::assistant("unused"),
    ]));
    let agent = Agent::builder(model.clone())
        .tool(Arc::new(Check {
            calls: calls.clone(),
            store: None,
            signal: Some((entered.clone(), release.clone())),
        }))
        .completion_check(Arc::new(Once {
            calls: calls.clone(),
            policies: Arc::new(AtomicUsize::new(0)),
        }))
        .build();
    let running = agent.clone();
    let task = tokio::spawn(async move { running.run_turn("work").await });
    tokio::time::timeout(std::time::Duration::from_secs(2), entered.notified())
        .await
        .unwrap();
    assert!(agent.is_turn_active());
    assert!(agent.request_cancel());
    assert!(!task.is_finished());
    release.notify_one();
    let TurnResult::Done(output) = task.await.unwrap().unwrap() else {
        panic!("suspended")
    };
    assert_eq!(output.final_text(), "Stopped by the user.");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(model.requests.lock().unwrap().len(), 1);
    assert!(
        agent
            .snapshot()
            .messages
            .iter()
            .any(|m| matches!(m,Msg::Tool{content,..}if content=="actual check output"))
    );
}

#[tokio::test]
async fn completion_identity_avoids_model_supplied_collisions_and_refuses_overflow() {
    let calls = Arc::new(AtomicUsize::new(0));
    let model = Arc::new(ScriptedModel::new(vec![
        Msg::assistant("premature"),
        Msg::assistant("done"),
    ]));
    let mut state = AgentState::default();
    state.messages.push(Msg::Assistant {
        content: String::new(),
        tool_calls: vec![ToolCall {
            id: "_craftsman_completion_0".into(),
            name: "old".into(),
            args: serde_json::json!({}),
        }],
    });
    state
        .messages
        .push(Msg::tool("_craftsman_completion_0", "old result"));
    let agent = Agent::builder(model)
        .state(Arc::new(Mutex::new(state)))
        .tool(Arc::new(Check {
            calls: calls.clone(),
            store: None,
            signal: None,
        }))
        .completion_check(Arc::new(Once {
            calls,
            policies: Arc::new(AtomicUsize::new(0)),
        }))
        .build();
    agent.run_turn("work").await.unwrap();
    assert!(agent.snapshot().messages.iter().any(|m|matches!(m,Msg::Tool{tool_call_id,content,..}if tool_call_id=="_craftsman_completion_1"&&content=="actual check output")));
    agent.completion_sequence.store(u64::MAX, Ordering::SeqCst);
    assert!(agent.next_completion_id().is_err());
}
