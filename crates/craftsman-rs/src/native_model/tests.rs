use super::*;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn request() -> ModelRequest {
    ModelRequest {
        system: "Test instructions".into(),
        messages: vec![Msg::user("Inspect the project")],
        tools: vec![craftsman_harness::ToolDef {
            name: "read_file".into(),
            description: "Read a file".into(),
            parameters: json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}),
        }],
    }
}
fn event(value: Value) -> String {
    format!("data: {value}\n\n")
}
fn responses(items: Vec<Value>, text: &str) -> String {
    let mut stream = String::new();
    if !text.is_empty() {
        stream.push_str(&event(
            json!({"type":"response.output_text.delta","delta":text}),
        ));
    }
    stream.push_str(&event(json!({"type":"response.completed","response":{"status":"completed","output":items,"usage":{"input_tokens":31,"output_tokens":12}}})));
    stream
}
fn message(text: &str) -> Value {
    json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":text}]})
}

#[tokio::test]
async fn responses_use_the_explicit_protocol_auth_tools_and_effort() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/responses"))
        .and(header("authorization", "Bearer synthetic-key"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            responses(vec![message("Hello")], "Hello"),
            "text/event-stream",
        ))
        .expect(1)
        .mount(&server)
        .await;
    let native = NativeClient::new(
        Protocol::OpenaiResponses,
        &server.uri(),
        "synthetic-key",
        "gpt-fixture",
        "openai/gpt-fixture",
        &BTreeMap::from([(
            "craftsman_reasoning".into(),
            json!({"type":"effort","value":"high"}),
        )]),
    )
    .unwrap();
    let mut visible = String::new();
    let response = native
        .complete(&request(), |text| visible.push_str(text))
        .await
        .unwrap();
    assert_eq!(visible, "Hello");
    assert_eq!(
        response.usage,
        Usage {
            input_tokens: 31,
            output_tokens: 12
        }
    );
    let requests = server.received_requests().await.unwrap();
    let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(body["model"], "gpt-fixture");
    assert_eq!(body["store"], false);
    assert_eq!(body["reasoning"]["effort"], "high");
    assert_eq!(body["tools"][0]["name"], "read_file");
    assert_eq!(body["tools"][0]["strict"], false);
    assert!(body.get("messages").is_none());
    assert!(body.get("craftsman_reasoning").is_none());
}

#[tokio::test]
async fn responses_reasoning_items_survive_checkpoint_restore_and_tool_results() {
    let server = MockServer::start().await;
    let items = vec![
        json!({"type":"reasoning","id":"rs_real","encrypted_content":"opaque-fixture","summary":[]}),
        json!({"type":"function_call","id":"fc_real","call_id":"call_real","name":"read_file","arguments":"{\"path\":\"README.md\"}"}),
    ];
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(responses(items.clone(), ""), "text/event-stream"),
        )
        .expect(1)
        .mount(&server)
        .await;
    let native = NativeClient::new(
        Protocol::OpenaiResponses,
        &server.uri(),
        "key",
        "fixture",
        "p/fixture",
        &BTreeMap::new(),
    )
    .unwrap();
    let req = request();
    let response = native.complete(&req, |_| {}).await.unwrap();
    let mut state = AgentState::default();
    state.messages = req.messages.clone();
    state.messages.push(response.message.clone());
    native.history.review_model(&mut state).await.unwrap();
    let restored: AgentState =
        serde_json::from_value(serde_json::to_value(&state).unwrap()).unwrap();
    let replacement = NativeClient::new(
        Protocol::OpenaiResponses,
        &server.uri(),
        "key",
        "fixture",
        "p/fixture",
        &BTreeMap::new(),
    )
    .unwrap();
    let mut restored = restored;
    replacement
        .history
        .prepare_model(&mut restored)
        .await
        .unwrap();
    restored
        .messages
        .push(Msg::tool("call_real", "File contents"));
    let body = replacement
        .request(&ModelRequest {
            messages: restored.messages.clone(),
            ..request()
        })
        .unwrap();
    assert_eq!(body["input"][1], items[0]);
    assert_eq!(body["input"][2], items[1]);
    assert_eq!(
        body["input"][3],
        json!({"type":"function_call_output","call_id":"call_real","output":"File contents"})
    );
    let other = NativeClient::new(
        Protocol::OpenaiResponses,
        &server.uri(),
        "key",
        "fixture",
        "other/fixture",
        &BTreeMap::new(),
    )
    .unwrap();
    other.history.prepare_model(&mut restored).await.unwrap();
    let body = other
        .request(&ModelRequest {
            messages: restored.messages,
            ..request()
        })
        .unwrap();
    assert!(
        !serde_json::to_string(&body)
            .unwrap()
            .contains("opaque-fixture")
    );
}

