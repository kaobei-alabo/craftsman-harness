use super::*;
use crate::ModelResponse;
use crate::testing::{MemCheckpointer, ScriptedModel};
use std::sync::atomic::{AtomicUsize, Ordering};

struct WriteOnce(Arc<AtomicUsize>);
#[async_trait::async_trait]
impl Tool for WriteOnce {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "write_once".into(),
            description: "continuation fixture".into(),
            parameters: serde_json::json!({"type":"object"}),
        }
    }
    async fn call(
        &self,
        _: serde_json::Value,
        ctx: &mut ToolCtx<'_>,
    ) -> Result<ToolOutcome, AgentError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        ctx.state.files.insert(
            "retained.txt".into(),
            crate::state::FileData::text("actual effect"),
        );
        ctx.state.todos.push(crate::state::Todo {
            content: "retained task".into(),
            status: crate::state::TodoStatus::Completed,
        });
        ctx.state
            .custom
            .insert("retained".into(), serde_json::json!(true));
        Ok(ToolOutcome::ok("written once"))
    }
}

fn call() -> Msg {
    Msg::Assistant {
        content: String::new(),
        tool_calls: vec![ToolCall {
            id: "write-1".into(),
            name: "write_once".into(),
            args: serde_json::json!({}),
        }],
    }
}

#[tokio::test]
async fn new_model_receives_retained_history_and_state_without_replaying_effects_or_resetting_sequence()
 {
    let cp = Arc::new(MemCheckpointer::default());
    let effects = Arc::new(AtomicUsize::new(0));
    let old = Agent::builder(Arc::new(ScriptedModel::new(vec![
        call(),
        Msg::assistant("remember alpha"),
    ])))
    .thread_id("same-thread")
    .checkpointer(cp.clone())
    .durable_turn_checkpoints(true)
    .tool(Arc::new(WriteOnce(effects.clone())))
    .build();
    old.run_turn("do work").await.unwrap();
    let last_seq = cp.saves.lock().unwrap().last().unwrap().1;
    let new_model = Arc::new(ScriptedModel::new(vec![Msg::assistant("continued")]));
    let new = Agent::builder(new_model.clone())
        .checkpointer(cp.clone())
        .durable_turn_checkpoints(true)
        .tool(Arc::new(WriteOnce(effects.clone())))
        .build();
    new.inherit_conversation(&old).unwrap();
    assert!(new_model.requests.lock().unwrap().is_empty());
    assert_eq!(effects.load(Ordering::SeqCst), 1);
    assert_eq!(new.thread(), "same-thread");
    assert_eq!(
        new.snapshot().files["retained.txt"].content,
        "actual effect"
    );
    assert_eq!(
        new.snapshot().todos[0].status,
        crate::state::TodoStatus::Completed
    );
    assert_eq!(new.snapshot().custom["retained"], true);
    assert!(new.snapshot().last_usage.is_none());
    new.run_turn("continue here").await.unwrap();
    let requests = new_model.requests.lock().unwrap();
    assert!(
        requests[0]
            .messages
            .iter()
            .any(|m| m == &Msg::assistant("remember alpha"))
    );
    assert!(
        requests[0]
            .messages
            .iter()
            .any(|m| m == &Msg::tool("write-1", "written once"))
    );
    assert_eq!(effects.load(Ordering::SeqCst), 1);
    assert!(
        cp.saves
            .lock()
            .unwrap()
            .iter()
            .filter(|s| s.1 > last_seq)
            .all(|s| s.0 == "same-thread")
    );
    assert!(cp.saves.lock().unwrap().last().unwrap().1 > last_seq);
}

#[tokio::test]
async fn transfer_preserves_parked_approval_and_requires_explicit_resolution() {
    let effects = Arc::new(AtomicUsize::new(0));
    let old = Agent::builder(Arc::new(ScriptedModel::new(vec![call()])))
        .tool(Arc::new(WriteOnce(effects.clone())))
        .middleware(Arc::new(crate::builtin::HitlPolicyMiddleware::new(
            ["write_once"],
            "old",
        )))
        .build();
    assert!(matches!(
        old.run_turn("work").await.unwrap(),
        TurnResult::Suspended(_)
    ));
    let parked = old.pending_suspension().unwrap();
    let new = Agent::builder(Arc::new(ScriptedModel::new(vec![Msg::assistant(
        "approved done",
    )])))
    .tool(Arc::new(WriteOnce(effects.clone())))
    .build();
    new.inherit_conversation(&old).unwrap();
    assert_eq!(new.pending_suspension(), Some(parked));
    assert_eq!(effects.load(Ordering::SeqCst), 0);
    new.resume(crate::interrupt::HitlAction::Approve, "denied")
        .await
        .unwrap();
    assert_eq!(effects.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn closed_old_owner_does_not_close_the_new_model_owner() {
    let old_owner = crate::SessionCancellation::new();
    let new_owner = crate::SessionCancellation::new();
    let old = Agent::builder(Arc::new(ScriptedModel::new(vec![Msg::assistant(
        "old done",
    )])))
    .interrupt_model_on_cancel(true)
    .cancellation_session(old_owner.clone())
    .build();
    old.run_turn("old work").await.unwrap();
    old_owner.cancel();
    let new = Agent::builder(Arc::new(ScriptedModel::new(vec![Msg::assistant(
        "new done",
    )])))
    .interrupt_model_on_cancel(true)
    .cancellation_session(new_owner.clone())
    .build();
    new.inherit_conversation(&old).unwrap();
    assert!(!new_owner.is_cancelled());
    assert!(matches!(
        new.run_turn("new work").await.unwrap(),
        TurnResult::Done(_)
    ));
    assert!(
        new.snapshot()
            .messages
            .contains(&Msg::assistant("old done"))
    );
}

#[tokio::test]
async fn active_and_identical_agents_refuse_transfer_without_replacing_target_state() {
    struct Waiting(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>);
    #[async_trait::async_trait]
    impl ModelProvider for Waiting {
        async fn complete(&self, _: ModelRequest) -> Result<ModelResponse, AgentError> {
            self.0.notify_one();
            self.1.notified().await;
            Ok(ModelResponse {
                message: Msg::assistant("done"),
                usage: crate::Usage::default(),
            })
        }
    }
    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let old = Agent::builder(Arc::new(Waiting(started.clone(), release.clone()))).build();
    let task = tokio::spawn({
        let old = old.clone();
        async move { old.run_turn("active").await }
    });
    started.notified().await;
    let target = Agent::builder(Arc::new(ScriptedModel::new(vec![]))).build();
    target
        .state
        .lock()
        .unwrap()
        .messages
        .push(Msg::user("keep target"));
    assert!(target.inherit_conversation(&old).is_err());
    assert_eq!(target.snapshot().messages, vec![Msg::user("keep target")]);
    assert!(old.inherit_conversation(&old.clone()).is_err());
    release.notify_one();
    task.await.unwrap().unwrap();
}
