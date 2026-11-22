//! One conversation from a raw capture to the Chat wire and back, through
//! the framework's agent, this session's boundary, the private Chat provider
//! and the existing client and guard, against a loopback server. The vault
//! and the guard are real; the detector is fixed host reply rules.

use super::tests::{draft, vault};
use super::view::received_chat_reply;
use super::*;
use crate::llm::LlmClient;
use crate::privacy_hook::tests::{EMAIL, LiteralSource, PHONE};
use crate::privacy_send::tests::answer;
use craftsman_harness::provenance::{ModelViewBoundary, SourceRef};
use craftsman_harness::{
    Agent, AgentError, ModelProvider, ModelRequest, Msg, Tool, ToolCall, ToolCtx, ToolDef,
    ToolOutcome, TurnResult,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Mutex;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

const OPERATOR: &str = "ops@example.invalid";

/// A tool with fixed host text in its description and a fixed raw result.
struct Lookup(String);

#[async_trait::async_trait]
impl Tool for Lookup {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "lookup".into(),
            description: format!("Look up a contact. Problems go to {OPERATOR}."),
            parameters: json!({"type": "object", "properties": {"who": {"type": "string"}}}),
        }
    }

    async fn call(&self, _: Value, _: &mut ToolCtx<'_>) -> Result<ToolOutcome, AgentError> {
        Ok(ToolOutcome::ok(self.0.clone()))
    }
}

fn tool_call() -> ResponseTemplate {
    let body = json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": "c1", "type": "function",
        "function": {"name": "lookup", "arguments": "{\"who\":\"them\"}"}}]}, "finish_reason": "tool_calls"}],
        "usage": {"prompt_tokens": 5, "completion_tokens": 1}});
    ResponseTemplate::new(200).set_body_raw(
        format!("data: {body}\n\ndata: [DONE]\n\n"),
        "text/event-stream",
    )
}

/// First a tool call, then a final answer.
async fn server() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(tool_call())
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(answer("chat", &["fixture answer"]))
        .mount(&server)
        .await;
    server
}

fn client(server: &MockServer) -> LlmClient {
    LlmClient::new_with_options(
        &server.uri(),
        "fixture-key",
        "fixture-model",
        &BTreeMap::new(),
    )
}

struct Conversation {
    session: Arc<PrivacySession>,
    boundary: Arc<ConversationBoundary>,
    agent: Agent,
    usage: Arc<Mutex<(u64, u64)>>,
    shown: Arc<Mutex<String>>,
}

async fn conversation(vault: &Vault, server: &MockServer, tool_result: &str) -> Conversation {
    let source = LiteralSource::new(&[(EMAIL, "email"), (PHONE, "phone"), (OPERATOR, "email")]);
    let session = Arc::new(
        PrivacySession::open(
            vault,
            FreshConversationBinding::reserve(&std::env::temp_dir()).unwrap(),
            Detector::from_test_source(source),
        )
        .await
        .unwrap(),
    );
    // The system text is host text, captured and admitted before any request.
    let system = session
        .admit_raw(&CapturedRaw::from_producer(
            RawOrigin::HostStatic,
            &format!("You are a careful assistant. The operator is {OPERATOR}."),
        ))
        .await
        .unwrap();
    let boundary = session.boundary(&[system]).unwrap();
    let usage = Arc::new(Mutex::new((0, 0)));
    let shown = Arc::new(Mutex::new(String::new()));
    let provider = ViewChatProvider::new(client(server), session.clone(), usage.clone(), {
        let shown = shown.clone();
        Arc::new(move |text: &str| shown.lock().unwrap().push_str(text))
    })
    .await
    .unwrap();
    let agent = Agent::builder(Arc::new(provider))
        .tool(Arc::new(Lookup(tool_result.to_owned())))
        .model_view_boundary(Some(boundary.clone()))
        .build();
    Conversation {
        session,
        boundary,
        agent,
        usage,
        shown,
    }
}

fn recorded(agent: &Agent) -> Vec<Msg> {
    agent.state.lock().unwrap().messages.clone()
}

async fn bodies(server: &MockServer) -> Vec<Value> {
    server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|request| serde_json::from_slice(&request.body).unwrap())
        .collect()
}

