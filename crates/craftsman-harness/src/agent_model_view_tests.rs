//! Ordinary turns through the optional source view boundary, with a scripted
//! provider and a recording boundary. Nothing here contacts a model service.

use super::model_view::Slot;
use super::*;
use crate::provenance::{
    ModelDelivery, ModelViewBoundary, ObservedToolResult, SourceRef, TrackedModelRequest,
};
use crate::testing::{MemCheckpointer, NoopTool, ScriptedModel};
use crate::{ModelResponse, ToolDef, Usage};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

struct Tag(String);

fn source(tag: &str) -> SourceRef {
    SourceRef::new(Arc::new(Tag(tag.to_owned())))
}

fn tag(source: &SourceRef) -> String {
    source.downcast::<Tag>().expect("test payload").0.clone()
}

/// What the provider was handed for one request.
struct Seen {
    recorded: ModelRequest,
    system: Vec<String>,
    messages: Vec<String>,
    tools: Vec<String>,
}

struct ViewModel {
    script: Mutex<std::vec::IntoIter<Msg>>,
    seen: Mutex<Vec<Seen>>,
    recorded_path_calls: AtomicUsize,
}

impl ViewModel {
    fn new(script: Vec<Msg>) -> Arc<Self> {
        Arc::new(Self {
            script: Mutex::new(script.into_iter()),
            seen: Mutex::new(Vec::new()),
            recorded_path_calls: AtomicUsize::new(0),
        })
    }
}

#[async_trait::async_trait]
impl ModelProvider for ViewModel {
    async fn complete(&self, _: ModelRequest) -> Result<ModelResponse, AgentError> {
        self.recorded_path_calls.fetch_add(1, Ordering::SeqCst);
        Err(AgentError::Model("the recorded-text path was used".into()))
    }

    async fn complete_with_view(
        &self,
        req: TrackedModelRequest,
    ) -> Result<ModelDelivery, AgentError> {
        let mut seen = self.seen.lock().unwrap();
        let receipt = source(&format!("receipt:{}", seen.len()));
        seen.push(Seen {
            recorded: req.recorded().clone(),
            system: req.system_sources().iter().map(tag).collect(),
            messages: req.message_sources().iter().map(tag).collect(),
            tools: req.tool_sources().iter().map(tag).collect(),
        });
        match self.script.lock().unwrap().next() {
            Some(message) => Ok(ModelDelivery::new(
                ModelResponse {
                    message,
                    usage: Usage {
                        input_tokens: 7,
                        output_tokens: 3,
                    },
                },
                receipt,
            )),
            None => Err(AgentError::Model("scripted model exhausted".into())),
        }
    }
}

#[derive(Default)]
struct Boundary {
    events: Mutex<Vec<String>>,
    fail_tool: AtomicBool,
    fail_model: AtomicBool,
}

#[async_trait::async_trait]
impl ModelViewBoundary for Boundary {
    async fn system_sources(&self) -> Result<Vec<SourceRef>, AgentError> {
        Ok(vec![source("system:static"), source("system:file")])
    }

    async fn tool_source(&self, definition: &ToolDef) -> Result<SourceRef, AgentError> {
        Ok(source(&format!("definition:{}", definition.name)))
    }

    async fn commit_model(
        &self,
        receipt: SourceRef,
        committed: &Msg,
    ) -> Result<SourceRef, AgentError> {
        let receipt = tag(&receipt);
        let calls = match committed {
            Msg::Assistant { tool_calls, .. } => tool_calls.len(),
            _ => panic!("only a committed reply is reported here"),
        };
        self.events
            .lock()
            .unwrap()
            .push(format!("model {receipt} calls={calls}"));
        if self.fail_model.load(Ordering::SeqCst) {
            return Err(AgentError::State("no view for this reply".into()));
        }
        Ok(source(&format!("model:{receipt}")))
    }

    async fn observe_tool_result(
        &self,
        result: ObservedToolResult<'_>,
    ) -> Result<SourceRef, AgentError> {
        self.events.lock().unwrap().push(format!(
            "tool {} {} {:?} error={}",
            result.call_id, result.tool, result.content, result.is_error
        ));
        if self.fail_tool.load(Ordering::SeqCst) {
            return Err(AgentError::State("no view for this result".into()));
        }
        Ok(source(&format!("result:{}", result.call_id)))
    }
}

fn call(id: &str, name: &str) -> Msg {
    Msg::Assistant {
        content: String::new(),
        tool_calls: vec![ToolCall {
            id: id.into(),
            name: name.into(),
            args: serde_json::json!({}),
        }],
    }
}

fn selected(
    model: &Arc<ViewModel>,
    boundary: &Arc<Boundary>,
    saves: &Arc<MemCheckpointer>,
) -> Agent {
    Agent::builder(model.clone())
        .tool(Arc::new(NoopTool::new("noop")))
        .model_view_boundary(Some(boundary.clone()))
        .checkpointer(saves.clone())
        // Save after every accepted input and tool result, as the owned
        // terminal does. Without this, a result is saved with the next reply.
        .durable_turn_checkpoints(true)
        .build()
}

fn recorded(agent: &Agent) -> Vec<Msg> {
    agent.state.lock().unwrap().messages.clone()
}

