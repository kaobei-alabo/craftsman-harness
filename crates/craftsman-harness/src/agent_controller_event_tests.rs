//! Benign configured-controller flows and direct event ownership checks.
//! No private MAIN callback, budget fault or retry-boundary scenario.

use super::*;
use crate::provenance::*;
use crate::testing::MemCheckpointer;
use crate::{CompletionAction, CompletionAfterCheck, CompletionCheck};

fn source(text: &str) -> SourceRef {
    SourceRef::new(Arc::new(text.to_owned()))
}

struct Model {
    calls: AtomicU64,
}
#[async_trait::async_trait]
impl ModelProvider for Model {
    async fn complete(&self, _: ModelRequest) -> Result<crate::ModelResponse, AgentError> {
        unreachable!("this fixture uses the tracked provider path")
    }
    async fn complete_with_view(
        &self,
        request: TrackedModelRequest,
    ) -> Result<ModelDelivery, AgentError> {
        assert_eq!(
            request.recorded().messages.len(),
            request.message_inputs().len()
        );
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(ModelDelivery::new(
            crate::ModelResponse {
                message: Msg::assistant("actual fixture answer"),
                usage: crate::Usage {
                    input_tokens: 7,
                    output_tokens: 3,
                },
            },
            source("fixture received reply"),
        ))
    }
}

struct Completion {
    stop: bool,
    calls: AtomicU64,
    origin: SourceRef,
}
#[async_trait::async_trait]
impl CompletionCheck for Completion {
    fn source_for_view(&self) -> Option<SourceRef> {
        Some(self.origin.clone())
    }
    async fn before_finish(&self, _: &AgentState) -> Result<CompletionAction, AgentError> {
        let previous = self.calls.fetch_add(1, Ordering::SeqCst);
        if self.stop {
            Ok(CompletionAction::Stop {
                reason: "configured completion refusal".into(),
            })
        } else if previous == 0 {
            Ok(CompletionAction::Check {
                tool: "check".into(),
                args: serde_json::json!({}),
            })
        } else {
            Ok(CompletionAction::Ready)
        }
    }
    async fn after_check(&self, state: &AgentState) -> Result<CompletionAfterCheck, AgentError> {
        assert!(
            matches!(state.messages.last(), Some(Msg::Tool { content, is_error: false, .. }) if content == "actual check output")
        );
        Ok(CompletionAfterCheck::Continue)
    }
}

struct Check {
    calls: AtomicU64,
    saves: Arc<MemCheckpointer>,
}
#[async_trait::async_trait]
impl Tool for Check {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "check".into(),
            description: "ordinary controller fixture".into(),
            parameters: serde_json::json!({"type":"object"}),
        }
    }
    async fn call(
        &self,
        _: serde_json::Value,
        _: &mut ToolCtx<'_>,
    ) -> Result<ToolOutcome, AgentError> {
        let saves = self.saves.saves.lock().unwrap();
        assert!(
            matches!(saves.last().unwrap().2.messages.last(), Some(Msg::Assistant { tool_calls, .. }) if tool_calls.len() == 1 && tool_calls[0].name == "check")
        );
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(ToolOutcome::ok("actual check output"))
    }
}

struct IdlePolicy {
    origin: SourceRef,
    saves: Arc<MemCheckpointer>,
    events: Mutex<Vec<ActualIdleCompaction>>,
    calls: AtomicU64,
}
impl crate::AgentBoundaryPolicy for IdlePolicy {
    fn view_support(&self) -> ViewSupport {
        ViewSupport::Tracked
    }
    fn source_for_view(&self) -> Option<SourceRef> {
        Some(self.origin.clone())
    }
    fn user_input_with_view(
        &self,
        state: &mut AgentState,
        event: &ActualBoundaryInput,
    ) -> Result<(), AgentError> {
        assert!(event.in_callback());
        assert_eq!(
            state.messages.get(event.index()),
            Some(event.input().commit().message())
        );
        Ok(())
    }
    fn idle_compaction_committed(&self, state: &mut AgentState) -> Result<(), AgentError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        state
            .custom
            .insert("fixture_idle_observed".into(), serde_json::json!(true));
        Ok(())
    }
    fn idle_compaction_committed_with_view(
        &self,
        state: &mut AgentState,
        event: &ActualIdleCompaction,
    ) -> Result<(), AgentError> {
        assert!(event.in_callback() && event.source().is(&self.origin));
        let before = event
            .before()
            .iter()
            .map(|item| item.commit().message().clone())
            .collect::<Vec<_>>();
        let after = event
            .after()
            .iter()
            .map(|item| item.commit().message().clone())
            .collect::<Vec<_>>();
        assert_eq!(
            before,
            self.saves.saves.lock().unwrap().last().unwrap().2.messages
        );
        assert_eq!(after, state.messages);
        assert!(!state.custom.contains_key("fixture_idle_observed"));
        self.events.lock().unwrap().push(event.clone());
        self.idle_compaction_committed(state)
    }
}