#[tokio::test]
async fn a_raw_capture_reaches_the_chat_wire_only_as_views_and_the_record_stays_real() {
    let root = tempfile::tempdir().unwrap();
    let vault = vault(&root).await;
    let server = server().await;
    let result = format!("their number is {PHONE}");
    let c = conversation(&vault, &server, &result).await;

    let text = format!("who handles {EMAIL}?");
    let raw = draft(&text);
    let node = c.session.admit_raw(&raw).await.unwrap();
    let TurnResult::Done(output) = c
        .agent
        .run_turn_with_view(text.clone(), c.session.source(&node).unwrap())
        .await
        .unwrap()
    else {
        panic!("turn did not finish");
    };
    assert_eq!(output.final_text(), "fixture answer");
    assert_eq!(*c.shown.lock().unwrap(), "fixture answer");

    // The recorded conversation is the real one: what was typed, what the
    // model replied and what the tool returned.
    assert_eq!(
        recorded(&c.agent),
        vec![
            Msg::user(text.clone()),
            Msg::Assistant {
                content: String::new(),
                tool_calls: vec![ToolCall {
                    id: "c1".into(),
                    name: "lookup".into(),
                    args: json!({"who": "them"}),
                }],
            },
            Msg::Tool {
                tool_call_id: "c1".into(),
                content: result.clone(),
                is_error: false,
            },
            Msg::assistant("fixture answer"),
        ]
    );
    // The real usage of both replies is kept.
    assert_eq!(*c.usage.lock().unwrap(), (8, 3));

    // No original left the process in either request.
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    for request in &requests {
        let sent = String::from_utf8(request.body.clone()).unwrap();
        for original in [EMAIL, PHONE, OPERATOR] {
            assert!(!sent.contains(original), "{original}");
        }
    }
    // Each text went out as the view of its own source; structure, names,
    // ids and the model's own call went out as recorded.
    let sent = bodies(&server).await;
    let view = node.model_view().unwrap();
    for body in &sent {
        let system = body["messages"][0]["content"].as_str().unwrap();
        assert!(system.starts_with("You are a careful assistant. The operator is pv1-"));
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(
            body["messages"][1],
            json!({"role": "user", "content": view})
        );
        let function = &body["tools"][0]["function"];
        assert_eq!(function["name"], "lookup");
        assert!(
            function["description"]
                .as_str()
                .unwrap()
                .starts_with("Look up a contact. Problems go to pv1-")
        );
        assert_eq!(
            function["parameters"],
            json!({"type": "object", "properties": {"who": {"type": "string"}}})
        );
    }
    assert_eq!(sent[0]["messages"].as_array().unwrap().len(), 2);
    let later = sent[1]["messages"].as_array().unwrap();
    assert_eq!(later.len(), 4);
    assert_eq!(later[2]["role"], "assistant");
    assert_eq!(later[2]["tool_calls"][0]["id"], "c1");
    assert_eq!(
        later[2]["tool_calls"][0]["function"]["arguments"],
        "{\"who\":\"them\"}"
    );
    assert_eq!(later[3]["role"], "tool");
    assert_eq!(later[3]["tool_call_id"], "c1");
    let tool_view = later[3]["content"].as_str().unwrap();
    assert!(tool_view.starts_with("their number is ") && tool_view != result);
    // Three values are known: the operator, the address and the number.
    assert_eq!(c.session.known_values().await, 3);
    assert!(c.session.check().is_ok());
}

#[tokio::test]
async fn a_returned_result_without_a_view_stays_recorded_and_ends_the_conversation() {
    let root = tempfile::tempdir().unwrap();
    let vault = vault(&root).await;
    let server = server().await;
    // The tool will return an issued placeholder, which cannot be admitted.
    let probe = conversation(&vault, &server, "unused").await;
    drop(probe);
    let c = conversation(&vault, &server, "placeholder").await;
    let text = format!("who handles {EMAIL}?");
    let node = c.session.admit_raw(&draft(&text)).await.unwrap();
    let issued = node
        .model_view()
        .unwrap()
        .strip_prefix("who handles ")
        .unwrap()
        .trim_end_matches('?')
        .to_owned();
    let result = format!("the tool echoed {issued}");
    let agent = Agent::builder(Arc::new(
        ViewChatProvider::new(
            client(&server),
            c.session.clone(),
            c.usage.clone(),
            Arc::new(|_: &str| {}),
        )
        .await
        .unwrap(),
    ))
    .tool(Arc::new(Lookup(result.clone())))
    .model_view_boundary(Some(c.boundary.clone()))
    .build();

    let error = agent
        .run_turn_with_view(text.clone(), c.session.source(&node).unwrap())
        .await
        .err()
        .expect("the turn stops");
    assert!(matches!(error, AgentError::State(_)));
    // The result is recorded exactly as the tool returned it. It was not
    // replaced, and nothing claims the tool did not run.
    let messages = recorded(&agent);
    assert_eq!(messages.len(), 3);
    assert_eq!(
        messages[2],
        Msg::Tool {
            tool_call_id: "c1".into(),
            content: result,
            is_error: false,
        }
    );
    // One request was made. The session is closed, so nothing further is
    // admitted, composed or sent.
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
    assert_eq!(c.session.check(), Err(SessionStop::Closed));
    assert!(matches!(
        agent
            .run_turn_with_view("again", SourceRef::new(Arc::new(())))
            .await,
        Err(AgentError::State(_))
    ));
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_source_that_is_not_this_conversations_or_does_not_fit_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let vault = vault(&root).await;
    let server = server().await;
    let c = conversation(&vault, &server, "unused").await;
    let other = conversation(&vault, &server, "unused").await;
    let text = format!("who handles {EMAIL}?");
    let node = c.session.admit_raw(&draft(&text)).await.unwrap();
    let foreign = other.session.admit_raw(&draft(&text)).await.unwrap();

    // A node of another conversation gives no source here.
    assert_eq!(c.session.source(&foreign).err(), Some(SessionStop::Unbound));
    assert!(
        c.session
            .admitted(&other.session.source(&foreign).unwrap())
            .is_none()
    );
    assert!(
        c.session
            .admitted(&c.session.source(&node).unwrap())
            .is_some()
    );
    assert!(
        c.session
            .committed_model(&c.session.source(&node).unwrap())
            .is_none()
    );
    // A draft cannot stand in the system channel.
    assert_eq!(
        c.session.boundary(&[node.clone()]).err(),
        Some(SessionStop::Unbound)
    );

    // The recorded text is never sent through this provider, and a client
    // that is already bound or speaks a native wire is not accepted.
    let provider = ViewChatProvider::new(
        client(&server),
        c.session.clone(),
        c.usage.clone(),
        Arc::new(|_: &str| {}),
    )
    .await
    .unwrap();
    assert!(matches!(
        provider
            .complete(ModelRequest {
                system: String::new(),
                messages: vec![Msg::user(text.clone())],
                tools: vec![],
            })
            .await,
        Err(AgentError::Model(_))
    ));
    let bound = client(&server)
        .with_send_guard(c.session.send_guard())
        .unwrap();
    assert!(matches!(
        ViewChatProvider::new(
            bound,
            c.session.clone(),
            c.usage.clone(),
            Arc::new(|_: &str| {})
        )
        .await
        .err(),
        Some(SessionStop::Guard(_))
    ));

    // A reply received by another conversation cannot be committed here.
    let reply = Msg::assistant("a reply");
    let receipt = SourceRef::new(Arc::new(
        received_chat_reply(&other.session, &reply, 0, [0; 32]).unwrap(),
    ));
    assert!(c.boundary.commit_model(receipt, &reply).await.is_err());
    assert_eq!(c.session.check(), Err(SessionStop::Closed));

    // A source that does not fit the recorded input: the input stays
    // recorded, nothing is sent and the conversation stops.
    let d = conversation(&vault, &server, "unused").await;
    let node = d.session.admit_raw(&draft(&text)).await.unwrap();
    let error = d
        .agent
        .run_turn_with_view("a different input", d.session.source(&node).unwrap())
        .await
        .err()
        .expect("the turn stops");
    assert!(matches!(error, AgentError::Model(_)));
    assert_eq!(recorded(&d.agent), vec![Msg::user("a different input")]);
    assert_eq!(d.session.check(), Err(SessionStop::Closed));
    assert!(server.received_requests().await.unwrap().is_empty());
}

