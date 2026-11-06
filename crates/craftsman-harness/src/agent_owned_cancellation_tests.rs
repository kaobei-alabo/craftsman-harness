//! Actual loop/child/compaction ownership under an opted-in turn and session.
use super::*;
use crate::builtin::{CompactionMiddleware, CompactionTrigger, SubAgentMiddleware};
use crate::{
    AgentStatus, ModelResponse, SessionCancellation, SubAgentSpec, TurnCancellation, Usage,
};
use async_trait::async_trait;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Duration;
use tokio::sync::Notify;
const WAIT: Duration = Duration::from_secs(2);

struct DropFlag(Arc<AtomicBool>);
impl Drop for DropFlag {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}
struct BlockedModel {
    started: Notify,
    dropped: Arc<AtomicBool>,
}
impl BlockedModel {
    fn new() -> Self {
        Self {
            started: Notify::new(),
            dropped: Arc::new(AtomicBool::new(false)),
        }
    }
}
#[async_trait]
impl ModelProvider for BlockedModel {
    async fn complete(&self, _req: ModelRequest) -> Result<ModelResponse, AgentError> {
        let _owned = DropFlag(self.dropped.clone());
        self.started.notify_one();
        std::future::pending().await
    }
}
struct ParentModel {
    calls: AtomicUsize,
    background: bool,
}
#[async_trait]
impl ModelProvider for ParentModel {
    async fn complete(&self, _req: ModelRequest) -> Result<ModelResponse, AgentError> {
        let message = if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            Msg::Assistant {
                content: String::new(),
                tool_calls: vec![ToolCall {
                    id: "task-call".into(),
                    name: "task".into(),
                    args: serde_json::json!({"description":"isolated task", "subagent_type":"worker", "run_in_background":self.background}),
                }],
            }
        } else {
            Msg::assistant("parent finished")
        };
        Ok(ModelResponse {
            message,
            usage: Usage::default(),
        })
    }
}
fn parent(
    model: Arc<dyn ModelProvider>,
    child: Arc<dyn ModelProvider>,
    tools: Vec<Arc<dyn Tool>>,
    background: bool,
    session: &SessionCancellation,
) -> Agent {
    let _ = background;
    Agent::builder(model)
        .interrupt_model_on_cancel(true)
        .cancellation_session(session.clone())
        .middleware(Arc::new(SubAgentMiddleware::new(vec![SubAgentSpec {
            name: "worker".into(),
            description: "isolated fixture".into(),
            system_prompt: String::new(),
            tools,
            model: Some(child),
            harness_profile: None,
            interrupt_on: vec![],
            max_depth: 1,
        }])))
        .build()
}
fn text(result: TurnResult) -> String {
    match result {
        TurnResult::Done(output) => output.final_text().to_string(),
        _ => panic!("must not suspend"),
    }
}

