//! Loopback fixtures shared by the privacy conversation tests: one owned
//! configuration per model wire, and one scripted reply in each wire's own
//! stream format. The first reply of a native wire carries an item only the
//! provider can read: encrypted reasoning, a signed thinking block or a
//! thought signature.

use crate::config::Config;
use serde_json::{Value, json};
use wiremock::{MockServer, ResponseTemplate};

/// An owned configuration that points one wire at `server`.
pub(crate) fn config(wire: &str, server: &MockServer) -> Config {
    let shared = r#""harness_profile":"general","voice":{"enabled":false},"gadgets":{"enabled":false},"mcp_servers":[],"lsp_servers":{}"#;
    let uri = server.uri();
    let text = match wire {
        "chat" => {
            format!(
                r#"{{"base_url":"{uri}","api_key":"fixture-key","model":"fixture-model",{shared}}}"#
            )
        }
        _ => {
            let protocol = match wire {
                "responses" => "openai_responses",
                "claude" => "anthropic_messages",
                _ => "gemini_generate_content",
            };
            format!(
                r#"{{"model":"test/fixture-model","providers":{{"test":{{"protocol":"{protocol}","base_url":"{uri}","api_key":"fixture-key","models":{{"fixture-model":{{"settings":{{"reasoning":null}}}}}}}}}},{shared}}}"#
            )
        }
    };
    serde_json::from_str(&text).unwrap()
}

/// One reply: a call `(id, tool, arguments)` or the answer `text`.
pub(crate) fn reply(
    wire: &str,
    index: usize,
    call: Option<(&str, &str, &Value)>,
    text: &str,
) -> ResponseTemplate {
    let raw = match wire {
        "responses" => {
            let mut output = Vec::new();
            if index == 0 {
                output.push(json!({"type":"reasoning","id":"wire-reasoning","encrypted_content":"wire-original-opaque","summary":[]}));
            }
            output.push(match call {
                Some((id, tool, args)) => json!({"type":"function_call","id":format!("wire-function-{index}"),"call_id":id,"name":tool,"arguments":args.to_string(),"status":"completed"}),
                None => json!({"type":"message","id":format!("wire-message-{index}"),"role":"assistant","status":"completed","content":[{"type":"output_text","text":text,"annotations":[]}]}),
            });
            let value = json!({"type":"response.completed","response":{"id":format!("wire-response-{index}"),"status":"completed","output":output,"usage":{"input_tokens":3,"output_tokens":2}}});
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
            let (start, delta, stop) = match call {
                Some((id, tool, args)) => (
                    json!({"type":"tool_use","id":id,"name":tool,"input":{}}),
                    json!({"type":"input_json_delta","partial_json":args.to_string()}),
                    "tool_use",
                ),
                None => (
                    json!({"type":"text","text":""}),
                    json!({"type":"text_delta","text":text}),
                    "end_turn",
                ),
            };
            events.push(json!({"type":"content_block_start","index":block,"content_block":start}));
            events.push(json!({"type":"content_block_delta","index":block,"delta":delta}));
            events.push(json!({"type":"content_block_stop","index":block}));
            events.push(json!({"type":"message_delta","delta":{"stop_reason":stop},"usage":{"output_tokens":2}}));
            events.push(json!({"type":"message_stop"}));
            events
                .iter()
                .map(|event| format!("data: {event}\n\n"))
                .collect()
        }
        "gemini" => {
            let mut part = match call {
                Some((id, tool, args)) => json!({"functionCall":{"id":id,"name":tool,"args":args}}),
                None => json!({"text":text}),
            };
            if index == 0 {
                part["thoughtSignature"] = json!("wire-original-signature");
            }
            let value = json!({"candidates":[{"content":{"parts":[part]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":3,"candidatesTokenCount":2}});
            format!("data: {value}\n\n")
        }
        _ => {
            let (delta, finish) = match call {
                Some((id, tool, args)) => (
                    json!({"tool_calls":[{"index":0,"id":id,"type":"function","function":{"name":tool,"arguments":args.to_string()}}]}),
                    "tool_calls",
                ),
                None => (json!({"content":text}), "stop"),
            };
            let value = json!({"choices":[{"delta":delta,"finish_reason":finish}],"usage":{"prompt_tokens":3,"completion_tokens":2}});
            format!("data: {value}\n\ndata: [DONE]\n\n")
        }
    };
    ResponseTemplate::new(200).set_body_raw(raw, "text/event-stream")
}