// ---- cancellation, human decisions and checked operations --------------------

use craftsman_harness::provenance::ViewSupport;
use craftsman_harness::{
    AgentMiddleware, AgentState, Flow, HitlAction, HitlRequest, Suspension, ToolNext,
};

/// A middleware whose author stated that it leaves the record alone. It can
/// cancel the turn after the first reply and park every call.
#[derive(Default)]
struct Gate {
    agent: Mutex<Option<Arc<Agent>>>,
    cancel_once: std::sync::atomic::AtomicBool,
    park: std::sync::atomic::AtomicBool,
}

#[async_trait::async_trait]
impl AgentMiddleware for Gate {
    fn name(&self) -> &str {
        "gate"
    }

    fn view_support(&self) -> ViewSupport {
        ViewSupport::Preserving
    }

    async fn review_model(&self, _: &mut AgentState) -> Result<Flow, AgentError> {
        if self.cancel_once.swap(false, Ordering::SeqCst)
            && let Some(agent) = self.agent.lock().unwrap().as_ref()
        {
            agent.request_cancel();
        }
        Ok(Flow::Proceed)
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
                    summary: "run lookup?".into(),
                },
            })));
        }
        next.run(call, ctx).await
    }
}

/// What the tool saw of the operation that was checked for its own call.
#[derive(Default)]
struct Seen {
    args: Vec<Value>,
    edited: Vec<bool>,
    /// Place, range and the original behind each field.
    fields: Vec<Vec<(FieldPlace, std::ops::Range<usize>, String)>>,
}

/// A tool named `lookup` that reads the operation in flight.
struct Inspect {
    boundary: Mutex<Option<Arc<ConversationBoundary>>>,
    seen: Mutex<Seen>,
    result: String,
}

#[async_trait::async_trait]
impl Tool for Inspect {
    fn def(&self) -> ToolDef {
        Lookup(String::new()).def()
    }

    async fn call(&self, args: Value, _: &mut ToolCtx<'_>) -> Result<ToolOutcome, AgentError> {
        let boundary = self.boundary.lock().unwrap().clone().unwrap();
        let operation = boundary
            .operation_in_flight()
            .expect("an operation was checked");
        assert_eq!(operation.call().unwrap().args, args);
        let fields = operation
            .fields()
            .unwrap()
            .expect("no overlap")
            .iter()
            .map(|field| {
                let original = field
                    .with_record(|_, records, index| {
                        records
                            .with_host_field(index, |host| {
                                host.original_for_validated_host_field().to_owned()
                            })
                            .unwrap()
                    })
                    .unwrap();
                (field.place().clone(), field.range(), original)
            })
            .collect();
        let mut seen = self.seen.lock().unwrap();
        seen.args.push(args);
        seen.edited.push(operation.edited_by_human());
        seen.fields.push(fields);
        Ok(ToolOutcome::ok(self.result.clone()))
    }
}

fn sse(body: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(
        format!("data: {body}\n\ndata: [DONE]\n\n"),
        "text/event-stream",
    )
}

fn asks(arguments: Value) -> ResponseTemplate {
    sse(
        json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": "c1", "type": "function",
        "function": {"name": "lookup", "arguments": arguments.to_string()}}]}, "finish_reason": "tool_calls"}],
        "usage": {"prompt_tokens": 5, "completion_tokens": 1}}),
    )
}

/// Mounts `replies` to be given in order, one request each.
async fn scripted(server: &MockServer, replies: Vec<ResponseTemplate>) {
    for (index, reply) in replies.into_iter().enumerate() {
        Mock::given(method("POST"))
            .respond_with(reply)
            .up_to_n_times(1)
            .with_priority(index as u8 + 1)
            .mount(server)
            .await;
    }
}

struct Gated {
    c: Conversation,
    gate: Arc<Gate>,
    tool: Arc<Inspect>,
    agent: Arc<Agent>,
}

/// One conversation whose agent has the gate and the inspecting tool.
async fn gated(vault: &Vault, server: &MockServer, result: &str) -> Gated {
    let c = conversation(vault, server, "unused").await;
    let gate = Arc::new(Gate::default());
    let tool = Arc::new(Inspect {
        boundary: Mutex::new(Some(c.boundary.clone())),
        seen: Mutex::new(Seen::default()),
        result: result.to_owned(),
    });
    let provider = ViewChatProvider::new(
        client(server),
        c.session.clone(),
        c.usage.clone(),
        Arc::new(|_: &str| {}),
    )
    .await
    .unwrap();
    let agent = Arc::new(
        Agent::builder(Arc::new(provider))
            .tool(tool.clone())
            .middleware(gate.clone())
            .model_view_boundary(Some(c.boundary.clone()))
            .build(),
    );
    *gate.agent.lock().unwrap() = Some(agent.clone());
    Gated {
        c,
        gate,
        tool,
        agent,
    }
}