#[derive(Default)]
struct Boundary {
    owner: Mutex<Option<ActualAgentOwner>>,
    completion: Option<Arc<Completion>>,
    idle: Option<Arc<IdlePolicy>>,
    stopped: Mutex<Vec<ActualRetainedCommit>>,
}
impl Boundary {
    fn check_owner(&self, owner: &ActualAgentOwner) {
        assert!(
            self.owner
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .same_agent(owner)
        );
    }
}
#[async_trait::async_trait]
impl ModelViewBoundary for Boundary {
    fn bind_agent(&self, owner: &ActualAgentOwner) -> Result<(), AgentError> {
        let mut held = self.owner.lock().unwrap();
        assert!(held.is_none());
        *held = Some(owner.clone());
        Ok(())
    }
    fn validate_controller_sources(
        &self,
        bindings: &ActualControllerBindings,
    ) -> Result<(), AgentError> {
        self.check_owner(bindings.owner());
        assert_eq!(bindings.has_completion(), self.completion.is_some());
        assert_eq!(bindings.has_policy(), self.idle.is_some());
        if let Some(expected) = &self.completion {
            let actual: Arc<dyn CompletionCheck> = expected.clone();
            assert!(bindings.completion_matches(&actual));
            assert!(bindings.completion_source().unwrap().is(&expected.origin));
        }
        if let Some(expected) = &self.idle {
            let actual: Arc<dyn crate::AgentBoundaryPolicy> = expected.clone();
            assert!(bindings.policy_matches(&actual));
            assert!(bindings.policy_source().unwrap().is(&expected.origin));
        }
        Ok(())
    }
    async fn system_sources(&self) -> Result<Vec<SourceRef>, AgentError> {
        Ok(vec![])
    }
    async fn system_addition(&self, parts: &[SourcePart]) -> Result<SourceRef, AgentError> {
        assert!(
            parts
                .iter()
                .all(|part| matches!(part.from, PartFrom::Fixed))
        );
        Ok(source("actual fixed compaction system"))
    }
    async fn host_message(&self, part: &SourcePart) -> Result<SourceRef, AgentError> {
        assert!(matches!(part.from, PartFrom::Fixed));
        Ok(source("actual fixed compaction request"))
    }
    async fn tool_source(&self, _: &ToolDef) -> Result<SourceRef, AgentError> {
        Ok(source("fixture definition"))
    }
    async fn commit_model(&self, _: SourceRef, _: &Msg) -> Result<SourceRef, AgentError> {
        unreachable!("actual retained commit required")
    }
    async fn observe_tool_result(
        &self,
        _: ObservedToolResult<'_>,
    ) -> Result<SourceRef, AgentError> {
        unreachable!("actual retained outcome required")
    }
    async fn commit_retained_model(
        &self,
        commit: &ActualRetainedCommit,
    ) -> Result<SourceRef, AgentError> {
        self.check_owner(commit.owner());
        assert_eq!(commit.event(), RetainedEvent::ModelReply);
        assert_eq!(commit.received().unwrap().message(), commit.message());
        Ok(source("actual committed reply"))
    }
    async fn observe_retained_tool(&self, fact: &ActualToolFact) -> Result<SourceRef, AgentError> {
        assert_eq!(fact.terminals().len(), 1);
        assert_eq!(fact.returned().unwrap().content, "actual check output");
        self.check_owner(fact.commit().unwrap().owner());
        Ok(source("actual check return"))
    }
    async fn observe_host_scheduled(
        &self,
        scheduled: &ActualHostScheduledCall,
    ) -> Result<SourceRef, AgentError> {
        self.check_owner(scheduled.owner());
        let completion: Arc<dyn CompletionCheck> = self.completion.as_ref().unwrap().clone();
        assert!(scheduled.same_controller(&completion));
        assert_eq!(
            scheduled.commit().event(),
            RetainedEvent::Framework(FrameworkCommit::CompletionCheck)
        );
        assert_eq!(scheduled.call().name, "check");
        Ok(source("actual hosted check"))
    }
    async fn before_terminal(
        &self,
        invocation: &ActualToolInvocation,
    ) -> Result<Option<SourceRef>, AgentError> {
        let actual = invocation.planned().host_scheduled().unwrap();
        self.check_owner(actual.owner());
        assert_eq!(actual.call(), invocation.call());
        let controller: Arc<dyn CompletionCheck> = self.completion.as_ref().unwrap().clone();
        assert!(actual.same_controller(&controller));
        Ok(None)
    }
    async fn observe_retained_framework(
        &self,
        commit: &ActualRetainedCommit,
    ) -> Result<SourceRef, AgentError> {
        self.check_owner(commit.owner());
        assert_eq!(
            commit.event(),
            RetainedEvent::Framework(FrameworkCommit::CompletionStop)
        );
        assert!(
            matches!(commit.message(), Msg::Assistant { tool_calls, .. } if tool_calls.is_empty())
        );
        self.stopped.lock().unwrap().push(commit.clone());
        Ok(source("actual fixed completion stop"))
    }
    async fn staged_host_message_source(
        &self,
        stage: &ActualStagedHostMessage,
    ) -> Result<SourceRef, AgentError> {
        self.check_owner(stage.owner());
        Ok(source("actual compaction instruction"))
    }
    async fn staged_summary_source(
        &self,
        stage: &ActualStagedSummary,
    ) -> Result<SourceRef, AgentError> {
        self.check_owner(stage.owner());
        assert!(stage.received().owner().same_agent(stage.owner()));
        Ok(source("actual staged summary"))
    }
    async fn observe_retained_summary(
        &self,
        commit: &ActualRetainedCommit,
    ) -> Result<SourceRef, AgentError> {
        self.check_owner(commit.owner());
        assert_eq!(commit.event(), RetainedEvent::Summary);
        assert_eq!(commit.summary().unwrap().message(), commit.message());
        Ok(source("actual retained summary"))
    }
}