#[tokio::test]
async fn foreground_model_stops_with_parent_and_registry_waits_for_actual_return() {
    let child = Arc::new(BlockedModel::new());
    let model = Arc::new(ParentModel {
        calls: AtomicUsize::new(0),
        background: false,
    });
    let session = SessionCancellation::new();
    let agent = parent(model.clone(), child.clone(), vec![], false, &session);
    let registry = agent.registry.clone();
    let running = agent.clone();
    let task = tokio::spawn(async move { running.run_turn("go").await });
    tokio::time::timeout(WAIT, child.started.notified())
        .await
        .unwrap();
    assert!(agent.request_cancel());
    let result = tokio::time::timeout(WAIT, task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(text(result), "Stopped by the user.");
    assert!(child.dropped.load(Ordering::SeqCst));
    assert_eq!(model.calls.load(Ordering::SeqCst), 1);
    assert!(
        registry
            .list()
            .iter()
            .all(|(_, status)| *status == AgentStatus::Failed)
    );
    assert!(agent.state.lock().unwrap().messages.iter().any(|message| matches!(message, Msg::Tool { tool_call_id, is_error: true, .. } if tool_call_id == "task-call")));
}

#[tokio::test]
async fn automatic_compaction_stops_without_replacing_the_original_transcript() {
    let compactor = Arc::new(BlockedModel::new());
    let main = Arc::new(BlockedModel::new());
    let agent = Agent::builder(main.clone())
        .interrupt_model_on_cancel(true)
        .middleware(Arc::new(CompactionMiddleware::new(
            compactor.clone(),
            CompactionTrigger::MessageCount {
                threshold_msgs: 2,
                keep_tail_msgs: 1,
            },
        )))
        .build();
    agent.state.lock().unwrap().messages =
        vec![Msg::user("old input"), Msg::assistant("old answer")];
    let running = agent.clone();
    let task = tokio::spawn(async move { running.run_turn("new input").await });
    tokio::time::timeout(WAIT, compactor.started.notified())
        .await
        .unwrap();
    assert!(agent.request_cancel());
    assert_eq!(
        text(
            tokio::time::timeout(WAIT, task)
                .await
                .unwrap()
                .unwrap()
                .unwrap()
        ),
        "Stopped by the user."
    );
    assert!(compactor.dropped.load(Ordering::SeqCst));
    assert!(!main.dropped.load(Ordering::SeqCst));
    assert_eq!(
        agent.state.lock().unwrap().messages,
        vec![
            Msg::user("old input"),
            Msg::assistant("old answer"),
            Msg::user("new input"),
            Msg::assistant("Stopped by the user.")
        ]
    );
}

#[tokio::test]
async fn legacy_agents_mask_an_ambient_cancelled_turn_including_their_provider() {
    struct Legacy;
    #[async_trait]
    impl ModelProvider for Legacy {
        async fn complete(&self, _req: ModelRequest) -> Result<ModelResponse, AgentError> {
            assert!(TurnCancellation::current().is_none());
            Ok(ModelResponse {
                message: Msg::assistant("legacy result"),
                usage: Usage::default(),
            })
        }
    }
    let turn = SessionCancellation::new().new_turn();
    turn.cancel();
    let agent = Agent::builder(Arc::new(Legacy)).build();
    assert_eq!(
        text(turn.scope(agent.run_turn("go")).await.unwrap()),
        "legacy result"
    );
    assert!(TurnCancellation::current().is_none());
}

#[tokio::test]
async fn closing_one_session_keeps_its_sibling_open_and_shutdown_closes_future_turns() {
    let shutdown = SessionCancellation::new();
    let first = shutdown.child().unwrap();
    let second = shutdown.child().unwrap();
    let first_turn = first.new_turn();
    let second_turn = second.new_turn();
    first.cancel();
    tokio::time::timeout(WAIT, first_turn.cancelled())
        .await
        .unwrap();
    assert!(!second_turn.is_cancelled());
    shutdown.cancel();
    tokio::time::timeout(WAIT, second_turn.cancelled())
        .await
        .unwrap();
    let model = Arc::new(BlockedModel::new());
    let agent = Agent::builder(model.clone())
        .interrupt_model_on_cancel(true)
        .cancellation_session(second)
        .build();
    assert!(matches!(
        agent.run_turn("must not begin").await,
        Err(AgentError::Cancelled)
    ));
    assert!(agent.state.lock().unwrap().messages.is_empty());
    assert!(!agent.is_turn_active());
    assert!(!model.dropped.load(Ordering::SeqCst));
}

#[tokio::test]
async fn background_turn_survives_parent_stop_but_observes_session_shutdown() {
    let shutdown = SessionCancellation::new();
    let foreground = shutdown.new_turn();
    let background = foreground.background();
    foreground.cancel();
    assert!(!background.is_cancelled());
    assert!(!shutdown.new_turn().is_cancelled());
    let child = Arc::new(BlockedModel::new());
    struct WaitingParent {
        calls: AtomicUsize,
        started: Notify,
    }
    #[async_trait]
    impl ModelProvider for WaitingParent {
        async fn complete(&self, _req: ModelRequest) -> Result<ModelResponse, AgentError> {
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                Ok(ModelResponse {
                    message: Msg::Assistant {
                        content: String::new(),
                        tool_calls: vec![ToolCall {
                            id: "background-call".into(),
                            name: "task".into(),
                            args: serde_json::json!({"description":"independent child","subagent_type":"worker","run_in_background":true}),
                        }],
                    },
                    usage: Usage::default(),
                })
            } else {
                self.started.notify_one();
                std::future::pending().await
            }
        }
    }
    let model = Arc::new(WaitingParent {
        calls: AtomicUsize::new(0),
        started: Notify::new(),
    });
    let agent = parent(model.clone(), child.clone(), vec![], true, &shutdown);
    let running = agent.clone();
    let main = tokio::spawn(async move { running.run_turn("go").await });
    tokio::time::timeout(WAIT, child.started.notified())
        .await
        .unwrap();
    tokio::time::timeout(WAIT, model.started.notified())
        .await
        .unwrap();
    assert!(agent.request_cancel());
    assert_eq!(
        text(
            tokio::time::timeout(WAIT, main)
                .await
                .unwrap()
                .unwrap()
                .unwrap()
        ),
        "Stopped by the user."
    );
    assert!(!child.dropped.load(Ordering::SeqCst));
    assert!(
        agent
            .registry
            .list()
            .iter()
            .any(|(_, status)| *status == AgentStatus::Running)
    );
    let joins = agent.registry.take_background_joins();
    shutdown.cancel();
    for join in joins {
        tokio::time::timeout(WAIT, join).await.unwrap().unwrap();
    }
    assert!(child.dropped.load(Ordering::SeqCst));
    assert!(
        agent
            .registry
            .list()
            .iter()
            .all(|(_, status)| *status == AgentStatus::Failed)
    );
}