/// The placeholder the conversation issued for the address in `node`.
fn issued(node: &AdmittedNode) -> String {
    node.model_view()
        .unwrap()
        .strip_prefix("who handles ")
        .unwrap()
        .trim_end_matches('?')
        .to_owned()
}

#[tokio::test]
async fn a_cancelled_turn_continues_and_the_agents_own_messages_are_sent_as_views() {
    let root = tempfile::tempdir().unwrap();
    let vault = vault(&root).await;
    let server = MockServer::start().await;
    scripted(
        &server,
        vec![
            asks(json!({"who": "them"})),
            answer("chat", &["fixture answer"]),
        ],
    )
    .await;
    let g = gated(&vault, &server, "unused").await;
    g.gate.cancel_once.store(true, Ordering::SeqCst);

    let text = format!("who handles {EMAIL}?");
    let node = g.c.session.admit_raw(&draft(&text)).await.unwrap();
    let TurnResult::Done(output) = g
        .agent
        .run_turn_with_view(text.clone(), g.c.session.source(&node).unwrap())
        .await
        .unwrap()
    else {
        panic!("a cancelled turn ends");
    };
    assert_eq!(output.final_text(), "Stopped by the user.");
    assert!(g.tool.seen.lock().unwrap().args.is_empty());
    // An ordinary cancellation does not end the conversation.
    assert!(g.c.session.check().is_ok());
    assert!(
        g.agent
            .recorded_sources()
            .unwrap()
            .iter()
            .all(Option::is_some)
    );

    let next = format!("then call {PHONE}");
    let node = g.c.session.admit_raw(&draft(&next)).await.unwrap();
    g.agent
        .run_turn_with_view(next.clone(), g.c.session.source(&node).unwrap())
        .await
        .unwrap();
    // The record holds the agent's own messages as it always writes them.
    let messages = recorded(&g.agent);
    assert_eq!(messages.len(), 6);
    assert_eq!(messages[3], Msg::assistant("Stopped by the user."));
    assert_eq!(messages[4], Msg::user(next));
    let sent = bodies(&server).await;
    assert_eq!(sent.len(), 2);
    let later = sent[1]["messages"].as_array().unwrap();
    assert_eq!(later.len(), 6);
    assert_eq!(later[3]["content"], "Tool call cancelled by the user.");
    assert_eq!(later[3]["tool_call_id"], "c1");
    assert_eq!(
        later[4],
        json!({"role": "assistant", "content": "Stopped by the user."})
    );
    assert_eq!(later[5]["content"], node.model_view().unwrap());
    let raw = String::from_utf8(server.received_requests().await.unwrap()[1].body.clone()).unwrap();
    for original in [EMAIL, PHONE, OPERATOR] {
        assert!(!raw.contains(original), "{original}");
    }
}