#[tokio::test]
async fn claude_uses_messages_blocks_and_preserves_signed_thinking() {
    let server = MockServer::start().await;
    let stream=[
        json!({"type":"message_start","message":{"usage":{"input_tokens":7,"cache_read_input_tokens":4}}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"opaque thought"}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"signed-fixture"}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"tool_real","name":"read_file","input":{}}}),
        json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":"}}),
        json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"\"README.md\"}"}}),
        json!({"type":"content_block_stop","index":1}),
        json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":9}}),
        json!({"type":"message_stop"}),
    ].into_iter().map(event).collect::<String>();
    Mock::given(method("POST"))
        .and(path("/messages"))
        .and(header("x-api-key", "synthetic-key"))
        .and(header("anthropic-version", "2026-10-21"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(stream, "text/event-stream"))
        .expect(1)
        .mount(&server)
        .await;
    let native = NativeClient::new(
        Protocol::AnthropicMessages,
        &server.uri(),
        "synthetic-key",
        "claude-fixture",
        "a/fixture",
        &BTreeMap::new(),
    )
    .unwrap();
    let req = request();
    let mut visible = String::new();
    let response = native
        .complete(&req, |text| visible.push_str(text))
        .await
        .unwrap();
    assert!(visible.is_empty());
    assert_eq!(
        response.usage,
        Usage {
            input_tokens: 11,
            output_tokens: 9
        }
    );
    let mut next = req;
    next.messages.push(response.message);
    next.messages.push(Msg::Tool {
        tool_call_id: "tool_real".into(),
        content: "Missing".into(),
        is_error: true,
    });
    let body = native.request(&next).unwrap();
    assert_eq!(
        body["messages"][1]["content"][0]["signature"],
        "signed-fixture"
    );
    assert_eq!(
        body["messages"][1]["content"][1]["input"]["path"],
        "README.md"
    );
    assert_eq!(body["messages"][2]["content"][0]["is_error"], true);
}

