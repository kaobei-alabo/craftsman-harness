//! Opted-in sampling cancellation preserves transcript and tool ownership.
use super::*;
use crate::model::{ModelResponse, Usage};
use async_trait::async_trait;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Duration;
use tokio::sync::Notify;

const WAIT: Duration = Duration::from_secs(1);

struct Dropped(Arc<AtomicBool>);
impl Drop for Dropped {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

fn completed(result: TurnResult) -> TurnOutput {
    match result {
        TurnResult::Done(output) => output,
        TurnResult::Suspended(_) => panic!("sampling cancellation must not suspend"),
    }
}

struct PendingModel {
    calls: AtomicUsize,
    started: Notify,
    dropped: Arc<AtomicBool>,
}

impl PendingModel {
    fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            started: Notify::new(),
            dropped: Arc::new(AtomicBool::new(false)),
        }
    }
}

#[async_trait]
impl ModelProvider for PendingModel {
    async fn complete(&self, _req: ModelRequest) -> Result<ModelResponse, AgentError> {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            let _owned_request = Dropped(self.dropped.clone());
            self.started.notify_one();
            std::future::pending().await
        } else {
            Ok(ModelResponse {
                message: Msg::assistant("next turn"),
                usage: Usage {
                    input_tokens: 7,
                    output_tokens: 3,
                },
            })
        }
    }
}

#[tokio::test]
async fn pending_sample_drops_without_release_and_next_turn_is_unaffected() {
    let model = Arc::new(PendingModel::new());
    let agent = Agent::builder(model.clone())
        .interrupt_model_on_cancel(true)
        .build();
    assert!(!agent.request_cancel());
    let running = agent.clone();
    let task = tokio::spawn(async move { running.run_turn("first").await });
    tokio::time::timeout(WAIT, model.started.notified())
        .await
        .unwrap();
    assert!(agent.request_cancel());
    assert!(agent.request_cancel());
    let output = tokio::time::timeout(WAIT, task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(completed(output).final_text(), "Stopped by the user.");
    assert!(model.dropped.load(Ordering::SeqCst));
    assert!(!agent.is_turn_active());
    assert!(!agent.request_cancel());
    {
        let state = agent.state.lock().unwrap();
        assert!(state.last_usage.is_none());
        assert!(
            matches!(state.messages.as_slice(), [Msg::User { content }, Msg::Assistant { content: stop, tool_calls }]
            if content == "first" && stop == "Stopped by the user." && tool_calls.is_empty())
        );
    }
    assert_eq!(
        completed(agent.run_turn("second").await.unwrap()).final_text(),
        "next turn"
    );
    assert_eq!(model.calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        agent.state.lock().unwrap().last_usage,
        Some(Usage {
            input_tokens: 7,
            output_tokens: 3
        })
    );
}

struct OwnedTool {
    started: Notify,
    release: Notify,
    dropped: Arc<AtomicBool>,
}

#[async_trait]
impl Tool for OwnedTool {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "owned".into(),
            description: "isolated owned operation".into(),
            parameters: serde_json::json!({"type":"object"}),
        }
    }
    async fn call(
        &self,
        _args: serde_json::Value,
        ctx: &mut ToolCtx<'_>,
    ) -> Result<ToolOutcome, AgentError> {
        let _owned = Dropped(self.dropped.clone());
        self.started.notify_one();
        self.release.notified().await;
        ctx.state
            .custom
            .insert("completed".into(), serde_json::json!(true));
        Ok(ToolOutcome::ok("actual completed result"))
    }
}

struct ToolModel {
    calls: AtomicUsize,
    cancel: Mutex<Option<Agent>>,
}

#[async_trait]
impl ModelProvider for ToolModel {
    async fn complete(&self, _req: ModelRequest) -> Result<ModelResponse, AgentError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(agent) = self.cancel.lock().unwrap().take() {
            assert!(agent.request_cancel());
        }
        Ok(ModelResponse {
            message: Msg::Assistant {
                content: "actual response".into(),
                tool_calls: ["first", "later"]
                    .into_iter()
                    .map(|id| ToolCall {
                        id: id.into(),
                        name: "owned".into(),
                        args: serde_json::json!({}),
                    })
                    .collect(),
            },
            usage: Usage {
                input_tokens: 11,
                output_tokens: 5,
            },
        })
    }
}