#[tokio::test]
async fn a_parked_call_runs_only_as_decided_and_the_tool_sees_the_checked_operation() {
    let root = tempfile::tempdir().unwrap();
    let vault = vault(&root).await;
    let text = format!("who handles {EMAIL}?");

    // Approved as the model wrote it.
    let server = MockServer::start().await;
    let g = gated(&vault, &server, &format!("their number is {PHONE}")).await;
    let other = conversation(&vault, &server, "unused").await;
    let node = g.c.session.admit_raw(&draft(&text)).await.unwrap();
    let placeholder = issued(&node);
    scripted(
        &server,
        vec![
            asks(json!({"who": placeholder, "notes": ["plain", format!("cc {placeholder}")]})),
            answer("chat", &[&format!("ask {placeholder} today")]),
        ],
    )
    .await;
    g.gate.park.store(true, Ordering::SeqCst);
    let TurnResult::Suspended(parked) = g
        .agent
        .run_turn_with_view(text.clone(), g.c.session.source(&node).unwrap())
        .await
        .unwrap()
    else {
        panic!("the call is parked");
    };
    let call = parked.request.tool_call.clone();
    g.gate.park.store(false, Ordering::SeqCst);
    // Evidence from another conversation, for another call or for another
    // action is turned away. Nothing runs and this conversation continues.
    let mut altered = call.clone();
    altered.args = json!({"who": "someone"});
    for wrong in [
        other
            .session
            .human_decision(&call, &HitlAction::Approve)
            .unwrap(),
        g.c.session
            .human_decision(&altered, &HitlAction::Approve)
            .unwrap(),
        g.c.session
            .human_decision(
                &call,
                &HitlAction::Deny {
                    feedback: String::new(),
                },
            )
            .unwrap(),
    ] {
        assert!(
            g.agent
                .resume_with_view(HitlAction::Approve, "Denied.", wrong)
                .await
                .is_err()
        );
    }
    assert!(g.tool.seen.lock().unwrap().args.is_empty());
    assert!(g.c.session.check().is_ok());
    let decision =
        g.c.session
            .human_decision(&call, &HitlAction::Approve)
            .unwrap();
    g.agent
        .resume_with_view(HitlAction::Approve, "Denied.", decision)
        .await
        .unwrap();
    {
        // The tool ran once, with the model's own arguments, and saw where
        // those arguments name what this conversation issued.
        let seen = g.tool.seen.lock().unwrap();
        assert_eq!(seen.args, [call.args.clone()]);
        assert_eq!(seen.edited, [false]);
        let argument = |pointer: &str| FieldPlace::Argument {
            tool: "lookup".into(),
            pointer: pointer.into(),
        };
        assert_eq!(
            seen.fields[0],
            [
                (
                    argument("/notes/1"),
                    3..3 + placeholder.len(),
                    EMAIL.to_owned()
                ),
                (argument("/who"), 0..placeholder.len(), EMAIL.to_owned()),
            ]
        );
    }
    // The host can reach what was committed and what ran through the sources
    // the agent kept.
    let sources = g.agent.recorded_sources().unwrap();
    let sources: Vec<_> = sources.into_iter().map(Option::unwrap).collect();
    assert_eq!(sources.len(), 4);
    let result = g.c.session.actual_result(&sources[2]).unwrap();
    assert_eq!((result.call_id(), result.tool()), ("c1", "lookup"));
    assert_eq!(result.reached().len(), 1);
    assert_eq!(result.reached()[0].call, call);
    assert_eq!(
        result.returned().unwrap().content,
        format!("their number is {PHONE}")
    );
    assert!(result.operation().is_some());
    assert_eq!(
        g.c.session.admitted(&sources[2]).unwrap().origin(),
        RawOrigin::ToolResult
    );
    let reply = g.c.session.committed_model(&sources[3]).unwrap();
    let fields = reply.fields().unwrap().unwrap();
    assert_eq!(fields.len(), 1);
    assert_eq!(*fields[0].place(), FieldPlace::ReplyText);
    assert_eq!(fields[0].range(), 4..4 + placeholder.len());
    // A reply is not an admitted node, and an input is not a reply.
    assert!(g.c.session.admitted(&sources[3]).is_none());
    assert!(g.c.session.committed_model(&sources[0]).is_none());
    assert!(other.session.committed_model(&sources[3]).is_none());
    let sent =
        String::from_utf8(server.received_requests().await.unwrap()[1].body.clone()).unwrap();
    assert!(!sent.contains(EMAIL) && !sent.contains(PHONE));

    // Approved with arguments a person wrote: the tool gets those, they are
    // not treated as the model's, and the reply stays as received.
    let server = MockServer::start().await;
    let g = gated(&vault, &server, "done").await;
    let node = g.c.session.admit_raw(&draft(&text)).await.unwrap();
    let placeholder = issued(&node);
    scripted(
        &server,
        vec![
            asks(json!({"who": placeholder})),
            answer("chat", &["fixture answer"]),
        ],
    )
    .await;
    g.gate.park.store(true, Ordering::SeqCst);
    let TurnResult::Suspended(parked) = g
        .agent
        .run_turn_with_view(text.clone(), g.c.session.source(&node).unwrap())
        .await
        .unwrap()
    else {
        panic!("the call is parked");
    };
    g.gate.park.store(false, Ordering::SeqCst);
    let call = parked.request.tool_call.clone();
    let edit = HitlAction::ApproveEdited {
        args: json!({"who": placeholder, "typed": EMAIL}),
    };
    let decision = g.c.session.human_decision(&call, &edit).unwrap();
    g.agent
        .resume_with_view(edit, "Denied.", decision)
        .await
        .unwrap();
    {
        let seen = g.tool.seen.lock().unwrap();
        assert_eq!(seen.args, [json!({"who": placeholder, "typed": EMAIL})]);
        assert_eq!(seen.edited, [true]);
        assert!(seen.fields[0].is_empty());
    }
    assert!(
        matches!(&recorded(&g.agent)[1], Msg::Assistant { tool_calls, .. }
        if tool_calls[0] == call)
    );

    // Denied with a reason: nothing runs, the reason is recorded as written
    // and the model sees its view.
    let server = MockServer::start().await;
    let g = gated(&vault, &server, "unused").await;
    let node = g.c.session.admit_raw(&draft(&text)).await.unwrap();
    scripted(
        &server,
        vec![
            asks(json!({"who": "them"})),
            answer("chat", &["fixture answer"]),
        ],
    )
    .await;
    g.gate.park.store(true, Ordering::SeqCst);
    let TurnResult::Suspended(parked) = g
        .agent
        .run_turn_with_view(text.clone(), g.c.session.source(&node).unwrap())
        .await
        .unwrap()
    else {
        panic!("the call is parked");
    };
    let deny = HitlAction::Deny {
        feedback: format!("call {PHONE} first"),
    };
    let decision =
        g.c.session
            .human_decision(&parked.request.tool_call, &deny)
            .unwrap();
    g.agent
        .resume_with_view(deny, "The call was denied.", decision)
        .await
        .unwrap();
    assert!(g.tool.seen.lock().unwrap().args.is_empty());
    assert_eq!(
        recorded(&g.agent)[2],
        Msg::Tool {
            tool_call_id: "c1".into(),
            content: format!("The call was denied. Feedback: call {PHONE} first"),
            is_error: true,
        }
    );
    let sent = bodies(&server).await;
    let denial = sent[1]["messages"][3]["content"].as_str().unwrap();
    assert!(denial.starts_with("The call was denied. Feedback: call ") && !denial.contains(PHONE));
    let sources = g.agent.recorded_sources().unwrap();
    assert_eq!(
        g.c.session
            .admitted(sources[2].as_ref().unwrap())
            .unwrap()
            .origin(),
        RawOrigin::HumanReply
    );
}

#[tokio::test]
async fn a_call_the_committed_reply_did_not_ask_for_is_refused_before_it_runs() {
    let root = tempfile::tempdir().unwrap();
    let vault = vault(&root).await;
    let server = server().await;
    let c = conversation(&vault, &server, "unused").await;
    let call = ToolCall {
        id: "c9".into(),
        name: "lookup".into(),
        args: json!({"who": "anyone"}),
    };
    // No reply has been committed, so no call was asked for: neither an
    // ordinary dispatch nor an approved one is let through.
    assert!(c.boundary.before_dispatch(&call, false).await.is_err());
    assert_eq!(c.session.check(), Err(SessionStop::Closed));
    assert!(c.boundary.operation_in_flight().is_none());

    let d = conversation(&vault, &server, "unused").await;
    assert!(d.boundary.before_dispatch(&call, true).await.is_err());
    assert_eq!(d.session.check(), Err(SessionStop::Closed));

    // Text from another agent has no source here.
    let e = conversation(&vault, &server, "unused").await;
    assert!(
        e.boundary
            .observe_framework_commit(
                craftsman_harness::provenance::FrameworkCommit::Notification,
                &Msg::user("[message from agent 'child']\nhello"),
            )
            .await
            .is_err()
    );
    assert_eq!(e.session.check(), Err(SessionStop::Closed));
}