#[tokio::test]
async fn an_agent_without_a_boundary_keeps_its_paths_and_has_no_typed_entry() {
    let model = Arc::new(ScriptedModel::new(vec![Msg::assistant("done")]));
    let agent = Agent::builder(model.clone())
        .system_prompt("system")
        .model_view_boundary(None)
        .build();
    // The typed entry is refused before anything is recorded.
    assert!(matches!(
        agent.run_turn_with_view("raw", source("input")).await,
        Err(AgentError::State(_))
    ));
    assert!(recorded(&agent).is_empty());
    // The ordinary turn is unchanged: one recorded-text request, as built.
    let TurnResult::Done(output) = agent.run_turn("raw").await.unwrap() else {
        panic!("turn did not finish");
    };
    assert_eq!(output.final_text(), "done");
    let requests = model.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].system, "system");
    assert_eq!(requests[0].messages, vec![Msg::user("raw")]);
    // A provider with no view path refuses a tracked request it is handed.
    let tracked = TrackedModelRequest {
        request: requests[0].clone(),
        system: vec![],
        messages: vec![source("input")],
        tools: vec![],
    };
    drop(requests);
    assert!(matches!(
        model.complete_with_view(tracked).await,
        Err(AgentError::Model(_))
    ));
    assert_eq!(model.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_selected_turn_records_the_real_conversation_and_sends_only_sources() {
    let model = ViewModel::new(vec![call("c1", "noop"), Msg::assistant("final answer")]);
    let boundary = Arc::new(Boundary::default());
    let saves = Arc::new(MemCheckpointer::default());
    let agent = selected(&model, &boundary, &saves);

    let TurnResult::Done(output) = agent
        .run_turn_with_view("raw user text", source("input:1"))
        .await
        .unwrap()
    else {
        panic!("turn did not finish");
    };
    assert_eq!(output.final_text(), "final answer");

    // The recorded conversation is the real one, in order.
    let messages = recorded(&agent);
    assert_eq!(
        messages,
        vec![
            Msg::user("raw user text"),
            call("c1", "noop"),
            Msg::Tool {
                tool_call_id: "c1".into(),
                content: "noop".into(),
                is_error: false,
            },
            Msg::assistant("final answer"),
        ]
    );
    // Usage of the real reply is kept.
    assert_eq!(
        agent.state.lock().unwrap().last_usage,
        Some(Usage {
            input_tokens: 7,
            output_tokens: 3
        })
    );
    // Checkpoints hold the same recorded messages.
    let saved = saves.saves.lock().unwrap();
    assert_eq!(saved.last().unwrap().2.messages, messages);
    drop(saved);

    // Each request carried one source per recorded message, in order, and
    // the sources of the system channel and of each tool definition.
    let seen = model.seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].messages, ["input:1"]);
    assert_eq!(
        seen[1].messages,
        ["input:1", "model:receipt:0", "result:c1"]
    );
    for request in seen.iter() {
        assert_eq!(request.system, ["system:static", "system:file"]);
        assert_eq!(request.tools, ["definition:noop"]);
        assert_eq!(request.recorded.system, "");
        assert_eq!(request.recorded.messages.len(), request.messages.len());
    }
    assert_eq!(seen[1].recorded.messages, messages[..3]);
    assert_eq!(model.recorded_path_calls.load(Ordering::SeqCst), 0);
    // The boundary heard of each real event once, after it was recorded.
    assert_eq!(
        *boundary.events.lock().unwrap(),
        [
            "model receipt:0 calls=1",
            "tool c1 noop \"noop\" error=false",
            "model receipt:1 calls=0",
        ]
    );

    // A second turn continues from the same sources.
    let model_sources = agent
        .model_view
        .as_ref()
        .unwrap()
        .sources
        .lock()
        .unwrap()
        .len();
    assert_eq!(model_sources, 4);
}

