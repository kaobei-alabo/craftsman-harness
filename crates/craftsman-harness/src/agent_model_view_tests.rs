//! Ordinary turns through the optional source view boundary, with a scripted
//! provider and a recording boundary. Nothing here contacts a model service.

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