// ---- native wires: receipts, and the protected call repair -------------------

use crate::provider_config::Protocol;
use craftsman_harness::ModelResponse;
use craftsman_harness::builtin::PatchToolCallsMiddleware;
use craftsman_harness::provenance::{ModelDelivery, TrackedModelRequest};
use std::sync::atomic::AtomicUsize;

/// One reply of a native wire's own stream, asking for `lookup` or answering.
/// The first reply carries an item only the provider can read.
fn native_reply(wire: &str, index: usize, asks: bool) -> ResponseTemplate {
    let args = json!({"who": "them"});
    let raw = match wire {
        "responses" => {
            let mut output = vec![];
            if index == 0 {
                output.push(json!({"type":"reasoning","id":"wire-reasoning","encrypted_content":"wire-original-opaque","summary":[]}));
            }
            output.push(if asks {
                json!({"type":"function_call","id":"wire-function","call_id":"c1","name":"lookup","arguments":args.to_string(),"status":"completed"})
            } else {
                json!({"type":"message","id":"wire-message","role":"assistant","status":"completed","content":[{"type":"output_text","text":"fixture answer","annotations":[]}]})
            });
            let value = json!({"type":"response.completed","response":{"id":"wire-response","status":"completed","output":output,"usage":{"input_tokens":3,"output_tokens":2}}});
            format!("event: response.completed\ndata: {value}\n\n")
        }
        "claude" => {
            let mut events =
                vec![json!({"type":"message_start","message":{"usage":{"input_tokens":3}}})];
            if index == 0 {
                events.push(json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}));
                events.push(json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"wire fixture reasoning"}}));
                events.push(json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"wire-original-signature"}}));
                events.push(json!({"type":"content_block_stop","index":0}));
            }
            let block = usize::from(index == 0);
            events.push(
                json!({"type":"content_block_start","index":block,"content_block":if asks {
                    json!({"type":"tool_use","id":"c1","name":"lookup","input":{}})
                } else {
                    json!({"type":"text","text":""})
                }}),
            );
            events.push(
                json!({"type":"content_block_delta","index":block,"delta":if asks {
                    json!({"type":"input_json_delta","partial_json":args.to_string()})
                } else {
                    json!({"type":"text_delta","text":"fixture answer"})
                }}),
            );
            events.push(json!({"type":"content_block_stop","index":block}));
            events.push(json!({"type":"message_delta","delta":{"stop_reason":if asks {"tool_use"} else {"end_turn"}},"usage":{"output_tokens":2}}));
            events.push(json!({"type":"message_stop"}));
            events
                .iter()
                .map(|event| format!("data: {event}\n\n"))
                .collect()
        }
        _ => {
            let mut part = if asks {
                json!({"functionCall":{"id":"c1","name":"lookup","args":args}})
            } else {
                json!({"text":"fixture answer"})
            };
            if index == 0 {
                part["thoughtSignature"] = json!("wire-original-signature");
            }
            let value = json!({"candidates":[{"content":{"parts":[part]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":3,"candidatesTokenCount":2}});
            format!("data: {value}\n\n")
        }
    };
    ResponseTemplate::new(200).set_body_raw(raw, "text/event-stream")
}

fn native_client(wire: &str, server: &MockServer, key: &str) -> LlmClient {
    let protocol = match wire {
        "responses" => Protocol::OpenaiResponses,
        "claude" => Protocol::AnthropicMessages,
        _ => Protocol::GeminiGenerateContent,
    };
    LlmClient::configured(
        &server.uri(),
        key,
        "fixture-model",
        protocol,
        "test/fixture-model",
        &BTreeMap::new(),
    )
    .unwrap()
}

/// Hands the first request to one provider and every later one to another,
/// as a conversation whose connection changed under it would.
struct Switching {
    first: Arc<dyn ModelProvider>,
    later: Arc<dyn ModelProvider>,
    requests: AtomicUsize,
}

#[async_trait::async_trait]
impl ModelProvider for Switching {
    async fn complete(&self, _: ModelRequest) -> Result<ModelResponse, AgentError> {
        Err(AgentError::Model("recorded text".into()))
    }

    async fn complete_with_view(
        &self,
        req: TrackedModelRequest,
    ) -> Result<ModelDelivery, AgentError> {
        match self.requests.fetch_add(1, Ordering::SeqCst) {
            0 => self.first.complete_with_view(req).await,
            _ => self.later.complete_with_view(req).await,
        }
    }
}

#[tokio::test]
async fn provider_items_go_back_only_through_the_client_that_received_them() {
    let root = tempfile::tempdir().unwrap();
    let vault = vault(&root).await;
    let text = format!("who handles {EMAIL}?");
    let silent = || -> crate::agent::TokenSink { Arc::new(|_: &str| {}) };

    // What happens after the first reply when the second request would go
    // through `later`: nothing is sent, and what really happened is kept.
    async fn stopped(c: &Conversation, agent: &Agent, text: &str, server: &MockServer) {
        let node = c.session.admit_raw(&draft(text)).await.unwrap();
        let error = agent
            .run_turn_with_view(text, c.session.source(&node).unwrap())
            .await
            .err()
            .expect("the second request is refused");
        assert!(matches!(error, AgentError::Model(_)));
        // The first reply, the call it asked for and the real result are
        // recorded; the real usage of that one reply is kept.
        let messages = recorded(agent);
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0], Msg::user(text));
        assert!(matches!(&messages[1], Msg::Assistant { tool_calls, .. }
            if tool_calls[0].name == "lookup"));
        assert!(
            matches!(&messages[2], Msg::Tool { content, is_error: false, .. }
            if content.contains(PHONE))
        );
        assert_eq!(*c.usage.lock().unwrap(), (3, 2));
        // One request left the process, and the conversation is over.
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
        assert_eq!(c.session.check(), Err(SessionStop::Closed));
    }

    for wire in ["responses", "claude", "gemini"] {
        // Another credential: the items were received by a different client.
        let server = MockServer::start().await;
        scripted(
            &server,
            vec![native_reply(wire, 0, true), native_reply(wire, 1, false)],
        )
        .await;
        let c = conversation(&vault, &server, "unused").await;
        let provider = |key: &str| {
            let client = native_client(wire, &server, key);
            let (session, usage) = (c.session.clone(), c.usage.clone());
            async move {
                Arc::new(
                    ViewNativeProvider::new(client, session, usage, Arc::new(|_: &str| {}))
                        .await
                        .unwrap(),
                ) as Arc<dyn ModelProvider>
            }
        };
        let agent = Agent::builder(Arc::new(Switching {
            first: provider("fixture-key").await,
            later: provider("another-key").await,
            requests: AtomicUsize::new(0),
        }))
        .tool(Arc::new(Lookup(format!("their number is {PHONE}"))))
        .model_view_boundary(Some(c.boundary.clone()))
        .build();
        stopped(&c, &agent, &text, &server).await;

        // The same client throughout: the item goes back exactly.
        let server = MockServer::start().await;
        scripted(
            &server,
            vec![native_reply(wire, 0, true), native_reply(wire, 1, false)],
        )
        .await;
        let c = conversation(&vault, &server, "unused").await;
        let provider = ViewNativeProvider::new(
            native_client(wire, &server, "fixture-key"),
            c.session.clone(),
            c.usage.clone(),
            silent(),
        )
        .await
        .unwrap();
        let agent = Agent::builder(Arc::new(provider))
            .tool(Arc::new(Lookup(format!("their number is {PHONE}"))))
            .model_view_boundary(Some(c.boundary.clone()))
            .build();
        let node = c.session.admit_raw(&draft(&text)).await.unwrap();
        agent
            .run_turn_with_view(text.clone(), c.session.source(&node).unwrap())
            .await
            .unwrap();
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 2);
        let second = String::from_utf8(requests[1].body.clone()).unwrap();
        let signed = if wire == "responses" {
            "wire-original-opaque"
        } else {
            "wire-original-signature"
        };
        assert!(second.contains(signed), "{wire}: {second}");
        for original in [EMAIL, PHONE, OPERATOR] {
            assert!(!second.contains(original), "{wire}");
        }
        assert_eq!(*c.usage.lock().unwrap(), (6, 4));
    }

    // A reply that no native client of this conversation received has no
    // items to send back on a native wire.
    let chat = server().await;
    let native = MockServer::start().await;
    let c = conversation(&vault, &chat, "unused").await;
    let first = ViewChatProvider::new(client(&chat), c.session.clone(), c.usage.clone(), silent())
        .await
        .unwrap();
    let later = ViewNativeProvider::new(
        native_client("claude", &native, "fixture-key"),
        c.session.clone(),
        c.usage.clone(),
        silent(),
    )
    .await
    .unwrap();
    let agent = Agent::builder(Arc::new(Switching {
        first: Arc::new(first),
        later: Arc::new(later),
        requests: AtomicUsize::new(0),
    }))
    .tool(Arc::new(Lookup(format!("their number is {PHONE}"))))
    .model_view_boundary(Some(c.boundary.clone()))
    .build();
    let node = c.session.admit_raw(&draft(&text)).await.unwrap();
    assert!(
        agent
            .run_turn_with_view(text.clone(), c.session.source(&node).unwrap())
            .await
            .is_err()
    );
    assert_eq!(recorded(&agent).len(), 3);
    assert_eq!(*c.usage.lock().unwrap(), (5, 1));
    assert!(native.received_requests().await.unwrap().is_empty());
    assert_eq!(c.session.check(), Err(SessionStop::Closed));

    // And the other way: a reply a native client received is not sent on
    // the Chat wire without its items.
    let native = MockServer::start().await;
    scripted(&native, vec![native_reply("gemini", 0, true)]).await;
    let chat = MockServer::start().await;
    let c = conversation(&vault, &native, "unused").await;
    let first = ViewNativeProvider::new(
        native_client("gemini", &native, "fixture-key"),
        c.session.clone(),
        c.usage.clone(),
        silent(),
    )
    .await
    .unwrap();
    let later = ViewChatProvider::new(client(&chat), c.session.clone(), c.usage.clone(), silent())
        .await
        .unwrap();
    let agent = Agent::builder(Arc::new(Switching {
        first: Arc::new(first),
        later: Arc::new(later),
        requests: AtomicUsize::new(0),
    }))
    .tool(Arc::new(Lookup(format!("their number is {PHONE}"))))
    .model_view_boundary(Some(c.boundary.clone()))
    .build();
    stopped(&c, &agent, &text, &native).await;
    assert!(chat.received_requests().await.unwrap().is_empty());

    // A conversation has one agent: its boundary is handed out once.
    assert_eq!(c.session.boundary(&[]).err(), Some(SessionStop::Unbound));
    // A client with no native transport is not a native provider, and a
    // native client is not a Chat provider.
    let d = conversation(&vault, &chat, "unused").await;
    assert!(
        ViewNativeProvider::new(client(&chat), d.session.clone(), d.usage.clone(), silent())
            .await
            .is_err()
    );
    assert!(
        ViewChatProvider::new(
            native_client("claude", &chat, "fixture-key"),
            d.session.clone(),
            d.usage.clone(),
            silent()
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn a_repaired_call_keeps_what_was_received_and_what_the_repair_changed() {
    let root = tempfile::tempdir().unwrap();
    let vault = vault(&root).await;
    let server = MockServer::start().await;
    let c = conversation(&vault, &server, "unused").await;
    let text = format!("who handles {EMAIL}?");
    let node = c.session.admit_raw(&draft(&text)).await.unwrap();
    let placeholder = issued(&node);
    // The model writes arguments that are not a JSON object.
    let malformed = format!("ask {placeholder} now{{");
    scripted(
        &server,
        vec![
            sse(json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": "c1", "type": "function",
                "function": {"name": "lookup", "arguments": malformed}}]}, "finish_reason": "tool_calls"}],
                "usage": {"prompt_tokens": 5, "completion_tokens": 1}})),
            answer("chat", &["fixture answer"]),
        ],
    )
    .await;
    let tool = Arc::new(Inspect {
        boundary: Mutex::new(Some(c.boundary.clone())),
        seen: Mutex::new(Seen::default()),
        result: "done".into(),
    });
    let provider = ViewChatProvider::new(
        client(&server),
        c.session.clone(),
        c.usage.clone(),
        Arc::new(|_: &str| {}),
    )
    .await
    .unwrap();
    let agent = Agent::builder(Arc::new(provider))
        .tool(tool.clone())
        .middleware(Arc::new(PatchToolCallsMiddleware))
        .model_view_boundary(Some(c.boundary.clone()))
        .build();
    agent
        .run_turn_with_view(text.clone(), c.session.source(&node).unwrap())
        .await
        .unwrap();

    // The record holds the repaired call, as an ordinary session records it.
    let repaired = json!({"_raw": malformed});
    let messages = recorded(&agent);
    assert!(matches!(&messages[1], Msg::Assistant { tool_calls, .. }
        if tool_calls[0].args == repaired));
    // The conversation keeps what was received, what the repair changed, and
    // which middleware changed it.
    let sources = agent.recorded_sources().unwrap();
    let reply = c
        .session
        .committed_model(sources[1].as_ref().unwrap())
        .unwrap();
    assert_eq!(reply.message().unwrap(), &messages[1]);
    assert!(
        matches!(reply.received_message().unwrap(), Msg::Assistant { tool_calls, .. }
        if tool_calls[0].args == Value::String(malformed.clone()))
    );
    let edits = reply.edits().unwrap();
    assert_eq!(edits.len(), 1);
    assert_eq!(
        (
            edits[0].by.as_str(),
            edits[0].call,
            &edits[0].received,
            &edits[0].replaced
        ),
        (
            "patch_tool_calls",
            0,
            &Value::String(malformed.clone()),
            &repaired
        )
    );
    {
        // The repaired call is what was checked and run, and the placeholder
        // the model wrote is found where the repair put it.
        let seen = tool.seen.lock().unwrap();
        assert_eq!(seen.args, [repaired.clone()]);
        assert_eq!(seen.edited, [false]);
        assert_eq!(
            seen.fields[0],
            [(
                FieldPlace::Argument {
                    tool: "lookup".into(),
                    pointer: "/_raw".into(),
                },
                4..4 + placeholder.len(),
                EMAIL.to_owned(),
            )]
        );
    }
    // The next request carries the repaired call, and no original.
    let sent = bodies(&server).await;
    assert_eq!(
        sent[1]["messages"][2]["tool_calls"][0]["function"]["arguments"],
        repaired.to_string()
    );
    let raw = String::from_utf8(server.received_requests().await.unwrap()[1].body.clone()).unwrap();
    assert!(!raw.contains(EMAIL));
}