#[tokio::test]
async fn actual_completion_window_remains_live_until_its_real_check_and_close() {
    let saves = Arc::new(MemCheckpointer::default());
    let completion = Arc::new(Completion {
        stop: false,
        calls: AtomicU64::new(0),
        origin: source("registered completion"),
    });
    let check = Arc::new(Check {
        calls: AtomicU64::new(0),
        saves: saves.clone(),
    });
    let model = Arc::new(Model {
        calls: AtomicU64::new(0),
    });
    let boundary = Arc::new(Boundary {
        completion: Some(completion.clone()),
        ..Boundary::default()
    });
    let agent = Agent::builder(model.clone())
        .model_view_boundary(Some(boundary))
        .completion_check(completion.clone())
        .tool(check.clone())
        .checkpointer(saves.clone())
        .durable_turn_checkpoints(true)
        .build();
    let TurnResult::Done(result) = agent
        .run_turn_with_view("actual input", source("actual input owner"))
        .await
        .unwrap()
    else {
        panic!("unexpected suspension")
    };
    assert_eq!(result.final_text(), "actual fixture answer");
    assert_eq!(model.calls.load(Ordering::SeqCst), 2);
    assert_eq!(check.calls.load(Ordering::SeqCst), 1);
    assert_eq!(completion.calls.load(Ordering::SeqCst), 2);
    assert_eq!(agent.actual_host_scheduled_calls().unwrap().len(), 1);
    assert!(!agent.is_turn_active());
    assert!(
        agent
            .recorded_sources()
            .unwrap()
            .iter()
            .all(Option::is_some)
    );
    assert_eq!(
        saves.saves.lock().unwrap().last().unwrap().2.messages,
        agent.snapshot().messages
    );
    // Direct owner closure: no reopening or skipping the actual turn liveness check.
    let _activity = agent.begin_turn().unwrap();
    assert!(matches!(
        agent.turn_boundary_action(),
        TurnBoundaryAction::Finish
    ));
    assert!(model_view::invocation_turn_live(&agent.turn_input.lock().unwrap()).is_ok());
    assert!(matches!(
        agent.completion_boundary_action(),
        TurnBoundaryAction::Finish
    ));
    assert!(model_view::invocation_turn_live(&agent.turn_input.lock().unwrap()).is_err());
}