#[tokio::test]
async fn gemini_preserves_each_part_and_actual_call_identity() {
    let server = MockServer::start().await;
    let part = json!({"functionCall":{"id":"native-call","name":"read_file","args":{"path":"README.md"}},"thoughtSignature":"signature-fixture"});
    let stream = event(
        json!({"candidates":[{"content":{"parts":[part.clone()]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":17,"candidatesTokenCount":5,"thoughtsTokenCount":3}}),
    );
    Mock::given(method("POST"))
        .and(path("/models/gemini-fixture:streamGenerateContent"))
        .and(header("x-goog-api-key", "synthetic-key"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(stream, "text/event-stream"))
        .expect(1)
        .mount(&server)
        .await;
    let native = NativeClient::new(
        Protocol::GeminiGenerateContent,
        &server.uri(),
        "synthetic-key",
        "gemini-fixture",
        "g/fixture",
        &BTreeMap::new(),
    )
    .unwrap();
    let mut req = request();
    let response = native.complete(&req, |_| {}).await.unwrap();
    assert_eq!(
        response.usage,
        Usage {
            input_tokens: 17,
            output_tokens: 8
        }
    );
    req.messages.push(response.message);
    req.messages.push(Msg::tool("native-call", "Contents"));
    let body = native.request(&req).unwrap();
    assert_eq!(body["contents"][1]["parts"][0], part);
    assert_eq!(
        body["contents"][2]["parts"][0]["functionResponse"]["id"],
        "native-call"
    );
    assert_eq!(
        body["contents"][2]["parts"][0]["functionResponse"]["name"],
        "read_file"
    );
    assert_eq!(
        body["tools"][0]["functionDeclarations"][0]["parametersJsonSchema"],
        req.tools[0].parameters
    );
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests[0].url.query(), Some("alt=sse"));
    assert!(!requests[0].url.as_str().contains("synthetic-key"));
}

#[test]
fn unfinished_duplicate_and_invalid_native_tool_calls_cannot_dispatch() {
    let mut parser = Parser::new(Protocol::OpenaiResponses);
    parser
        .event(
            event(json!({"type":"response.output_text.delta","delta":"partial"}))
                .trim()
                .as_bytes(),
            &mut |_| {},
        )
        .unwrap();
    assert!(parser.finish().is_err());
    for items in [
        vec![json!({"type":"function_call","call_id":"c","name":"read_file","arguments":"[]"})],
        vec![
            json!({"type":"function_call","call_id":"c","name":"read_file","arguments":"{}"}),
            json!({"type":"function_call","call_id":"c","name":"read_file","arguments":"{}"}),
        ],
    ] {
        let mut parser = Parser::new(Protocol::OpenaiResponses);
        parser
            .event(responses(items, "").trim().as_bytes(), &mut |_| {})
            .unwrap();
        assert!(parser.finish().is_err());
    }
}

#[tokio::test]
async fn errors_are_fixed_and_response_metadata_is_not_forwarded_to_other_models() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(401).set_body_string("secret-provider-body"))
        .expect(1)
        .mount(&server)
        .await;
    let native = NativeClient::new(
        Protocol::OpenaiResponses,
        &server.uri(),
        "private-key",
        "fixture",
        "p/f",
        &BTreeMap::new(),
    )
    .unwrap();
    let error = native
        .complete(&request(), |_| {})
        .await
        .err()
        .unwrap()
        .to_string();
    assert_eq!(error, "Native model endpoint returned HTTP 401.");
    assert!(!error.contains("private-key"));
    assert!(!error.contains("secret-provider-body"));
    let mut state = AgentState::default();
    state
        .custom
        .insert(HISTORY_KEY.into(), json!({"version":999,"connections":{}}));
    assert!(native.history.prepare_model(&mut state).await.is_err());
}

#[test]
fn unsupported_generation_settings_refuse_before_network_work() {
    let native = NativeClient::new(
        Protocol::OpenaiResponses,
        "http://127.0.0.1:1",
        "key",
        "fixture",
        "p/f",
        &BTreeMap::from([("api_key".into(), json!("foreign-key"))]),
    )
    .unwrap();
    let error = native.request(&request()).unwrap_err().to_string();
    assert_eq!(error, "Unsupported native model generation setting.");
}

#[test]
fn tool_less_compaction_requests_omit_tools_and_gemini_uses_native_enum_values() {
    for protocol in [
        Protocol::OpenaiResponses,
        Protocol::AnthropicMessages,
        Protocol::GeminiGenerateContent,
    ] {
        let native = NativeClient::new(
            protocol,
            "http://127.0.0.1:1",
            "key",
            "fixture",
            "p/f",
            &BTreeMap::new(),
        )
        .unwrap();
        let body = native
            .request(&ModelRequest {
                tools: Vec::new(),
                ..request()
            })
            .unwrap();
        assert!(body.get("tools").is_none());
    }
    let native = NativeClient::new(
        Protocol::GeminiGenerateContent,
        "http://127.0.0.1:1",
        "key",
        "fixture",
        "p/f",
        &BTreeMap::from([(
            "craftsman_reasoning".into(),
            json!({"type":"effort","value":"medium"}),
        )]),
    )
    .unwrap();
    assert_eq!(
        native.request(&request()).unwrap()["generationConfig"]["thinkingConfig"]["thinkingLevel"],
        "MEDIUM"
    );
}

#[tokio::test]
async fn corrupt_or_mutated_native_call_history_refuses_before_more_work() {
    let native = NativeClient::new(
        Protocol::OpenaiResponses,
        "http://127.0.0.1:1",
        "key",
        "fixture",
        "p/f",
        &BTreeMap::new(),
    )
    .unwrap();
    let message = Msg::Assistant {
        content: String::new(),
        tool_calls: vec![ToolCall {
            id: "c".into(),
            name: "read_file".into(),
            args: json!({"path":"safe.txt"}),
        }],
    };
    let record = Record {
        index: 1,
        message: message.clone(),
        items: vec![
            json!({"type":"function_call","call_id":"c","name":"read_file","arguments":"{\"path\":\"different.txt\"}"}),
        ],
    };
    let mut state = AgentState::default();
    state.messages = vec![Msg::user("read"), message];
    let mut connections = serde_json::Map::new();
    connections.insert(
        native.history.owner.clone(),
        serde_json::to_value(vec![record.clone()]).unwrap(),
    );
    state.custom.insert(
        HISTORY_KEY.into(),
        json!({"version":1,"connections":connections}),
    );
    assert!(native.history.prepare_model(&mut state).await.is_err());
    native.history.records.lock().unwrap().push(record);
    if let Msg::Assistant { tool_calls, .. } = &mut state.messages[1] {
        tool_calls[0].args = json!({"path":"changed.txt"});
    }
    assert!(native.history.review_model(&mut state).await.is_err());
}

#[tokio::test]
async fn configured_compatible_errors_do_not_echo_provider_bodies() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(401).set_body_string("secret-credential-echo"))
        .expect(1)
        .mount(&server)
        .await;
    let client = LlmClient::configured(
        &server.uri(),
        "synthetic-key",
        "fixture",
        Protocol::OpenaiChatCompletions,
        "p/f",
        &BTreeMap::new(),
    )
    .unwrap();
    let error = client
        .stream_turn(
            &[crate::llm::Message::User {
                content: "hi".into(),
            }],
            &[],
            |_| {},
            |_| {},
        )
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(
        error,
        "Configured Chat Completions request failed; verify endpoint, model and credentials."
    );
    assert!(!error.contains("secret-credential-echo"));
}

#[tokio::test]
async fn cancelling_an_actual_native_request_keeps_the_next_turn_usable() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_delay(std::time::Duration::from_secs(60)))
        .expect(1)
        .mount(&server)
        .await;
    let client = LlmClient::configured(
        &server.uri(),
        "synthetic-key",
        "fixture",
        Protocol::OpenaiResponses,
        "p/f",
        &BTreeMap::new(),
    )
    .unwrap();
    let usage = Arc::new(Mutex::new((0, 0)));
    let provider = Arc::new(crate::agent::StreamingLlmProvider {
        client,
        usage: usage.clone(),
        sink: Arc::new(|_| {}),
        round_sink: None,
        status_sink: None,
    });
    let agent = craftsman_harness::Agent::builder(provider.clone())
        .interrupt_model_on_cancel(true)
        .build();
    let mut turn = Box::pin(agent.run_turn("first"));
    let ready = async {
        loop {
            if server.received_requests().await.unwrap().len() == 1 {
                break;
            }
            tokio::task::yield_now().await;
        }
    };
    tokio::select! {
        result=&mut turn=>panic!("native request ended before cancellation: {result:?}"),
        ready=tokio::time::timeout(std::time::Duration::from_secs(5),ready)=>ready.unwrap(),
    }
    agent.request_cancel();
    let result = tokio::time::timeout(std::time::Duration::from_secs(2), &mut turn)
        .await
        .unwrap()
        .unwrap();
    let craftsman_harness::TurnResult::Done(output) = result else {
        panic!("native cancellation did not finish the turn");
    };
    assert_eq!(output.final_text(), "Stopped by the user.");
    drop(turn);
    assert!(
        provider
            .client
            .native
            .as_ref()
            .unwrap()
            .history
            .records
            .lock()
            .unwrap()
            .is_empty()
    );
    assert_eq!(*usage.lock().unwrap(), (0, 0));
    server.verify().await;
    server.reset().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            responses(vec![message("next-ready")], "next-ready"),
            "text/event-stream",
        ))
        .expect(1)
        .mount(&server)
        .await;
    let result = tokio::time::timeout(std::time::Duration::from_secs(2), agent.run_turn("second"))
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(result, craftsman_harness::TurnResult::Done(_)));
    assert_eq!(*usage.lock().unwrap(), (31, 12));
}

#[test]
fn incomplete_claude_blocks_and_mismatched_final_text_are_refused() {
    let mut parser = Parser::new(Protocol::AnthropicMessages);
    for value in [
        json!({"type":"message_start","message":{"usage":{"input_tokens":1}}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"c","name":"read_file","input":{}}}),
        json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":1}}),
    ] {
        parser
            .event(event(value).trim().as_bytes(), &mut |_| {})
            .unwrap();
    }
    assert!(
        parser
            .event(
                event(json!({"type":"message_stop"})).trim().as_bytes(),
                &mut |_| {}
            )
            .is_err()
    );
    let mut parser = Parser::new(Protocol::OpenaiResponses);
    parser
        .event(
            event(json!({"type":"response.output_text.delta","delta":"partial"}))
                .trim()
                .as_bytes(),
            &mut |_| {},
        )
        .unwrap();
    assert!(
        parser
            .event(
                responses(vec![message("different")], "").trim().as_bytes(),
                &mut |_| {}
            )
            .is_err()
    );
}
