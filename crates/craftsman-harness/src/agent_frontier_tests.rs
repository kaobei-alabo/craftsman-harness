//! Ordinary fake-provider and direct event/queue checks. No private MAIN
//! callback, budget fault, retry-boundary or close-between-retry scenario.

use super::*;
use crate::provenance::*;
use crate::testing::{MemCheckpointer, NoopTool};

fn source(name: &str) -> SourceRef {
    SourceRef::new(Arc::new(name.to_owned()))
}

struct Model(Mutex<usize>);

#[async_trait::async_trait]
impl ModelProvider for Model {
    async fn complete(&self, _: ModelRequest) -> Result<crate::ModelResponse, AgentError> {
        unreachable!("a selected ordinary fixture uses its view")
    }

    async fn complete_with_view(
        &self,
        request: TrackedModelRequest,
    ) -> Result<ModelDelivery, AgentError> {
        assert_eq!(
            request.recorded().messages.len(),
            request.retained_commits().len()
        );
        for (message, commit) in request
            .recorded()
            .messages
            .iter()
            .zip(request.retained_commits())
        {
            assert_eq!(message, commit.message());
        }
        let mut index = self.0.lock().unwrap();
        let message = if *index == 0 {
            Msg::Assistant {
                content: "actual reply".into(),
                tool_calls: vec![ToolCall {
                    id: "actual-call".into(),
                    name: "noop".into(),
                    args: serde_json::json!({}),
                }],
            }
        } else {
            Msg::assistant("done")
        };
        *index += 1;
        Ok(ModelDelivery::new(
            crate::ModelResponse {
                message,
                usage: crate::Usage {
                    input_tokens: 7,
                    output_tokens: 3,
                },
            },
            source("fixture received"),
        ))
    }
}

#[derive(Default)]
struct Boundary {
    owner: Mutex<Option<ActualAgentOwner>>,
    closed: AtomicU64,
}

#[async_trait::async_trait]
impl ModelViewBoundary for Boundary {
    fn bind_agent(&self, owner: &ActualAgentOwner) -> Result<(), AgentError> {
        let mut held = self.owner.lock().unwrap();
        assert!(held.is_none());
        *held = Some(owner.clone());
        Ok(())
    }

    fn close(&self) {
        self.closed.fetch_add(1, Ordering::SeqCst);
    }
    async fn system_sources(&self) -> Result<Vec<SourceRef>, AgentError> {
        Ok(vec![])
    }
    async fn tool_source(&self, _: &ToolDef) -> Result<SourceRef, AgentError> {
        Ok(source("definition"))
    }
    async fn commit_model(&self, _: SourceRef, _: &Msg) -> Result<SourceRef, AgentError> {
        unreachable!("the fixture requires an actual retained event")
    }
    async fn observe_tool_result(
        &self,
        _: ObservedToolResult<'_>,
    ) -> Result<SourceRef, AgentError> {
        unreachable!("the fixture requires actual terminal/chain facts")
    }
    async fn commit_retained_model(
        &self,
        commit: &ActualRetainedCommit,
    ) -> Result<SourceRef, AgentError> {
        assert_eq!(commit.event(), RetainedEvent::ModelReply);
        assert!(
            self.owner
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .same_agent(commit.owner())
        );
        assert_eq!(commit.received().unwrap().usage().input_tokens, 7);
        Ok(source("committed model"))
    }
    async fn observe_retained_tool(&self, fact: &ActualToolFact) -> Result<SourceRef, AgentError> {
        assert_eq!(fact.terminals().len(), 1);
        assert_eq!(fact.returned().unwrap().content, "noop");
        assert_eq!(fact.commit().unwrap().event(), RetainedEvent::ToolResult);
        Ok(source("actual tool"))
    }
    async fn observe_retained_framework(
        &self,
        commit: &ActualRetainedCommit,
    ) -> Result<SourceRef, AgentError> {
        assert!(matches!(
            commit.event(),
            RetainedEvent::Framework(FrameworkCommit::CancelNote)
        ));
        Ok(source("actual closing note"))
    }
    async fn before_terminal(
        &self,
        invocation: &ActualToolInvocation,
    ) -> Result<Option<SourceRef>, AgentError> {
        assert_eq!(invocation.call().id, "actual-call");
        assert_eq!(invocation.call().name, "noop");
        assert!(!invocation.resumed());
        Ok(Some(source("checked actual invocation")))
    }
}

fn selected(boundary: &Arc<Boundary>) -> Agent {
    Agent::builder(Arc::new(Model(Mutex::new(0))))
        .tool(Arc::new(NoopTool::new("noop")))
        .model_view_boundary(Some(boundary.clone()))
        .build()
}

