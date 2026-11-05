//! A scripted OpenAI-compatible model server for the golden-frame tests.
//!
//! The agent under test streams `POST /chat/completions`. Each request is
//! answered with the next scripted [`Round`], chosen by request count rather
//! than by message content, so a script reads top to bottom like the
//! conversation it stands in for. Asserting on what the agent actually sent is
//! left to the test, through [`MockModel::requests`].

use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

/// One streamed model response.
pub enum Round {
    /// Text deltas, then a normal stop.
    Text {
        deltas: Vec<String>,
        usage: (u64, u64),
    },
    /// Optional text deltas, then a single tool call.
    ToolCall {
        preface: Vec<String>,
        id: String,
        name: String,
        arguments: Value,
        usage: (u64, u64),
    },
    /// Hold the response back for this long, so a test can cancel mid-call.
    Stall(Duration),
}

impl Round {
    pub fn text(deltas: &[&str], usage: (u64, u64)) -> Round {
        Round::Text {
            deltas: deltas.iter().map(|delta| (*delta).to_string()).collect(),
            usage,
        }
    }

    pub fn tool_call(
        preface: &[&str],
        id: &str,
        name: &str,
        arguments: Value,
        usage: (u64, u64),
    ) -> Round {
        Round::ToolCall {
            preface: preface.iter().map(|delta| (*delta).to_string()).collect(),
            id: id.to_string(),
            name: name.to_string(),
            arguments,
            usage,
        }
    }

    fn template(&self) -> ResponseTemplate {
        let (events, delay) = match self {
            Round::Text { deltas, usage } => {
                let mut events: Vec<Value> = deltas.iter().map(|delta| content(delta)).collect();
                events.push(finish("stop"));
                events.push(usage_chunk(*usage));
                (events, None)
            }
            Round::ToolCall {
                preface,
                id,
                name,
                arguments,
                usage,
            } => {
                let mut events: Vec<Value> = preface.iter().map(|delta| content(delta)).collect();
                // The call id and name arrive first, the arguments in two
                // fragments after them, as real providers stream them.
                events.push(tool_delta(json!({
                    "index": 0,
                    "id": id,
                    "type": "function",
                    "function": {"name": name, "arguments": ""},
                })));
                let arguments = arguments.to_string();
                let (head, tail) = split_in_half(&arguments);
                for fragment in [head, tail] {
                    events.push(tool_delta(json!({
                        "index": 0,
                        "function": {"arguments": fragment},
                    })));
                }
                events.push(finish("tool_calls"));
                events.push(usage_chunk(*usage));
                (events, None)
            }
            Round::Stall(delay) => (vec![content("too late")], Some(*delay)),
        };
        let template = ResponseTemplate::new(200)
            .insert_header("content-type", "text/event-stream")
            .set_body_string(sse(&events));
        match delay {
            Some(delay) => template.set_delay(delay),
            None => template,
        }
    }
}

fn content(delta: &str) -> Value {
    json!({"choices": [{"index": 0, "delta": {"content": delta}}]})
}

fn tool_delta(call: Value) -> Value {
    json!({"choices": [{"index": 0, "delta": {"tool_calls": [call]}}]})
}

fn finish(reason: &str) -> Value {
    json!({"choices": [{"index": 0, "delta": {}, "finish_reason": reason}]})
}

fn usage_chunk((input, output): (u64, u64)) -> Value {
    json!({"choices": [], "usage": {"prompt_tokens": input, "completion_tokens": output}})
}

fn sse(events: &[Value]) -> String {
    let mut body = String::new();
    for event in events {
        body.push_str("data: ");
        body.push_str(&event.to_string());
        body.push_str("\n\n");
    }
    body.push_str("data: [DONE]\n\n");
    body
}

fn split_in_half(text: &str) -> (&str, &str) {
    let middle = text
        .char_indices()
        .map(|(index, _)| index)
        .nth(text.chars().count() / 2)
        .unwrap_or(text.len());
    text.split_at(middle)
}

struct Scripted {
    rounds: Vec<Round>,
    served: AtomicUsize,
}

impl Respond for Scripted {
    fn respond(&self, _request: &Request) -> ResponseTemplate {
        let index = self.served.fetch_add(1, Ordering::SeqCst);
        match self.rounds.get(index) {
            Some(round) => round.template(),
            // An unscripted extra request must be loud, not silently fed text.
            None => ResponseTemplate::new(500).set_body_string("scripted model: no round left"),
        }
    }
}

pub struct MockModel {
    server: MockServer,
}

impl MockModel {
    pub async fn start(rounds: Vec<Round>) -> MockModel {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(Scripted {
                rounds,
                served: AtomicUsize::new(0),
            })
            .mount(&server)
            .await;
        MockModel { server }
    }

    /// Base URL to put in the agent's `base_url`.
    pub fn url(&self) -> String {
        self.server.uri()
    }

    /// The JSON bodies of the requests received so far, oldest first.
    pub async fn requests(&self) -> Vec<Value> {
        self.server
            .received_requests()
            .await
            .unwrap_or_default()
            .iter()
            .map(|request| serde_json::from_slice(&request.body).expect("model request is JSON"))
            .collect()
    }
}