#[tokio::test]
async fn configured_completion_stop_retains_its_exact_framework_event_and_real_save() {
    let saves = Arc::new(MemCheckpointer::default());
    let completion = Arc::new(Completion {
        stop: true,
        calls: AtomicU64::new(0),
        origin: source("registered stop controller"),
    });
    let boundary = Arc::new(Boundary {
        completion: Some(completion.clone()),
        ..Boundary::default()
    });
    let model = Arc::new(Model {
        calls: AtomicU64::new(0),
    });
    let agent = Agent::builder(model.clone())
        .model_view_boundary(Some(boundary.clone()))
        .completion_check(completion)
        .checkpointer(saves.clone())
        .durable_turn_checkpoints(true)
        .build();
    let error = agent
        .run_turn_with_view("retained input", source("actual input owner"))
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        AgentError::State("configured completion refusal".into()).to_string()
    );
    let retained = agent.actual_retained_commits().unwrap();
    let note = retained
        .iter()
        .find(|commit| commit.event() == RetainedEvent::Framework(FrameworkCommit::CompletionStop))
        .unwrap();
    assert_eq!(
        note.message(),
        &Msg::assistant(
            "The declared completion check did not accept this turn. Completed effects and check results are retained."
        )
    );
    assert_eq!(
        saves.saves.lock().unwrap().last().unwrap().2.messages,
        agent.snapshot().messages
    );
    assert_eq!(model.calls.load(Ordering::SeqCst), 1);
    assert!(agent.actual_terminal_outcomes().unwrap().is_empty());
    assert_eq!(boundary.stopped.lock().unwrap().len(), 1);
    assert!(
        agent
            .recorded_sources()
            .unwrap()
            .iter()
            .all(Option::is_some)
    );
}

#[tokio::test]
async fn idle_controller_observes_actual_replacement_once_before_the_final_save() {
    let saves = Arc::new(MemCheckpointer::default());
    let controller = Arc::new(IdlePolicy {
        origin: source("registered idle controller"),
        saves: saves.clone(),
        events: Mutex::new(Vec::new()),
        calls: AtomicU64::new(0),
    });
    let boundary = Arc::new(Boundary {
        idle: Some(controller.clone()),
        ..Boundary::default()
    });
    let model = Arc::new(Model {
        calls: AtomicU64::new(0),
    });
    let summarizer = Arc::new(Model {
        calls: AtomicU64::new(0),
    });
    let compaction = Arc::new(crate::builtin::CompactionMiddleware::new(
        summarizer.clone(),
        crate::builtin::CompactionTrigger::MessageCount {
            threshold_msgs: 100,
            keep_tail_msgs: 20,
        },
    ));
    let agent = Agent::builder(model.clone())
        .model_view_boundary(Some(boundary))
        .boundary_policy(controller.clone())
        .middleware(compaction.clone())
        .checkpointer(saves.clone())
        .durable_turn_checkpoints(true)
        .build();
    for input in ["first actual input", "second actual input"] {
        agent
            .run_turn_with_view(input, source("actual input owner"))
            .await
            .unwrap();
    }
    let before = agent.actual_sourced_messages().unwrap();
    let save_count = saves.saves.lock().unwrap().len();
    assert_eq!(
        agent.compact_idle_with_view(&compaction).await.unwrap(),
        Some((4, 4))
    );
    assert_eq!(controller.calls.load(Ordering::SeqCst), 1);
    assert_eq!(summarizer.calls.load(Ordering::SeqCst), 1);
    assert_eq!(model.calls.load(Ordering::SeqCst), 2);
    let events = controller.events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(!events[0].in_callback());
    let actual: Arc<dyn crate::AgentBoundaryPolicy> = controller.clone();
    assert!(events[0].same_controller(&actual));
    assert!(
        events[0]
            .before()
            .iter()
            .zip(&before)
            .all(
                |(event, original)| event.commit().same_commit(original.commit())
                    && event.source().is(original.source())
            )
    );
    assert_eq!(
        events[0].after()[0].commit().event(),
        RetainedEvent::Summary
    );
    assert!(
        events[0]
            .after()
            .iter()
            .skip(1)
            .zip(before.iter().skip(1))
            .all(
                |(event, original)| event.commit().same_commit(original.commit())
                    && event.source().is(original.source())
            )
    );
    assert!(agent.actual_idle_compactions().unwrap()[0].same_compaction(&events[0]));
    let saved = saves.saves.lock().unwrap();
    assert_eq!(saved.len(), save_count + 2);
    assert_eq!(saved.last().unwrap().2.messages, agent.snapshot().messages);
    assert_eq!(
        saved.last().unwrap().2.custom["fixture_idle_observed"],
        serde_json::json!(true)
    );
    assert!(
        agent
            .recorded_sources()
            .unwrap()
            .iter()
            .all(Option::is_some)
    );
    assert!(!agent.is_turn_active());
}