#[tokio::test]
async fn ordinary_view_retains_real_reply_commit_and_terminal_facts() {
    let boundary = Arc::new(Boundary::default());
    let agent = selected(&boundary);
    let result = agent
        .run_turn_with_view("actual input", source("input"))
        .await
        .unwrap();
    let TurnResult::Done(output) = result else {
        panic!("fixture did not finish");
    };
    assert_eq!(output.final_text(), "done");
    let received = agent.actual_received_replies().unwrap();
    assert_eq!(received.len(), 2);
    assert_eq!(received[0].usage().output_tokens, 3);
    let commits = agent.actual_retained_commits().unwrap();
    assert_eq!(commits.len(), 4);
    let messages = agent.state.lock().unwrap().messages.clone();
    for (commit, message) in commits.iter().zip(&messages) {
        assert_eq!(commit.message(), message);
        assert!(commit.owner().same_agent(received[0].owner()));
    }
    let terminals = agent.actual_terminal_outcomes().unwrap();
    assert_eq!(terminals.len(), 1);
    assert_eq!(
        terminals[0].reached().result.as_ref().unwrap().content,
        "noop"
    );
    assert!(terminals[0].operation_source().is_some());
    let chains = agent.actual_tool_facts().unwrap();
    assert_eq!(chains.len(), 2);
    assert!(chains[0].commit().is_none());
    assert_eq!(chains[1].commit().unwrap().message(), &messages[2]);
    assert!(
        chains[0].terminals()[0]
            .invocation()
            .same_invocation(chains[1].terminals()[0].invocation())
    );
    assert_eq!(boundary.closed.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn immutable_event_check_rejects_status_id_role_or_text_changes() {
    let boundary = Arc::new(Boundary::default());
    let agent = selected(&boundary);
    agent
        .run_turn_with_view("actual input", source("input"))
        .await
        .unwrap();
    let original = agent.state.lock().unwrap().messages.clone();
    let view = agent.model_view.as_ref().unwrap();
    assert!(view.matches(&original));
    let mut changed = original.clone();
    if let Msg::Tool { is_error, .. } = &mut changed[2] {
        *is_error = true;
    }
    assert!(!view.matches(&changed));
    let mut changed = original.clone();
    if let Msg::Tool { tool_call_id, .. } = &mut changed[2] {
        *tool_call_id = "another".into();
    }
    assert!(!view.matches(&changed));
    let mut changed = original.clone();
    changed[2] = Msg::user("noop");
    assert!(!view.matches(&changed));
    let mut changed = original;
    if let Msg::Tool { content, .. } = &mut changed[2] {
        content.push('!');
    }
    assert!(!view.matches(&changed));
}

#[tokio::test]
async fn selected_queue_cancellation_keeps_each_accepted_source_and_real_save() {
    let boundary = Arc::new(Boundary::default());
    let saves = Arc::new(MemCheckpointer::default());
    let agent = Agent::builder(Arc::new(Model(Mutex::new(0))))
        .model_view_boundary(Some(boundary.clone()))
        .checkpointer(saves.clone())
        .durable_turn_checkpoints(true)
        .build();
    // Direct queue/activity primitive; no model or callback is started.
    let _activity = agent.begin_turn().unwrap();
    let first = source("first actual input");
    let second = source("second actual input");
    assert!(agent.steer_with_view("first", first.clone()));
    assert!(agent.steer_with_view("second", second.clone()));
    assert!(agent.request_cancel());
    assert!(!agent.steer_with_view("late", source("late")));
    assert_eq!(agent.turn_input.lock().unwrap().pending.len(), 2);
    agent.finish_cancelled().await.unwrap();
    let messages = agent.state.lock().unwrap().messages.clone();
    assert_eq!(
        messages,
        [
            Msg::user("first"),
            Msg::user("second"),
            Msg::assistant("Stopped by the user.")
        ]
    );
    assert_eq!(
        saves.saves.lock().unwrap().last().unwrap().2.messages,
        messages
    );
    let sources = agent.recorded_sources().unwrap();
    assert!(sources[0].as_ref().unwrap().is(&first));
    assert!(sources[1].as_ref().unwrap().is(&second));
}

#[tokio::test]
async fn typed_activity_rejects_another_entry_before_source_lock_access() {
    let boundary = Arc::new(Boundary::default());
    let agent = selected(&boundary);
    let _activity = agent.begin_turn().unwrap();
    agent
        .state
        .lock()
        .unwrap()
        .messages
        .push(Msg::user("unbound state"));
    let Err(AgentError::State(error)) = agent.run_turn_with_view("second", source("second")).await
    else {
        panic!("active ownership was not rejected");
    };
    assert_eq!(error, "an agent turn is already running");
    // This is an ownership primitive, not a concurrent main-loop fault probe.
    assert!(agent.is_turn_active());
}