#[tokio::test]
async fn entries_without_a_source_are_refused_on_a_selected_agent_before_any_effect() {
    let model = ViewModel::new(vec![Msg::assistant("unused")]);
    let boundary = Arc::new(Boundary::default());
    let saves = Arc::new(MemCheckpointer::default());
    let agent = selected(&model, &boundary, &saves);
    let other = selected(&model, &boundary, &saves);
    let plain = Agent::builder(model.clone()).build();

    assert!(matches!(
        agent.run_turn("raw").await,
        Err(AgentError::State(_))
    ));
    assert!(matches!(
        agent
            .resume(crate::interrupt::HitlAction::Approve, "denied")
            .await,
        Err(AgentError::State(_))
    ));
    assert!(!agent.steer("more"));
    assert!(matches!(
        agent.load_thread("default").await,
        Err(AgentError::State(_))
    ));
    assert!(agent.inherit_conversation(&other).is_err());
    assert!(agent.inherit_conversation(&plain).is_err());
    assert!(plain.inherit_conversation(&agent).is_err());
    let compaction = crate::builtin::compaction::CompactionMiddleware::new(
        model.clone(),
        crate::builtin::compaction::CompactionTrigger::MessageCount {
            threshold_msgs: 10,
            keep_tail_msgs: 2,
        },
    );
    assert!(agent.compact_idle(&compaction).await.is_err());

    // Nothing was recorded, saved or sent.
    assert!(recorded(&agent).is_empty() && saves.saves.lock().unwrap().is_empty());
    assert!(model.seen.lock().unwrap().is_empty());
    assert_eq!(model.recorded_path_calls.load(Ordering::SeqCst), 0);
    assert!(boundary.events.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_selected_agent_with_a_part_that_has_no_source_path_refuses() {
    let boundary = Arc::new(Boundary::default());
    let model = ViewModel::new(vec![Msg::assistant("unused")]);
    let base = || Agent::builder(model.clone()).model_view_boundary(Some(boundary.clone()));
    // System text outside the boundary, and any middleware.
    let with_system = base().system_prompt("untracked system text").build();
    let with_middleware = base()
        .middleware(Arc::new(crate::builtin::todos::TodoListMiddleware::new()))
        .build();
    for agent in [&with_system, &with_middleware] {
        assert!(matches!(
            agent.run_turn_with_view("raw", source("input")).await,
            Err(AgentError::State(_))
        ));
        assert!(recorded(agent).is_empty());
    }
    // A recorded message that arrived without a source.
    let agent = base().build();
    agent
        .state
        .lock()
        .unwrap()
        .messages
        .push(Msg::user("appended by something else"));
    assert!(matches!(
        agent.run_turn_with_view("raw", source("input")).await,
        Err(AgentError::State(_))
    ));
    assert_eq!(recorded(&agent).len(), 1);
    assert!(model.seen.lock().unwrap().is_empty());

    // A provider that has no view path is not given the recorded text.
    let scripted = Arc::new(ScriptedModel::new(vec![Msg::assistant("unused")]));
    let agent = Agent::builder(scripted.clone())
        .model_view_boundary(Some(boundary.clone()))
        .build();
    assert!(matches!(
        agent.run_turn_with_view("raw", source("input")).await,
        Err(AgentError::Model(_))
    ));
    // The accepted input stays recorded; no request was made.
    assert_eq!(recorded(&agent), vec![Msg::user("raw")]);
    assert!(scripted.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_missing_view_after_a_real_tool_result_keeps_the_result_and_stops() {
    let model = ViewModel::new(vec![call("c1", "noop"), Msg::assistant("never sampled")]);
    let boundary = Arc::new(Boundary::default());
    boundary.fail_tool.store(true, Ordering::SeqCst);
    let saves = Arc::new(MemCheckpointer::default());
    let agent = selected(&model, &boundary, &saves);

    let error = agent
        .run_turn_with_view("raw user text", source("input:1"))
        .await
        .err()
        .expect("the turn stops");
    assert!(matches!(error, AgentError::State(_)));
    // The real result is recorded and saved as returned. It is not replaced
    // and nothing says the tool did not run.
    let messages = recorded(&agent);
    assert_eq!(
        messages[2],
        Msg::Tool {
            tool_call_id: "c1".into(),
            content: "noop".into(),
            is_error: false,
        }
    );
    assert_eq!(messages.len(), 3);
    assert_eq!(
        saves.saves.lock().unwrap().last().unwrap().2.messages,
        messages
    );
    assert_eq!(
        agent.state.lock().unwrap().last_usage,
        Some(Usage {
            input_tokens: 7,
            output_tokens: 3
        })
    );
    // No further request was made, and the conversation takes no new turn.
    assert_eq!(model.seen.lock().unwrap().len(), 1);
    boundary.fail_tool.store(false, Ordering::SeqCst);
    assert!(matches!(
        agent.run_turn_with_view("again", source("input:2")).await,
        Err(AgentError::State(_))
    ));
    assert_eq!(recorded(&agent), messages);
    assert_eq!(model.seen.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_missing_view_after_a_real_reply_keeps_the_reply_and_its_usage() {
    let model = ViewModel::new(vec![Msg::assistant("the real reply")]);
    let boundary = Arc::new(Boundary::default());
    boundary.fail_model.store(true, Ordering::SeqCst);
    let saves = Arc::new(MemCheckpointer::default());
    let agent = selected(&model, &boundary, &saves);

    assert!(matches!(
        agent
            .run_turn_with_view("raw user text", source("input:1"))
            .await,
        Err(AgentError::State(_))
    ));
    let messages = recorded(&agent);
    assert_eq!(
        messages,
        vec![Msg::user("raw user text"), Msg::assistant("the real reply")]
    );
    assert_eq!(
        saves.saves.lock().unwrap().last().unwrap().2.messages,
        messages
    );
    assert_eq!(
        agent.state.lock().unwrap().last_usage,
        Some(Usage {
            input_tokens: 7,
            output_tokens: 3
        })
    );
    assert_eq!(
        *boundary.events.lock().unwrap(),
        ["model receipt:0 calls=0"]
    );
    // The reply has no source, so no later request can be composed.
    boundary.fail_model.store(false, Ordering::SeqCst);
    assert!(matches!(
        agent.run_turn_with_view("again", source("input:2")).await,
        Err(AgentError::State(_))
    ));
    assert_eq!(model.seen.lock().unwrap().len(), 1);
}

#[test]
fn a_source_is_opaque_and_compared_by_identity() {
    let first = source("same");
    let second = source("same");
    assert!(first.is(&first.clone()) && !first.is(&second));
    assert_eq!(tag(&first), tag(&second));
    assert!(first.downcast::<String>().is_none());
    assert_eq!(format!("{first:?}"), "SourceRef([opaque])");
}

// ---- frontiers beyond the first slice ----------------------------------------

use crate::interrupt::{HitlAction, HitlRequest, Suspension};
use crate::middleware::{ToolNext, ViewNext};
use crate::provenance::{FrameworkCommit, ReachedTool, ViewSupport};
use crate::{AgentMiddleware, ToolCtx, ToolOutcome};

/// A boundary that knows the agent's own messages, checks calls and
/// decisions, and records what it was shown.
#[derive(Default)]
struct Frontiers {
    events: Mutex<Vec<String>>,
    refuse_dispatch: AtomicBool,
}

impl Frontiers {
    fn events(&self) -> Vec<String> {
        self.events.lock().unwrap().clone()
    }

    fn note(&self, event: String) {
        self.events.lock().unwrap().push(event);
    }
}

fn reached(records: &[ReachedTool]) -> String {
    records
        .iter()
        .map(|record| match &record.result {
            Ok(outcome) => format!(
                "{}{}=>{:?}",
                record.call.name, record.call.args, outcome.content
            ),
            Err(error) => format!("{}{}=>error {error}", record.call.name, record.call.args),
        })
        .collect::<Vec<_>>()
        .join(";")
}

#[async_trait::async_trait]
impl ModelViewBoundary for Frontiers {
    async fn system_sources(&self) -> Result<Vec<SourceRef>, AgentError> {
        Ok(vec![])
    }

    async fn tool_source(&self, definition: &ToolDef) -> Result<SourceRef, AgentError> {
        Ok(source(&format!("definition:{}", definition.name)))
    }

    async fn commit_model(&self, receipt: SourceRef, _: &Msg) -> Result<SourceRef, AgentError> {
        Ok(source(&format!("model:{}", tag(&receipt))))
    }

    async fn commit_reviewed_model(
        &self,
        receipt: SourceRef,
        committed: &Msg,
        edits: &[crate::provenance::ReplyEdit],
    ) -> Result<SourceRef, AgentError> {
        for edit in edits {
            self.note(format!(
                "edit by {} call {} {} -> {}",
                edit.by, edit.call, edit.received, edit.replaced
            ));
        }
        self.commit_model(receipt, committed).await
    }

    async fn observe_tool_result(
        &self,
        result: ObservedToolResult<'_>,
    ) -> Result<SourceRef, AgentError> {
        self.note(format!(
            "result {} {:?} reached=[{}] returned={:?}",
            result.call_id,
            result.content,
            reached(result.reached),
            result.returned.map(|outcome| outcome.content.as_str()),
        ));
        Ok(source(&format!("result:{}", result.call_id)))
    }

    async fn observe_framework_commit(
        &self,
        event: FrameworkCommit,
        committed: &Msg,
    ) -> Result<SourceRef, AgentError> {
        let text = match committed {
            Msg::Tool { content, .. } | Msg::Assistant { content, .. } | Msg::User { content } => {
                content.clone()
            }
            _ => String::new(),
        };
        self.note(format!("own {event:?} {text:?}"));
        if event == FrameworkCommit::Notification {
            return Err(AgentError::State(
                "no source for another agent's text".into(),
            ));
        }
        Ok(source(&format!("own:{event:?}")))
    }

    async fn before_dispatch(&self, call: &ToolCall, approved: bool) -> Result<(), AgentError> {
        self.note(format!(
            "dispatch {}{} approved={approved}",
            call.name, call.args
        ));
        if self.refuse_dispatch.load(Ordering::SeqCst) {
            return Err(AgentError::State("this call may not run".into()));
        }
        Ok(())
    }

    async fn accept_decision(
        &self,
        call: &ToolCall,
        action: &HitlAction,
        decision: &SourceRef,
    ) -> Result<(), AgentError> {
        self.note(format!("decision {} {action:?} {}", call.id, tag(decision)));
        match tag(decision).as_str() {
            "decision:shown" => Ok(()),
            _ => Err(AgentError::State("not a decision for this call".into())),
        }
    }
}

/// Returns its `text` argument, and counts its runs.
struct Echo(AtomicUsize);

#[async_trait::async_trait]
impl Tool for Echo {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "echo".into(),
            description: "echo".into(),
            parameters: serde_json::json!({"type": "object"}),
        }
    }

    async fn call(
        &self,
        args: serde_json::Value,
        _: &mut ToolCtx<'_>,
    ) -> Result<ToolOutcome, AgentError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(ToolOutcome::ok(args["text"].as_str().unwrap_or("none")))
    }
}

fn echo_call(id: &str, text: &str) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: "echo".into(),
        args: serde_json::json!({"text": text}),
    }
}

fn calls(calls: Vec<ToolCall>) -> Msg {
    Msg::Assistant {
        content: String::new(),
        tool_calls: calls,
    }
}

/// A middleware whose author stated that it leaves the record alone. It can
/// park a call, cut an outcome, watch the model call, or break its statement.
#[derive(Default)]
struct Stated {
    park: AtomicBool,
    cut: AtomicBool,
    break_statement: AtomicBool,
    seen: Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl AgentMiddleware for Stated {
    fn name(&self) -> &str {
        "stated"
    }

    fn view_support(&self) -> ViewSupport {
        ViewSupport::Preserving
    }

    async fn prepare_model(&self, state: &mut AgentState) -> Result<(), AgentError> {
        self.seen.lock().unwrap().push("prepare".into());
        if self.break_statement.load(Ordering::SeqCst) {
            state.messages.push(Msg::user("slipped in"));
        }
        Ok(())
    }

    async fn around_model(
        &self,
        _: ModelRequest,
        _: crate::Next<'_>,
    ) -> Result<ModelResponse, AgentError> {
        panic!("the recorded-text wrap is never called on a selected agent");
    }

    async fn around_model_with_view(
        &self,
        req: TrackedModelRequest,
        next: ViewNext<'_>,
    ) -> Result<ModelDelivery, AgentError> {
        self.seen
            .lock()
            .unwrap()
            .push(format!("model sources={}", req.message_sources().len()));
        next.run(req).await
    }

    async fn around_tool(
        &self,
        call: ToolCall,
        ctx: &mut ToolCtx<'_>,
        next: ToolNext<'_>,
    ) -> Result<ToolOutcome, AgentError> {
        if self.park.load(Ordering::SeqCst) {
            return Err(AgentError::Suspended(Box::new(Suspension {
                frames: vec![],
                request: HitlRequest {
                    tool_call: call,
                    summary: "run echo?".into(),
                },
            })));
        }
        let mut outcome = next.run(call, ctx).await?;
        if self.cut.load(Ordering::SeqCst) {
            outcome.content.truncate(3);
        }
        Ok(outcome)
    }
}

struct Frontier {
    model: Arc<ViewModel>,
    boundary: Arc<Frontiers>,
    echo: Arc<Echo>,
    middleware: Arc<Stated>,
    agent: Arc<Agent>,
}

fn frontier(script: Vec<Msg>) -> Frontier {
    let model = ViewModel::new(script);
    let boundary = Arc::new(Frontiers::default());
    let echo = Arc::new(Echo(AtomicUsize::new(0)));
    let middleware = Arc::new(Stated::default());
    let agent = Arc::new(
        Agent::builder(model.clone())
            .tool(echo.clone())
            .middleware(middleware.clone())
            .model_view_boundary(Some(boundary.clone()))
            .checkpointer(Arc::new(MemCheckpointer::default()))
            .durable_turn_checkpoints(true)
            .build(),
    );
    Frontier {
        model,
        boundary,
        echo,
        middleware,
        agent,
    }
}

fn last_sources(model: &ViewModel) -> Vec<String> {
    model.seen.lock().unwrap().last().unwrap().messages.clone()
}

#[tokio::test]
async fn a_stated_middleware_runs_and_the_boundary_sees_the_tool_and_the_chain_result() {
    let f = frontier(vec![
        calls(vec![echo_call("c1", "full text")]),
        Msg::assistant("done"),
    ]);
    f.middleware.cut.store(true, Ordering::SeqCst);
    f.agent
        .run_turn_with_view("go", source("input"))
        .await
        .unwrap();
    // The hooks ran, and the model call went through the view wrap.
    assert_eq!(
        *f.middleware.seen.lock().unwrap(),
        ["prepare", "model sources=1", "prepare", "model sources=3"]
    );
    // The boundary was asked before the call ran, and was shown the tool's
    // own outcome, the chain's outcome and the content that was recorded.
    assert_eq!(
        f.boundary.events(),
        [
            r#"dispatch echo{"text":"full text"} approved=false"#,
            r#"result c1 "ful" reached=[echo{"text":"full text"}=>"full text"] returned=Some("ful")"#,
        ]
    );
    assert_eq!(
        recorded(&f.agent)[2],
        Msg::Tool {
            tool_call_id: "c1".into(),
            content: "ful".into(),
            is_error: false,
        }
    );
    assert_eq!(
        last_sources(&f.model),
        ["input", "model:receipt:0", "result:c1"]
    );
}

#[tokio::test]
async fn a_middleware_that_breaks_its_statement_stops_the_turn_before_any_request() {
    let f = frontier(vec![Msg::assistant("never sent")]);
    f.middleware.break_statement.store(true, Ordering::SeqCst);
    let error = f
        .agent
        .run_turn_with_view("go", source("input"))
        .await
        .unwrap_err();
    assert!(matches!(error, AgentError::State(_)));
    assert!(f.model.seen.lock().unwrap().is_empty());
    // The accepted input stays recorded; the slipped-in message does not.
    assert_eq!(recorded(&f.agent), vec![Msg::user("go")]);
}

#[tokio::test]
async fn a_refused_call_does_not_run_and_keeps_the_reply_that_asked_for_it() {
    let f = frontier(vec![calls(vec![echo_call("c1", "text")])]);
    f.boundary.refuse_dispatch.store(true, Ordering::SeqCst);
    let error = f
        .agent
        .run_turn_with_view("go", source("input"))
        .await
        .unwrap_err();
    assert!(matches!(error, AgentError::State(_)));
    assert_eq!(f.echo.0.load(Ordering::SeqCst), 0);
    // No result was made up for the call.
    assert_eq!(
        recorded(&f.agent),
        vec![Msg::user("go"), calls(vec![echo_call("c1", "text")])]
    );
}

#[tokio::test]
async fn typed_steering_is_recorded_in_order_with_its_own_source() {
    /// Steers the running turn while its first tool call is in flight.
    struct SteerDuringTool(Mutex<Option<Arc<Agent>>>, Mutex<Vec<bool>>);
    #[async_trait::async_trait]
    impl AgentMiddleware for SteerDuringTool {
        fn name(&self) -> &str {
            "steer"
        }
        fn view_support(&self) -> ViewSupport {
            ViewSupport::Preserving
        }
        async fn around_tool(
            &self,
            call: ToolCall,
            ctx: &mut ToolCtx<'_>,
            next: ToolNext<'_>,
        ) -> Result<ToolOutcome, AgentError> {
            if let Some(agent) = self.0.lock().unwrap().take() {
                let mut accepted = self.1.lock().unwrap();
                // The untyped entry stays refused during a selected turn.
                accepted.push(agent.steer("untyped"));
                accepted.push(agent.steer_with_view("also this", source("steer")));
            }
            next.run(call, ctx).await
        }
    }
    let model = ViewModel::new(vec![
        calls(vec![echo_call("c1", "one")]),
        Msg::assistant("done"),
    ]);
    let steer = Arc::new(SteerDuringTool(Mutex::new(None), Mutex::new(Vec::new())));
    let agent = Arc::new(
        Agent::builder(model.clone())
            .tool(Arc::new(Echo(AtomicUsize::new(0))))
            .middleware(steer.clone())
            .model_view_boundary(Some(Arc::new(Frontiers::default())))
            .build(),
    );
    // No turn is active yet.
    assert!(!agent.steer_with_view("early", source("early")));
    *steer.0.lock().unwrap() = Some(agent.clone());
    agent
        .run_turn_with_view("go", source("input"))
        .await
        .unwrap();
    assert_eq!(*steer.1.lock().unwrap(), [false, true]);
    // Recorded exactly at the next round boundary, and sent with the source
    // it arrived with.
    assert_eq!(recorded(&agent)[3], Msg::user("also this"));
    assert_eq!(
        last_sources(&model),
        ["input", "model:receipt:0", "result:c1", "steer"]
    );
    assert_eq!(recorded(&agent).len(), 5);
}

#[tokio::test]
async fn a_cancelled_turn_gets_sources_for_what_the_agent_appended_itself() {
    let f = frontier(vec![Msg::assistant("after the stop")]);
    f.middleware.park.store(false, Ordering::SeqCst);
    // A reply with two calls is committed, then the turn is cancelled before
    // the first call runs.
    let model = ViewModel::new(vec![calls(vec![
        echo_call("c1", "a"),
        echo_call("c2", "b"),
    ])]);
    let boundary = Arc::new(Frontiers::default());
    let echo = Arc::new(Echo(AtomicUsize::new(0)));
    struct CancelAfterReply(Mutex<Option<Arc<Agent>>>);
    #[async_trait::async_trait]
    impl AgentMiddleware for CancelAfterReply {
        fn name(&self) -> &str {
            "cancel"
        }
        fn view_support(&self) -> ViewSupport {
            ViewSupport::Preserving
        }
        async fn review_model(&self, _: &mut AgentState) -> Result<crate::Flow, AgentError> {
            if let Some(agent) = self.0.lock().unwrap().as_ref() {
                agent.request_cancel();
            }
            Ok(crate::Flow::Proceed)
        }
    }
    let cancel = Arc::new(CancelAfterReply(Mutex::new(None)));
    let agent = Arc::new(
        Agent::builder(model.clone())
            .tool(echo.clone())
            .middleware(cancel.clone())
            .model_view_boundary(Some(boundary.clone()))
            .build(),
    );
    *cancel.0.lock().unwrap() = Some(agent.clone());
    let TurnResult::Done(output) = agent
        .run_turn_with_view("go", source("input"))
        .await
        .unwrap()
    else {
        panic!("a cancelled turn ends");
    };
    *cancel.0.lock().unwrap() = None;
    assert_eq!(output.final_text(), "Stopped by the user.");
    assert_eq!(echo.0.load(Ordering::SeqCst), 0);
    // The record is what an unselected agent records.
    let messages = recorded(&agent);
    assert_eq!(messages.len(), 5);
    assert!(
        matches!(&messages[2], Msg::Tool { content, is_error: true, .. }
        if content == "Tool call cancelled by the user.")
    );
    // Each message the agent appended itself was reported with its event.
    assert_eq!(
        boundary.events(),
        [
            r#"own CancelledCall "Tool call cancelled by the user.""#,
            r#"own CancelledCall "Tool call cancelled by the user.""#,
            r#"own CancelNote "Stopped by the user.""#,
        ]
    );
    // The conversation continues: the next request carries every source.
    *model.script.lock().unwrap() = vec![Msg::assistant("continued")].into_iter();
    agent
        .run_turn_with_view("again", source("second"))
        .await
        .unwrap();
    assert_eq!(
        last_sources(&model),
        [
            "input",
            "model:receipt:0",
            "own:CancelledCall",
            "own:CancelledCall",
            "own:CancelNote",
            "second"
        ]
    );
    drop(f);
}

#[tokio::test]
async fn a_parked_call_resumes_only_through_a_checked_decision() {
    let script = || {
        vec![
            calls(vec![echo_call("c1", "model text")]),
            Msg::assistant("done"),
        ]
    };
    // Approved as shown.
    let f = frontier(script());
    f.middleware.park.store(true, Ordering::SeqCst);
    let parked = f.agent.run_turn_with_view("go", source("input")).await;
    assert!(matches!(parked, Ok(TurnResult::Suspended(_))));
    assert_eq!(f.echo.0.load(Ordering::SeqCst), 0);
    // The untyped entry is refused, and so is a decision the boundary does
    // not accept. Both leave the call parked and unrun.
    assert!(f.agent.resume(HitlAction::Approve, "denied").await.is_err());
    assert!(
        f.agent
            .resume_with_view(HitlAction::Approve, "denied", source("decision:other"))
            .await
            .is_err()
    );
    assert_eq!(f.echo.0.load(Ordering::SeqCst), 0);
    assert!(f.agent.pending_suspension().is_some());
    f.middleware.park.store(false, Ordering::SeqCst);
    f.agent
        .resume_with_view(HitlAction::Approve, "denied", source("decision:shown"))
        .await
        .unwrap();
    assert_eq!(f.echo.0.load(Ordering::SeqCst), 1);
    let events = f.boundary.events();
    assert_eq!(
        events[events.len() - 3..],
        [
            "decision c1 Approve decision:shown".to_owned(),
            r#"dispatch echo{"text":"model text"} approved=true"#.to_owned(),
            r#"result c1 "model text" reached=[echo{"text":"model text"}=>"model text"] returned=Some("model text")"#.to_owned(),
        ]
    );
    assert_eq!(
        last_sources(&f.model),
        ["input", "model:receipt:0", "result:c1"]
    );

    // Approved with edited arguments: the tool gets the edited call, and the
    // reply that asked for the original stays recorded as received.
    let f = frontier(script());
    f.middleware.park.store(true, Ordering::SeqCst);
    f.agent
        .run_turn_with_view("go", source("input"))
        .await
        .unwrap();
    f.middleware.park.store(false, Ordering::SeqCst);
    let edited = HitlAction::ApproveEdited {
        args: serde_json::json!({"text": "human text"}),
    };
    f.agent
        .resume_with_view(edited, "denied", source("decision:shown"))
        .await
        .unwrap();
    assert!(
        f.boundary
            .events()
            .contains(&r#"dispatch echo{"text":"human text"} approved=true"#.to_owned())
    );
    assert_eq!(
        recorded(&f.agent)[1],
        calls(vec![echo_call("c1", "model text")])
    );
    assert!(matches!(&recorded(&f.agent)[2], Msg::Tool { content, .. } if content == "human text"));

    // Denied: nothing runs, the denial is recorded and reported as the
    // agent's own message with the human's words.
    let f = frontier(script());
    f.middleware.park.store(true, Ordering::SeqCst);
    f.agent
        .run_turn_with_view("go", source("input"))
        .await
        .unwrap();
    let denied = HitlAction::Deny {
        feedback: "not that one".into(),
    };
    f.agent
        .resume_with_view(denied, "The call was denied.", source("decision:shown"))
        .await
        .unwrap();
    assert_eq!(f.echo.0.load(Ordering::SeqCst), 0);
    assert!(
        f.boundary
            .events()
            .contains(&r#"own Denial "The call was denied. Feedback: not that one""#.to_owned())
    );
    assert_eq!(
        last_sources(&f.model),
        ["input", "model:receipt:0", "own:Denial"]
    );
}

#[tokio::test]
async fn text_from_another_agent_has_no_source_and_stops_the_next_request() {
    let f = frontier(vec![Msg::assistant("never sent")]);
    f.agent.model_view.as_ref().unwrap().note_inputs(vec![None]);
    f.agent
        .state
        .lock()
        .unwrap()
        .messages
        .push(Msg::user("[message from agent 'child']\nhello"));
    let error = f
        .agent
        .run_turn_with_view("go", source("input"))
        .await
        .unwrap_err();
    assert!(matches!(error, AgentError::State(_)));
    assert!(f.model.seen.lock().unwrap().is_empty());
    assert_eq!(recorded(&f.agent).len(), 2);
}

#[tokio::test]
async fn durable_reconciliation_keeps_every_slot_beside_its_message() {
    // A reply asked for two calls and one result was recorded before an
    // interruption; a later input follows.
    let messages = vec![
        Msg::user("go"),
        calls(vec![echo_call("c1", "a"), echo_call("c2", "b")]),
        Msg::Tool {
            tool_call_id: "c1".into(),
            content: "a".into(),
            is_error: false,
        },
        Msg::user("later"),
        calls(vec![echo_call("c3", "c")]),
    ];
    let mut plain = AgentState {
        messages: messages.clone(),
        ..Default::default()
    };
    reconcile_recovered_calls(&mut plain, None);

    let view = ModelView::new(Arc::new(Frontiers::default()));
    view.note_inputs(
        ["input", "model:0", "result:c1", "later", "model:1"]
            .iter()
            .map(|name| Some(source(name)))
            .collect(),
    );
    let mut tracked = AgentState {
        messages,
        ..Default::default()
    };
    view.reconcile(&mut tracked);
    // The record is reconciled exactly as without a boundary.
    assert_eq!(tracked.messages, plain.messages);
    let slots = view.sources.lock().unwrap();
    let shape: Vec<String> = slots
        .iter()
        .map(|slot| match slot {
            Slot::Bound(kept) => tag(kept),
            Slot::Pending(event) => format!("{event:?}"),
        })
        .collect();
    assert_eq!(
        shape,
        [
            "input",
            "model:0",
            "result:c1",
            "UnrecordedCall",
            "later",
            "model:1",
            "UnrecordedCall"
        ]
    );
    assert!(matches!(&tracked.messages[3], Msg::Tool { tool_call_id, .. } if tool_call_id == "c2"));
}

// ---- tracked review of a reply, and state a middleware must leave alone ------

fn string_call(id: &str, raw: &str) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: "echo".into(),
        args: serde_json::Value::String(raw.into()),
    }
}

#[tokio::test]
async fn the_protected_call_repair_runs_as_a_recorded_change_of_the_received_reply() {
    use crate::builtin::patch_tool_calls::PatchToolCallsMiddleware;
    let script = || {
        vec![
            calls(vec![
                string_call("c1", r#"{"text":"encoded"}"#),
                echo_call("c2", "plain"),
                string_call("c3", "not json{"),
            ]),
            Msg::assistant("done"),
        ]
    };
    let model = ViewModel::new(script());
    let boundary = Arc::new(Frontiers::default());
    let echo = Arc::new(Echo(AtomicUsize::new(0)));
    let agent = Agent::builder(model.clone())
        .tool(echo.clone())
        .middleware(Arc::new(PatchToolCallsMiddleware))
        .model_view_boundary(Some(boundary.clone()))
        .build();
    agent
        .run_turn_with_view("go", source("input"))
        .await
        .unwrap();
    // The record holds the repaired calls, exactly as an unselected agent
    // records them.
    let plain = Agent::builder(Arc::new(ScriptedModel::new(script())))
        .tool(Arc::new(Echo(AtomicUsize::new(0))))
        .middleware(Arc::new(PatchToolCallsMiddleware))
        .build();
    plain.run_turn("go").await.unwrap();
    assert_eq!(recorded(&agent), recorded(&plain));
    assert_eq!(
        recorded(&agent)[1],
        calls(vec![
            echo_call("c1", "encoded"),
            echo_call("c2", "plain"),
            ToolCall {
                id: "c3".into(),
                name: "echo".into(),
                args: serde_json::json!({"_raw": "not json{"}),
            },
        ])
    );
    // Each change was recorded when it was made and reported with the commit:
    // which middleware, which call, what was received and what replaced it.
    let events = boundary.events();
    assert_eq!(
        events[..2],
        [
            r#"edit by patch_tool_calls call 0 "{\"text\":\"encoded\"}" -> {"text":"encoded"}"#
                .to_owned(),
            r#"edit by patch_tool_calls call 2 "not json{" -> {"_raw":"not json{"}"#.to_owned(),
        ]
    );
    // The repaired calls are what was checked and run.
    assert!(events.contains(&r#"dispatch echo{"text":"encoded"} approved=false"#.to_owned()));
    assert!(events.contains(&r#"dispatch echo{"_raw":"not json{"} approved=false"#.to_owned()));
    assert_eq!(echo.0.load(Ordering::SeqCst), 3);

    // A boundary that does not know such changes refuses the changed reply.
    // The reply stays recorded as repaired and nothing is run.
    let model = ViewModel::new(script());
    let unaware = Arc::new(Boundary::default());
    let echo = Arc::new(Echo(AtomicUsize::new(0)));
    let agent = Agent::builder(model.clone())
        .tool(echo.clone())
        .middleware(Arc::new(PatchToolCallsMiddleware))
        .model_view_boundary(Some(unaware.clone()))
        .build();
    assert!(
        agent
            .run_turn_with_view("go", source("input"))
            .await
            .is_err()
    );
    assert_eq!(recorded(&agent).len(), 2);
    assert_eq!(echo.0.load(Ordering::SeqCst), 0);
    assert!(unaware.events.lock().unwrap().is_empty());
}

/// A middleware that states it leaves the record alone and then does not.
struct Meddler(&'static str);

#[async_trait::async_trait]
impl AgentMiddleware for Meddler {
    fn name(&self) -> &str {
        "meddler"
    }

    fn view_support(&self) -> ViewSupport {
        ViewSupport::Preserving
    }

    async fn turn_start(&self, state: &mut AgentState) -> Result<(), AgentError> {
        match self.0 {
            "custom" => {
                state.custom.insert("note".into(), serde_json::json!(1));
            }
            "todos" => state.todos.push(crate::Todo {
                content: "slipped in".into(),
                status: crate::TodoStatus::Pending,
            }),
            "files" => {
                state.files.insert(
                    "a.txt".into(),
                    crate::FileData {
                        content: String::new(),
                        modified: false,
                    },
                );
            }
            _ => {}
        }
        Ok(())
    }

    async fn review_model(&self, state: &mut AgentState) -> Result<crate::Flow, AgentError> {
        if self.0 == "reply" {
            if let Some(Msg::Assistant { content, .. }) = state.messages.last_mut() {
                content.push_str(" (edited)");
            }
        }
        Ok(crate::Flow::Proceed)
    }

    async fn around_tool(
        &self,
        call: ToolCall,
        ctx: &mut ToolCtx<'_>,
        next: ToolNext<'_>,
    ) -> Result<ToolOutcome, AgentError> {
        if self.0 == "before tool" {
            ctx.state
                .custom
                .insert("before".into(), serde_json::json!(1));
        }
        let outcome = next.run(call, ctx).await;
        if self.0 == "after tool" {
            ctx.state
                .custom
                .insert("after".into(), serde_json::json!(1));
        }
        outcome
    }
}

/// A tool that changes recorded state itself, which a tool may do.
struct Marker;

#[async_trait::async_trait]
impl Tool for Marker {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "echo".into(),
            description: "marks".into(),
            parameters: serde_json::json!({"type": "object"}),
        }
    }

    async fn call(
        &self,
        _: serde_json::Value,
        ctx: &mut ToolCtx<'_>,
    ) -> Result<ToolOutcome, AgentError> {
        ctx.state.custom.insert("tool".into(), serde_json::json!(1));
        Ok(ToolOutcome::ok("marked"))
    }
}

#[tokio::test]
async fn a_statement_does_not_cover_changes_to_todos_files_custom_state_or_a_reply() {
    let script = || vec![calls(vec![echo_call("c1", "text")]), Msg::assistant("done")];
    let run = |what: &'static str| async move {
        let model = ViewModel::new(script());
        let agent = Agent::builder(model.clone())
            .tool(Arc::new(Marker))
            .middleware(Arc::new(Meddler(what)))
            .model_view_boundary(Some(Arc::new(Frontiers::default())))
            .build();
        let result = agent.run_turn_with_view("go", source("input")).await;
        (result, agent, model)
    };
    // A hook that changes anything the agent carries stops the turn before a
    // request exists, and its change is not kept.
    for what in ["custom", "todos", "files"] {
        let (result, agent, model) = run(what).await;
        assert!(matches!(result, Err(AgentError::State(_))), "{what}");
        assert!(model.seen.lock().unwrap().is_empty(), "{what}");
        let state = agent.state.lock().unwrap();
        assert_eq!(state.messages, vec![Msg::user("go")], "{what}");
        assert!(state.custom.is_empty() && state.todos.is_empty() && state.files.is_empty());
    }
    // An untracked change of the received reply stops the turn before the
    // reply is committed or any call runs.
    let (result, agent, _) = run("reply").await;
    assert!(matches!(result, Err(AgentError::State(_))));
    assert_eq!(recorded(&agent), vec![Msg::user("go")]);
    // A change around the tool: the tool ran, so its result and its own
    // effect are kept, and the turn stops there.
    for what in ["before tool", "after tool"] {
        let (result, agent, model) = run(what).await;
        assert!(matches!(result, Err(AgentError::State(_))), "{what}");
        assert_eq!(model.seen.lock().unwrap().len(), 1, "{what}");
        let state = agent.state.lock().unwrap();
        assert_eq!(state.messages.len(), 3, "{what}");
        assert_eq!(state.custom["tool"], 1, "{what}");
    }
    // The tool's own change is what a tool may do.
    let (result, agent, model) = run("nothing").await;
    assert!(matches!(result, Ok(TurnResult::Done(_))));
    assert_eq!(model.seen.lock().unwrap().len(), 2);
    assert_eq!(agent.state.lock().unwrap().custom["tool"], 1);
}