#[tokio::test]
async fn running_tool_retains_ownership_and_remaining_calls_are_balanced() {
    let model = Arc::new(ToolModel {
        calls: AtomicUsize::new(0),
        cancel: Mutex::new(None),
    });
    let tool = Arc::new(OwnedTool {
        started: Notify::new(),
        release: Notify::new(),
        dropped: Arc::new(AtomicBool::new(false)),
    });
    let agent = Agent::builder(model.clone())
        .tool(tool.clone())
        .interrupt_model_on_cancel(true)
        .build();
    let running = agent.clone();
    let task = tokio::spawn(async move { running.run_turn("work").await });
    tokio::time::timeout(WAIT, tool.started.notified())
        .await
        .unwrap();
    assert!(agent.request_cancel());
    assert!(agent.is_turn_active());
    assert!(!task.is_finished());
    assert!(!tool.dropped.load(Ordering::SeqCst));
    tool.release.notify_one();
    let result = tokio::time::timeout(WAIT, task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(completed(result).final_text(), "Stopped by the user.");
    assert!(tool.dropped.load(Ordering::SeqCst));
    assert_eq!(model.calls.load(Ordering::SeqCst), 1);
    let state = agent.state.lock().unwrap();
    assert_eq!(
        state.custom.get("completed"),
        Some(&serde_json::json!(true))
    );
    assert!(state.messages.iter().any(|m| matches!(m, Msg::Tool { tool_call_id, content, is_error: false } if tool_call_id == "first" && content == "actual completed result")));
    assert!(state.messages.iter().any(|m| matches!(m, Msg::Tool { tool_call_id, content, is_error: true } if tool_call_id == "later" && content == "Tool call cancelled by the user.")));
}

#[tokio::test]
async fn stop_requested_in_ready_response_keeps_actual_usage_without_dispatch() {
    let model = Arc::new(ToolModel {
        calls: AtomicUsize::new(0),
        cancel: Mutex::new(None),
    });
    let tool = Arc::new(OwnedTool {
        started: Notify::new(),
        release: Notify::new(),
        dropped: Arc::new(AtomicBool::new(false)),
    });
    let agent = Agent::builder(model.clone())
        .tool(tool.clone())
        .interrupt_model_on_cancel(true)
        .build();
    *model.cancel.lock().unwrap() = Some(agent.clone());
    let result = tokio::time::timeout(WAIT, agent.run_turn("work"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(completed(result).final_text(), "Stopped by the user.");
    assert!(!tool.dropped.load(Ordering::SeqCst));
    let state = agent.state.lock().unwrap();
    assert_eq!(
        state.last_usage,
        Some(Usage {
            input_tokens: 11,
            output_tokens: 5
        })
    );
    let results: Vec<_> = state
        .messages
        .iter()
        .filter_map(|m| match m {
            Msg::Tool {
                tool_call_id,
                content,
                is_error,
            } => Some((tool_call_id.as_str(), content.as_str(), *is_error)),
            _ => None,
        })
        .collect();
    assert_eq!(
        results,
        [
            ("first", "Tool call cancelled by the user.", true),
            ("later", "Tool call cancelled by the user.", true)
        ]
    );
}

struct PrepareGate {
    started: Notify,
    release: Notify,
}
#[async_trait]
impl AgentMiddleware for PrepareGate {
    fn name(&self) -> &str {
        "prepare_fixture"
    }
    async fn prepare_model(&self, state: &mut AgentState) -> Result<(), AgentError> {
        self.started.notify_one();
        self.release.notified().await;
        state.system_extras.push("completed preparation".into());
        Ok(())
    }
}

#[tokio::test]
async fn stop_before_wait_registration_refuses_model_after_owned_preparation() {
    let model = Arc::new(PendingModel::new());
    let prepare = Arc::new(PrepareGate {
        started: Notify::new(),
        release: Notify::new(),
    });
    let agent = Agent::builder(model.clone())
        .middleware(prepare.clone())
        .interrupt_model_on_cancel(true)
        .build();
    let running = agent.clone();
    let task = tokio::spawn(async move { running.run_turn("first").await });
    tokio::time::timeout(WAIT, prepare.started.notified())
        .await
        .unwrap();
    assert!(agent.request_cancel());
    assert!(!task.is_finished());
    prepare.release.notify_one();
    assert_eq!(
        completed(
            tokio::time::timeout(WAIT, task)
                .await
                .unwrap()
                .unwrap()
                .unwrap()
        )
        .final_text(),
        "Stopped by the user."
    );
    assert_eq!(model.calls.load(Ordering::SeqCst), 0);
}

struct PendingSamplingHook {
    started: Notify,
    dropped: Arc<AtomicBool>,
    ends: AtomicUsize,
}
#[async_trait]
impl AgentMiddleware for PendingSamplingHook {
    fn name(&self) -> &str {
        "sampling_fixture"
    }
    async fn around_model(
        &self,
        _req: ModelRequest,
        _next: crate::middleware::Next<'_>,
    ) -> Result<ModelResponse, AgentError> {
        let _owned = Dropped(self.dropped.clone());
        self.started.notify_one();
        std::future::pending().await
    }
    async fn turn_end(&self, state: &mut AgentState) -> Result<(), AgentError> {
        assert!(self.dropped.load(Ordering::SeqCst));
        assert!(
            matches!(state.messages.last(), Some(Msg::Assistant { content, .. }) if content == "Stopped by the user.")
        );
        self.ends.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn pending_sampling_hook_drops_before_normal_turn_end() {
    let model = Arc::new(PendingModel::new());
    let hook = Arc::new(PendingSamplingHook {
        started: Notify::new(),
        dropped: Arc::new(AtomicBool::new(false)),
        ends: AtomicUsize::new(0),
    });
    let agent = Agent::builder(model.clone())
        .middleware(hook.clone())
        .interrupt_model_on_cancel(true)
        .build();
    let running = agent.clone();
    let task = tokio::spawn(async move { running.run_turn("first").await });
    tokio::time::timeout(WAIT, hook.started.notified())
        .await
        .unwrap();
    assert!(agent.request_cancel());
    assert_eq!(
        completed(
            tokio::time::timeout(WAIT, task)
                .await
                .unwrap()
                .unwrap()
                .unwrap()
        )
        .final_text(),
        "Stopped by the user."
    );
    assert_eq!(hook.ends.load(Ordering::SeqCst), 1);
    assert_eq!(model.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn uncancelled_model_error_keeps_its_original_classification() {
    struct FailedModel;
    #[async_trait]
    impl ModelProvider for FailedModel {
        async fn complete(&self, _req: ModelRequest) -> Result<ModelResponse, AgentError> {
            Err(AgentError::Model("original fixture failure".into()))
        }
    }
    let agent = Agent::builder(Arc::new(FailedModel))
        .interrupt_model_on_cancel(true)
        .build();
    assert!(
        matches!(agent.run_turn("first").await, Err(AgentError::Model(text)) if text == "original fixture failure")
    );
    assert!(!agent.is_turn_active());
    assert!(!agent.request_cancel());
    assert_eq!(agent.state.lock().unwrap().messages.len(), 1);
}