#[tokio::test]
async fn an_explicit_closed_session_cannot_inherit_an_unrelated_open_turn() {
    let closed = SessionCancellation::new();
    closed.cancel();
    let ambient = SessionCancellation::new().new_turn();
    let agent = Agent::builder(Arc::new(BlockedModel::new()))
        .interrupt_model_on_cancel(true)
        .cancellation_session(closed)
        .build();
    assert!(matches!(
        ambient.scope(agent.run_turn("must not begin")).await,
        Err(AgentError::Cancelled)
    ));
    assert!(agent.state.lock().unwrap().messages.is_empty());
}

struct CleanupTool {
    started: Notify,
    cleaning: Notify,
    release: Notify,
    finished: AtomicBool,
}
#[async_trait]
impl Tool for CleanupTool {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "cleanup".into(),
            description: String::new(),
            parameters: serde_json::json!({}),
        }
    }
    async fn call(
        &self,
        _args: serde_json::Value,
        ctx: &mut ToolCtx<'_>,
    ) -> Result<ToolOutcome, AgentError> {
        let turn = TurnCancellation::current().unwrap();
        ctx.backend
            .write("/completed-write", "actual committed write")
            .await?;
        self.started.notify_one();
        turn.cancelled().await;
        self.cleaning.notify_one();
        self.release.notified().await;
        self.finished.store(true, Ordering::SeqCst);
        Ok(ToolOutcome::err("cancelled after owned cleanup"))
    }
}
struct CleanupModel;
#[async_trait]
impl ModelProvider for CleanupModel {
    async fn complete(&self, _req: ModelRequest) -> Result<ModelResponse, AgentError> {
        Ok(ModelResponse {
            message: Msg::Assistant {
                content: String::new(),
                tool_calls: vec![ToolCall {
                    id: "cleanup-call".into(),
                    name: "cleanup".into(),
                    args: serde_json::json!({}),
                }],
            },
            usage: Usage::default(),
        })
    }
}

