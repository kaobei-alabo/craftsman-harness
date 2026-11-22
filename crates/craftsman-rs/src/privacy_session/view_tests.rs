//! One conversation from a raw capture to the Chat wire and back, through
//! the framework's agent, this session's boundary, the private Chat provider
//! and the existing client and guard, against a loopback server. The vault
//! and the guard are real; the detector is fixed host reply rules.

use super::tests::{draft, vault};
use super::*;
use crate::llm::LlmClient;
use crate::privacy_hook::tests::{EMAIL, LiteralSource, PHONE};
use crate::privacy_provider::ViewChatProvider;
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
        .err(),
        Some(SessionStop::Guard(_))
    ));

    // A reply received by another conversation cannot be committed here.
    let reply = Msg::assistant("a reply");
    let receipt = SourceRef::new(Arc::new(other.session.received_model(&reply).unwrap()));
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