#[tokio::test]
async fn what_a_client_adds_to_every_request_is_admitted_and_may_hold_no_value() {
    let root = tempfile::tempdir().unwrap();
    let vault = vault(&root).await;
    let server = MockServer::start().await;
    let silent = || -> crate::agent::TokenSink { Arc::new(|_: &str| {}) };
    // A configured setting that holds a supported value cannot be replaced
    // without changing what it means: the provider is refused and the
    // conversation that was offered ends before any request.
    let c = conversation(&vault, &server, "unused").await;
    let setting = BTreeMap::from([("stop_sequences".to_owned(), json!([EMAIL]))]);
    let native = LlmClient::configured(
        &server.uri(),
        "fixture-key",
        "fixture-model",
        Protocol::AnthropicMessages,
        "test/fixture-model",
        &setting,
    )
    .unwrap();
    assert_eq!(
        ViewNativeProvider::new(native, c.session.clone(), c.usage.clone(), silent())
            .await
            .err(),
        Some(SessionStop::Unbound)
    );
    assert_eq!(c.session.check(), Err(SessionStop::Closed));

    let c = conversation(&vault, &server, "unused").await;
    let setting = BTreeMap::from([("user".to_owned(), json!(EMAIL))]);
    let chat = LlmClient::new_with_options(&server.uri(), "fixture-key", "fixture-model", &setting);
    assert_eq!(
        ViewChatProvider::new(chat, c.session.clone(), c.usage.clone(), silent())
            .await
            .err(),
        Some(SessionStop::Unbound)
    );
    assert_eq!(c.session.check(), Err(SessionStop::Closed));

    // Ordinary settings are admitted as they are and sent as they are.
    let c = conversation(&vault, &server, "unused").await;
    let setting = BTreeMap::from([("temperature".to_owned(), json!(0.5))]);
    let native = LlmClient::configured(
        &server.uri(),
        "fixture-key",
        "fixture-model",
        Protocol::OpenaiResponses,
        "test/fixture-model",
        &setting,
    )
    .unwrap();
    assert!(
        ViewNativeProvider::new(native, c.session.clone(), c.usage.clone(), silent())
            .await
            .is_ok()
    );
    assert!(c.session.check().is_ok());
    assert!(server.received_requests().await.unwrap().is_empty());
}