#[tokio::test]
async fn background_stop_retains_running_status_until_owned_cleanup_returns() {
    let tool = Arc::new(CleanupTool {
        started: Notify::new(),
        cleaning: Notify::new(),
        release: Notify::new(),
        finished: AtomicBool::new(false),
    });
    let agent = parent(
        Arc::new(ParentModel {
            calls: AtomicUsize::new(0),
            background: true,
        }),
        Arc::new(CleanupModel),
        vec![tool.clone()],
        true,
        &SessionCancellation::new(),
    );
    agent.run_turn("go").await.unwrap();
    tokio::time::timeout(WAIT, tool.started.notified())
        .await
        .unwrap();
    let name = agent.registry.list()[0].0.clone();
    assert!(agent.registry.abort(&name));
    tokio::time::timeout(WAIT, tool.cleaning.notified())
        .await
        .unwrap();
    assert_eq!(agent.registry.status(&name), Some(AgentStatus::Running));
    assert_eq!(
        agent.registry.progress(&name).as_deref(),
        Some("cancelling")
    );
    assert!(!tool.finished.load(Ordering::SeqCst));
    let joins = agent.registry.take_background_joins();
    tool.release.notify_one();
    for join in joins {
        tokio::time::timeout(WAIT, join).await.unwrap().unwrap();
    }
    assert!(tool.finished.load(Ordering::SeqCst));
    assert_eq!(agent.registry.status(&name), Some(AgentStatus::Failed));
}

#[tokio::test]
async fn foreground_stop_keeps_actual_child_writes_and_waits_for_cleanup() {
    let tool = Arc::new(CleanupTool {
        started: Notify::new(),
        cleaning: Notify::new(),
        release: Notify::new(),
        finished: AtomicBool::new(false),
    });
    let agent = parent(
        Arc::new(ParentModel {
            calls: AtomicUsize::new(0),
            background: false,
        }),
        Arc::new(CleanupModel),
        vec![tool.clone()],
        false,
        &SessionCancellation::new(),
    );
    let running = agent.clone();
    let task = tokio::spawn(async move { running.run_turn("go").await });
    tokio::time::timeout(WAIT, tool.started.notified())
        .await
        .unwrap();
    assert!(agent.request_cancel());
    tokio::time::timeout(WAIT, tool.cleaning.notified())
        .await
        .unwrap();
    assert!(!task.is_finished());
    tool.release.notify_one();
    assert_eq!(
        text(
            tokio::time::timeout(WAIT, task)
                .await
                .unwrap()
                .unwrap()
                .unwrap()
        ),
        "Stopped by the user."
    );
    assert_eq!(
        agent.state.lock().unwrap().files["/completed-write"].content,
        "actual committed write"
    );
    assert!(tool.finished.load(Ordering::SeqCst));
}

#[test]
fn shutdown_parent_depth_is_bounded_without_weakening_existing_sessions() {
    let root = SessionCancellation::new();
    let mut leaf = root.clone();
    for _ in 0..32 {
        leaf = leaf.child().unwrap();
    }
    assert!(leaf.child().is_err());
    assert!(!leaf.is_cancelled());
    root.cancel();
    assert!(leaf.is_cancelled());
}

#[tokio::test]
async fn a_cancelled_turn_end_is_not_restarted_or_given_a_fabricated_stop_answer() {
    struct FinishOnce {
        session: SessionCancellation,
        calls: AtomicUsize,
    }
    #[async_trait]
    impl AgentMiddleware for FinishOnce {
        fn name(&self) -> &str {
            "finish-once"
        }
        async fn turn_end(&self, _state: &mut AgentState) -> Result<(), AgentError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.session.cancel();
            Err(AgentError::Cancelled)
        }
    }
    struct Answer;
    #[async_trait]
    impl ModelProvider for Answer {
        async fn complete(&self, _req: ModelRequest) -> Result<ModelResponse, AgentError> {
            Ok(ModelResponse {
                message: Msg::assistant("actual completed model answer"),
                usage: Usage::default(),
            })
        }
    }
    let session = SessionCancellation::new();
    let finish = Arc::new(FinishOnce {
        session: session.clone(),
        calls: AtomicUsize::new(0),
    });
    let agent = Agent::builder(Arc::new(Answer))
        .interrupt_model_on_cancel(true)
        .cancellation_session(session)
        .middleware(finish.clone())
        .build();
    assert!(matches!(
        agent.run_turn("go").await,
        Err(AgentError::Cancelled)
    ));
    assert_eq!(finish.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        agent.state.lock().unwrap().messages,
        vec![
            Msg::user("go"),
            Msg::assistant("actual completed model answer")
        ]
    );
    assert!(!agent.is_turn_active());
}
