//! Marked real-builder/HTTP/file/SQLite context proof. No hosted provider calls.
use super::*;
use craftsman_harness::{Checkpointer, Tool, ToolCtx, ToolOutcome, TurnResult};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::Path;
use std::process::Stdio;
use std::sync::Condvar;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::sync::Notify;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const HELPER: &str = "agent::context_control_tests::native_context_control_fixture";
const MARKER: &str = "craftsman-context-control=";
const DONE: &str =
    "actual context input, HTTP, file, checkpoint and lifecycle assertions completed";
const CONTEXT_KEY: &str = "_craftsman_context_v1";
const INITIAL: &str = "Complete the actual context fixture task.";

fn flat_config(base: String, context: Option<Value>) -> Config {
    let mut value = json!({"base_url":base,"api_key":"context-fixture-key","model":"context-fixture",
        "harness_profile":"general","voice":{"enabled":false},"gadgets":{"enabled":false},
        "mcp_servers":[],"lsp_servers":{},"permission_mode":"manual"});
    if let Some(context) = context {
        value["context"] = context;
    }
    serde_json::from_value(value).unwrap()
}

fn enabled() -> Value {
    json!({"enabled":true,"max_bytes":12288})
}

fn named_config(server: &MockServer, protocol: &str) -> Config {
    let protocol_name = match protocol {
        "chat" => "openai_chat_completions",
        "responses" => "openai_responses",
        "claude" => "anthropic_messages",
        _ => "gemini_generate_content",
    };
    let base = if protocol == "gemini" {
        server.uri()
    } else {
        format!("{}/v1", server.uri())
    };
    serde_json::from_value(json!({"model":"test/context-fixture","providers":{"test":{
        "protocol":protocol_name,"base_url":base,"api_key":"context-fixture-key",
        "models":{"context-fixture":{"settings":{"reasoning":null}}}}},
        "context":enabled(),"harness_profile":"general","voice":{"enabled":false},"gadgets":{"enabled":false},
        "mcp_servers":[],"lsp_servers":{},"permission_mode":"manual"})).unwrap()
}

fn save_config(cfg: &Config) -> (PathBuf, [u8; 32]) {
    let directory = PathBuf::from(std::env::var_os("CRAFTSMAN_CONFIG_DIR").unwrap());
    let path = directory.join("config.json");
    let bytes = serde_json::to_vec_pretty(cfg).unwrap();
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)
        .unwrap();
    file.write_all(&bytes).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .unwrap();
    }
    (path, Sha256::digest(&bytes).into())
}

fn config_preserved(saved: &(PathBuf, [u8; 32])) {
    let actual: [u8; 32] = Sha256::digest(std::fs::read(&saved.0).unwrap()).into();
    assert_eq!(actual, saved.1);
}

async fn build_fixture_session(
    cfg: &Config,
    directory: &Path,
    support: bool,
    denied: bool,
    extra_tools: Vec<Arc<dyn Tool>>,
) -> (Session, Arc<AtomicUsize>) {
    let approvals = Arc::new(AtomicUsize::new(0));
    let calls = approvals.clone();
    let session = build_session_ext(
        cfg,
        Arc::new(Mutex::new(ApprovalState::default())),
        "",
        false,
        false,
        None,
        SessionExtras {
            context_compilation: support,
            workspace_root: Some(directory.to_path_buf()),
            cancellation_session: Some(craftsman_harness::SessionCancellation::default()),
            token_sink: Some(Arc::new(|_| {})),
            activity_sink: Some(Arc::new(|_| {})),
            approval_prompt: Some(Arc::new(move |_, _, _| {
                calls.fetch_add(1, Ordering::SeqCst);
                if denied {
                    ApprovalPromptResult::Reject("retain the fixture file".into())
                } else {
                    ApprovalPromptResult::Approve
                }
            })),
            extra_tools,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    (session, approvals)
}

fn system<'a>(body: &'a Value, protocol: &str) -> &'a str {
    match protocol {
        "responses" => body["instructions"].as_str().unwrap(),
        "claude" => body["system"].as_str().unwrap(),
        "gemini" => body["systemInstruction"]["parts"][0]["text"]
            .as_str()
            .unwrap(),
        _ => body["messages"][0]["content"].as_str().unwrap(),
    }
}

fn context_payload(system: &str) -> Value {
    let start = system
        .find("{\"version\":1,\"initial_request\"")
        .expect("actual compiled context block");
    serde_json::Deserializer::from_str(&system[start..])
        .into_iter::<Value>()
        .next()
        .unwrap()
        .unwrap()
}

fn read_args() -> Value {
    json!({"file_path":"src/value.txt","offset":0,"limit":10})
}
fn edit_args() -> Value {
    json!({"file_path":"src/value.txt","old_string":"before","new_string":"after"})
}

fn response(
    protocol: &str,
    index: usize,
    tool: Option<(&str, Value)>,
    text: &str,
) -> ResponseTemplate {
    let raw = match protocol {
        "responses" => {
            let mut output = Vec::new();
            if index == 0 {
                output.push(json!({"type":"reasoning","id":"context-reasoning","encrypted_content":"context-original-opaque","summary":[]}));
            }
            output.push(match tool {
                Some((name,args))=>json!({"type":"function_call","id":format!("context-function-{index}"),"call_id":format!("context-call-{index}"),"name":name,"arguments":args.to_string(),"status":"completed"}),
                None=>json!({"type":"message","id":format!("context-message-{index}"),"role":"assistant","status":"completed","content":[{"type":"output_text","text":text,"annotations":[]}]}),
            });
            let value = json!({"type":"response.completed","response":{"id":format!("context-response-{index}"),"status":"completed","output":output,"usage":{"input_tokens":3,"output_tokens":2}}});
            format!("event: response.completed\ndata: {value}\n\n")
        }
        "claude" => {
            let mut events =
                vec![json!({"type":"message_start","message":{"usage":{"input_tokens":3}}})];
            if index == 0 {
                events.push(json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}));
                events.push(json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"context fixture reasoning"}}));
                events.push(json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"context-original-signature"}}));
                events.push(json!({"type":"content_block_stop","index":0}));
            }
            let block = usize::from(index == 0);
            events.push(json!({"type":"content_block_start","index":block,"content_block":match &tool {
                Some((name,_))=>json!({"type":"tool_use","id":format!("context-call-{index}"),"name":name,"input":{}}),
                None=>json!({"type":"text","text":""}),
            }}));
            events.push(json!({"type":"content_block_delta","index":block,"delta":match &tool {
                Some((_,args))=>json!({"type":"input_json_delta","partial_json":args.to_string()}),
                None=>json!({"type":"text_delta","text":text}),
            }}));
            events.push(json!({"type":"content_block_stop","index":block}));
            events.push(json!({"type":"message_delta","delta":{"stop_reason":if tool.is_some(){"tool_use"}else{"end_turn"}},"usage":{"output_tokens":2}}));
            events.push(json!({"type":"message_stop"}));
            events
                .iter()
                .map(|event| format!("data: {event}\n\n"))
                .collect()
        }
        "gemini" => {
            let mut part = match tool {
                Some((name, args)) => {
                    json!({"functionCall":{"id":format!("context-call-{index}"),"name":name,"args":args}})
                }
                None => json!({"text":text}),
            };
            if index == 0 {
                part["thoughtSignature"] = json!("context-original-signature");
            }
            let value = json!({"candidates":[{"content":{"parts":[part]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":3,"candidatesTokenCount":2}});
            format!("data: {value}\n\n")
        }
        _ => {
            let delta = match &tool {
                Some((name, args)) => {
                    json!({"tool_calls":[{"index":0,"id":format!("context-call-{index}"),"type":"function","function":{"name":name,"arguments":args.to_string()}}]})
                }
                None => json!({"content":text}),
            };
            let value = json!({"choices":[{"delta":delta,"finish_reason":if tool.is_some(){"tool_calls"}else{"stop"}}],"usage":{"prompt_tokens":3,"completion_tokens":2}});
            format!("data: {value}\n\ndata: [DONE]\n\n")
        }
    };
    ResponseTemplate::new(200).set_body_raw(raw, "text/event-stream")
}

fn assert_native_history(body: &Value, protocol: &str) {
    match protocol {
        "responses"=>assert!(body["input"].as_array().unwrap().iter().any(|item| item==&json!({"type":"reasoning","id":"context-reasoning","encrypted_content":"context-original-opaque","summary":[]}))),
        "claude"=>assert!(body["messages"].as_array().unwrap().iter().filter(|message|message["role"]=="assistant")
            .flat_map(|message|message["content"].as_array().unwrap()).any(|part|part==&json!({"type":"thinking","thinking":"context fixture reasoning","signature":"context-original-signature"}))),
        "gemini"=>assert!(body["contents"].as_array().unwrap().iter().filter(|message|message["role"]=="model")
            .flat_map(|message|message["parts"].as_array().unwrap()).any(|part|part==&json!({"functionCall":{"id":"context-call-0","name":"read_file","args":read_args()},"thoughtSignature":"context-original-signature"}))),
        _=>{},
    }
}

fn done(result: TurnResult, text: &str) {
    let TurnResult::Done(output) = result else {
        panic!("unexpected non-completed fixture turn");
    };
    assert_eq!(output.final_text(), text);
}

async fn stored(session: &Session) -> AgentState {
    let (_, state, suspension) = session
        .checkpointer
        .as_ref()
        .unwrap()
        .load_latest(&session.thread_id)
        .await
        .unwrap()
        .unwrap();
    assert!(suspension.is_none());
    state
}

async fn protocol_case(directory: &Path, protocol: &str, denied: bool, unknown: bool) {
    std::fs::create_dir(directory.join("src")).unwrap();
    std::fs::write(directory.join("src/value.txt"), "before").unwrap();
    let server = MockServer::start().await;
    let requests = Arc::new(AtomicUsize::new(0));
    let captured = Arc::new(Mutex::new(Vec::<Value>::new()));
    let count = requests.clone();
    let bodies = captured.clone();
    let selected = protocol.to_owned();
    let root = directory.to_path_buf();
    Mock::given(method("POST"))
        .and(path(match protocol {
            "chat" => "/v1/chat/completions",
            "responses" => "/v1/responses",
            "claude" => "/v1/messages",
            "gemini" => "/models/context-fixture:streamGenerateContent",
            _ => "/chat/completions",
        }))
        .respond_with(move |request: &wiremock::Request| {
            let index = count.fetch_add(1, Ordering::SeqCst);
            assert!(index < 3);
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            let data = context_payload(system(&body, &selected));
            assert_eq!(data["initial_request"]["text"], INITIAL);
            if index > 0 {
                assert_native_history(&body, &selected);
            }
            if index == 1 {
                assert_eq!(data["file_observations"][0]["selector"], "src/value.txt");
                assert_eq!(data["file_observations"][0]["freshness"], "historical");
                assert!(body.to_string().contains("before"));
            }
            if index == 2 {
                assert_eq!(
                    data["file_observations"][0]["freshness"],
                    if denied {
                        "historical"
                    } else if unknown {
                        "uncertain"
                    } else {
                        "stale"
                    }
                );
                assert_eq!(
                    std::fs::read_to_string(root.join("src/value.txt")).unwrap(),
                    if denied { "before" } else { "after" }
                );
            }
            bodies.lock().unwrap().push(body);
            response(
                &selected,
                index,
                match index {
                    0 => Some(("read_file", read_args())),
                    1 => Some((
                        if unknown {
                            "fixture_unknown_effect"
                        } else {
                            "edit_file"
                        },
                        if unknown { json!({}) } else { edit_args() },
                    )),
                    _ => None,
                },
                "actual context fixture complete",
            )
        })
        .expect(3)
        .mount(&server)
        .await;
    let cfg = if matches!(protocol, "chat" | "responses" | "claude" | "gemini") {
        named_config(&server, protocol)
    } else {
        flat_config(server.uri(), Some(enabled()))
    };
    let saved = save_config(&cfg);
    let mut extras = Vec::<Arc<dyn Tool>>::new();
    if unknown {
        extras.push(Arc::new(UnknownEffect(directory.join("src/value.txt"))));
    }
    let (mut session, approvals) =
        build_fixture_session(&cfg, directory, true, denied, extras).await;
    done(
        session.agent.run_turn(INITIAL).await.unwrap(),
        "actual context fixture complete",
    );
    assert_eq!(requests.load(Ordering::SeqCst), 3);
    assert_eq!(approvals.load(Ordering::SeqCst), usize::from(!unknown));
    assert_eq!(*session.usage.lock().unwrap(), (9, 6));
    let state = stored(&session).await;
    assert_eq!(state.custom[CONTEXT_KEY]["initial_input"]["text"], INITIAL);
    assert_eq!(
        state.custom["_craftsman_durable_turn_v1"]["in_progress"],
        false
    );
    let results: Vec<_> = state
        .messages
        .iter()
        .filter_map(|message| {
            if let Msg::Tool {
                content, is_error, ..
            } = message
            {
                Some((content.clone(), *is_error))
            } else {
                None
            }
        })
        .collect();
    assert_eq!(results.len(), 2);
    assert!(results[0].0.contains("before") && !results[0].1);
    assert_eq!(results[1].1, denied);
    if !denied && !unknown {
        assert!(results[1].0.contains("Updated file"));
    }
    let original = state.messages.clone();
    let thread = session.thread_id.clone();
    assert!(session.load_thread(&thread).await.unwrap());
    assert_eq!(session.agent.state.lock().unwrap().messages, original);
    assert_eq!(
        requests.load(Ordering::SeqCst),
        3,
        "reopen does not replay or sample"
    );
    let controls = session.controls.as_ref().unwrap();
    let report = controls
        .context
        .render(&session.agent.state.lock().unwrap(), None)
        .unwrap();
    assert!(report.contains("\"last_prepared_request\": null"));
    assert_eq!(
        requests.load(Ordering::SeqCst),
        3,
        "context inspection is local"
    );
    config_preserved(&saved);
    assert_eq!(captured.lock().unwrap().len(), 3);
    session.lsp.shutdown().await;
}

struct UnknownEffect(PathBuf);
#[async_trait]
impl Tool for UnknownEffect {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "fixture_unknown_effect".into(),
            description: "actual isolated file effect".into(),
            parameters: json!({"type":"object"}),
        }
    }
    async fn call(&self, _: Value, _: &mut ToolCtx<'_>) -> Result<ToolOutcome, AgentError> {
        std::fs::write(&self.0, "after").unwrap();
        Ok(ToolOutcome::ok(
            "actual unknown-operation file effect completed",
        ))
    }
}

async fn steering_case(directory: &Path) {
    let server = MockServer::start().await;
    let requests = Arc::new(AtomicUsize::new(0));
    let first = Arc::new(Notify::new());
    let release = Arc::new((Mutex::new(false), Condvar::new()));
    let count = requests.clone();
    let seen = first.clone();
    let gate = release.clone();
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(move |request: &wiremock::Request| {
            let index = count.fetch_add(1, Ordering::SeqCst);
            assert!(index < 2);
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            let data = context_payload(system(&body, "flat"));
            assert_eq!(data["initial_request"]["text"], INITIAL);
            if index == 0 {
                assert_eq!(data["recent_user_inputs"], json!([]));
                seen.notify_one();
                let (lock, wake) = &*gate;
                let (released, timeout) = wake
                    .wait_timeout_while(lock.lock().unwrap(), Duration::from_secs(15), |released| {
                        !*released
                    })
                    .unwrap();
                assert!(
                    *released && !timeout.timed_out(),
                    "actual first request readiness/release bound"
                );
            } else {
                let updates = data["recent_user_inputs"].as_array().unwrap();
                assert_eq!(updates.len(), 2);
                assert!(
                    updates
                        .iter()
                        .all(|update| update["text"] == "actual accepted steering")
                );
                assert_ne!(updates[0]["event"], updates[1]["event"]);
            }
            response(
                "flat",
                index,
                None,
                if index == 0 {
                    "first returned answer"
                } else {
                    "answer after actual steering"
                },
            )
        })
        .expect(2)
        .mount(&server)
        .await;
    let cfg = flat_config(server.uri(), Some(enabled()));
    let saved = save_config(&cfg);
    let (session, _) = build_fixture_session(&cfg, directory, true, false, vec![]).await;
    let ready = first.notified();
    tokio::pin!(ready);
    let agent = session.agent.clone();
    let task = tokio::spawn(async move { agent.run_turn(INITIAL).await });
    tokio::time::timeout(Duration::from_secs(15), &mut ready)
        .await
        .unwrap();
    assert!(session.agent.steer("actual accepted steering"));
    assert!(session.agent.steer("actual accepted steering"));
    let live = session.agent.state.lock().unwrap().clone();
    assert_eq!(
        live.custom[CONTEXT_KEY]["updates"],
        json!([]),
        "acceptance alone does not promote pending input"
    );
    let first_saved = stored(&session).await;
    assert_eq!(
        first_saved.custom[CONTEXT_KEY]["initial_input"]["text"],
        INITIAL
    );
    assert_eq!(first_saved.custom[CONTEXT_KEY]["updates"], json!([]));
    *release.0.lock().unwrap() = true;
    release.1.notify_one();
    done(
        tokio::time::timeout(Duration::from_secs(15), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
        "answer after actual steering",
    );
    let final_saved = stored(&session).await;
    assert_eq!(
        final_saved.custom[CONTEXT_KEY]["updates"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(final_saved.messages.iter().filter(|message|matches!(message,Msg::User { content } if content=="actual accepted steering")).count(),2);
    assert_eq!(
        final_saved.custom["_craftsman_durable_turn_v1"]["in_progress"],
        false
    );
    assert_eq!(requests.load(Ordering::SeqCst), 2);
    config_preserved(&saved);
    session.lsp.shutdown().await;
}

async fn lifecycle_case(directory: &Path) {
    std::fs::create_dir(directory.join("src")).unwrap();
    std::fs::write(directory.join("src/value.txt"), "before").unwrap();
    let server = MockServer::start().await;
    let requests = Arc::new(AtomicUsize::new(0));
    let count = requests.clone();
    Mock::given(method("POST")).and(path("/chat/completions")).respond_with(move |request:&wiremock::Request| {
        let index=count.fetch_add(1,Ordering::SeqCst); assert!(index<7);
        let body:Value=serde_json::from_slice(&request.body).unwrap();
        let summary=system(&body,"flat").starts_with("You are compacting an ongoing agent conversation");
        assert_eq!(summary,matches!(index,3|5));
        if !summary {
            let data=context_payload(system(&body,"flat"));
            assert_eq!(data["initial_request"]["text"],INITIAL);
            assert!(data["recent_user_inputs"].as_array().unwrap().iter().all(|input|input["text"]!="actual model-generated continuation summary"));
        }
        response("flat",index,match index {0=>Some(("read_file",read_args())),1=>Some(("edit_file",edit_args())),_=>None},
            match index {3|5=>"actual model-generated continuation summary",2=>"first lifecycle answer",4=>"answer after automatic compaction",_=>"answer after actual model transfer"})
    }).expect(7).mount(&server).await;
    let cfg = flat_config(server.uri(), Some(enabled()));
    let saved = save_config(&cfg);
    let (mut session, approvals) =
        build_fixture_session(&cfg, directory, true, false, vec![]).await;
    done(
        session.agent.run_turn(INITIAL).await.unwrap(),
        "first lifecycle answer",
    );
    let first = session.agent.state.lock().unwrap().clone();
    let original_tools: Vec<_> = first
        .messages
        .iter()
        .filter(|message| matches!(message, Msg::Tool { .. }))
        .cloned()
        .collect();
    assert_eq!(original_tools.len(), 2);
    {
        let mut state = session.agent.state.lock().unwrap();
        // Synthetic retained history triggers the real automatic pass; the
        // original read/edit group below came from actual HTTP/tool dispatch.
        let mut prefix: Vec<_> = (0..70)
            .flat_map(|index| {
                [
                    Msg::user(format!("older fixture turn {index}")),
                    Msg::assistant(format!("older fixture reply {index}")),
                ]
            })
            .collect();
        prefix.extend(state.messages.clone());
        state.messages = prefix;
    }
    done(
        session
            .agent
            .run_turn("Continue the lifecycle task.")
            .await
            .unwrap(),
        "answer after automatic compaction",
    );
    let automatic = stored(&session).await;
    assert_eq!(automatic.custom[CONTEXT_KEY]["compactions"], 1);
    for result in &original_tools {
        assert!(automatic.messages.contains(result));
    }
    assert_eq!(requests.load(Ordering::SeqCst), 5);
    let before = session.agent.state.lock().unwrap().messages.len();
    let counts = session
        .agent
        .compact_idle(&session.compaction)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(counts.0, before);
    assert!(counts.1 < counts.0);
    let manual = stored(&session).await;
    assert_eq!(manual.messages.len(), counts.1);
    assert_eq!(manual.custom[CONTEXT_KEY]["compactions"], 2);
    assert_eq!(manual.custom[CONTEXT_KEY]["initial_input"]["text"], INITIAL);
    assert!(
        manual.messages.contains(&original_tools[1]),
        "the original recent write result remains in the owned kept tail"
    );
    assert_eq!(requests.load(Ordering::SeqCst), 6);
    assert!(
        session
            .controls
            .as_ref()
            .unwrap()
            .context
            .render(&session.agent.state.lock().unwrap(), None)
            .unwrap()
            .contains("\"last_prepared_request\": null")
    );
    let thread = session.thread_id.clone();
    assert!(session.load_thread(&thread).await.unwrap());
    assert_eq!(
        session.agent.state.lock().unwrap().messages,
        manual.messages
    );
    assert_eq!(requests.load(Ordering::SeqCst), 6);
    session.lsp.shutdown().await;
    let mut replacement_cfg = cfg.clone();
    replacement_cfg.model = Some("replacement-model".into());
    let (mut replacement, _) =
        build_fixture_session(&replacement_cfg, directory, true, false, vec![]).await;
    replacement
        .inherit_conversation(&session, Some(&thread))
        .unwrap();
    assert_eq!(
        replacement.agent.state.lock().unwrap().messages,
        manual.messages
    );
    assert!(replacement.agent.state.lock().unwrap().last_usage.is_none());
    assert_eq!(
        requests.load(Ordering::SeqCst),
        6,
        "transfer invokes no model or tool"
    );
    assert_eq!(
        *replacement.usage.lock().unwrap(),
        *session.usage.lock().unwrap()
    );
    let report = replacement
        .controls
        .as_ref()
        .unwrap()
        .context
        .render(&replacement.agent.state.lock().unwrap(), None)
        .unwrap();
    assert!(report.contains("\"last_prepared_request\": null"));
    done(
        replacement
            .agent
            .run_turn("Continue using the selected new model.")
            .await
            .unwrap(),
        "answer after actual model transfer",
    );
    assert_eq!(requests.load(Ordering::SeqCst), 7);
    assert_eq!(
        approvals.load(Ordering::SeqCst),
        1,
        "actual file edit is not replayed"
    );
    assert_eq!(
        std::fs::read_to_string(directory.join("src/value.txt")).unwrap(),
        "after"
    );
    let final_saved = stored(&replacement).await;
    assert_eq!(
        final_saved.custom[CONTEXT_KEY]["initial_input"]["text"],
        INITIAL
    );
    assert_eq!(
        final_saved.custom[CONTEXT_KEY]["updates"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(*replacement.usage.lock().unwrap(), (15, 10));
    config_preserved(&saved);
    replacement.lsp.shutdown().await;
}

async fn steering_batch_case(directory: &Path, byte_limit: bool) {
    let server = MockServer::start().await;
    let requests = Arc::new(AtomicUsize::new(0));
    let first = Arc::new(Notify::new());
    let release = Arc::new((Mutex::new(false), Condvar::new()));
    let count = requests.clone();
    let seen = first.clone();
    let gate = release.clone();
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(move |request: &wiremock::Request| {
            assert_eq!(count.fetch_add(1, Ordering::SeqCst), 0);
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            assert_eq!(
                context_payload(system(&body, "flat"))["initial_request"]["text"],
                INITIAL
            );
            seen.notify_one();
            let (lock, wake) = &*gate;
            let (released, timeout) = wake
                .wait_timeout_while(lock.lock().unwrap(), Duration::from_secs(15), |released| {
                    !*released
                })
                .unwrap();
            assert!(
                *released && !timeout.timed_out(),
                "actual batch readiness/release bound"
            );
            response(
                "flat",
                0,
                None,
                "actual response before selected batch refusal",
            )
        })
        .expect(1)
        .mount(&server)
        .await;
    let cfg = flat_config(server.uri(), Some(enabled()));
    let saved = save_config(&cfg);
    let (session, _) = build_fixture_session(&cfg, directory, true, false, vec![]).await;
    let ready = first.notified();
    tokio::pin!(ready);
    let agent = session.agent.clone();
    let task = tokio::spawn(async move { agent.run_turn(INITIAL).await });
    tokio::time::timeout(Duration::from_secs(15), &mut ready)
        .await
        .unwrap();
    let first_saved = stored(&session).await;
    let first_revision = first_saved.custom[CONTEXT_KEY]["last_input"]["revision"]
        .as_u64()
        .unwrap();
    let inputs: Vec<_> = (0..if byte_limit { 9 } else { 40 })
        .map(|index| {
            let mut input = format!("actual accepted batch input {index}: ");
            if byte_limit {
                input.push_str(
                    &"x".repeat(crate::context_compiler::MAX_OBSERVED_INPUT_BYTES - input.len()),
                );
            }
            input
        })
        .collect();
    for input in &inputs {
        assert!(session.agent.steer(input.clone()));
    }
    assert_eq!(
        session.agent.state.lock().unwrap().custom[CONTEXT_KEY]["last_input"]["revision"],
        first_revision
    );
    *release.0.lock().unwrap() = true;
    release.1.notify_one();
    let error = tokio::time::timeout(Duration::from_secs(15), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err()
        .to_string();
    assert!(error.contains("Selected input observation exceeds its batch work limit."));
    let actual = session.agent.state.lock().unwrap().clone();
    let checkpoint = stored(&session).await;
    for state in [&actual, &checkpoint] {
        let users: Vec<_> = state
            .messages
            .iter()
            .filter_map(|message| {
                if let Msg::User { content } = message {
                    Some(content.as_str())
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(users.len(), inputs.len() + 1);
        assert_eq!(users[0], INITIAL);
        for (index, (actual, expected)) in users[1..].iter().zip(inputs.iter()).enumerate() {
            assert!(
                *actual == expected.as_str(),
                "accepted source mismatch at index {index}"
            );
        }
        let last_revision = state.custom[CONTEXT_KEY]["last_input"]["revision"]
            .as_u64()
            .unwrap();
        assert!(last_revision >= first_revision);
        assert!(
            last_revision - first_revision <= if byte_limit { 8 } else { 32 },
            "selected source observer work is bounded"
        );
        assert!(state.messages.iter().any(|message| matches!(message, Msg::Assistant { content, .. } if content == "actual response before selected batch refusal")));
    }
    assert_eq!(
        requests.load(Ordering::SeqCst),
        1,
        "batch refusal performs no later model request"
    );
    config_preserved(&saved);
    session.lsp.shutdown().await;
}

async fn malformed_rebind_case(directory: &Path) {
    let server = MockServer::start().await;
    let requests = Arc::new(AtomicUsize::new(0));
    let count = requests.clone();
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(move |_: &wiremock::Request| {
            assert_eq!(count.fetch_add(1, Ordering::SeqCst), 0);
            response("flat", 0, None, "actual answer before malformed rebind")
        })
        .expect(1)
        .mount(&server)
        .await;
    let cfg = flat_config(server.uri(), Some(enabled()));
    let saved = save_config(&cfg);
    let (mut session, _) = build_fixture_session(&cfg, directory, true, false, vec![]).await;
    session
        .agent
        .state
        .lock()
        .unwrap()
        .custom
        .insert("retained_context_fixture".into(), json!({"original":true}));
    done(
        session.agent.run_turn(INITIAL).await.unwrap(),
        "actual answer before malformed rebind",
    );
    let cp = session.checkpointer.as_ref().unwrap();
    let (sequence, mut bad, suspension) =
        cp.load_latest(&session.thread_id).await.unwrap().unwrap();
    assert!(suspension.is_none());
    let original = bad.messages.clone();
    bad.custom[CONTEXT_KEY]["version"] = json!(99);
    cp.save(
        &session.thread_id,
        sequence.checked_add(1).unwrap(),
        &bad,
        None,
    )
    .await
    .unwrap();
    let thread = session.thread_id.clone();
    let error = session.load_thread(&thread).await.unwrap_err().to_string();
    assert!(error.contains("Context compilation state is unavailable"));
    let restored = session.agent.state.lock().unwrap().clone();
    assert_eq!(restored.messages, original);
    assert_eq!(
        restored.custom["retained_context_fixture"],
        json!({"original":true})
    );
    assert_eq!(restored.custom[CONTEXT_KEY]["version"], 99);
    let input = "actual follow-up retained after malformed reopen";
    let error = session.agent.run_turn(input).await.unwrap_err().to_string();
    assert!(error.contains("further sampling was stopped"));
    let final_saved = stored(&session).await;
    assert!(
        final_saved
            .messages
            .iter()
            .any(|message| matches!(message, Msg::User { content } if content == input))
    );
    assert!(final_saved.messages.iter().any(|message| matches!(message, Msg::Assistant { content, .. } if content == "actual answer before malformed rebind")));
    assert_eq!(
        final_saved.custom["retained_context_fixture"],
        json!({"original":true})
    );
    assert_eq!(
        requests.load(Ordering::SeqCst),
        1,
        "malformed rebind never samples or replays"
    );
    config_preserved(&saved);
    session.lsp.shutdown().await;
}

async fn cancelled_steering_case(directory: &Path) {
    std::fs::create_dir(directory.join("src")).unwrap();
    std::fs::write(directory.join("src/value.txt"), "before").unwrap();
    let server = MockServer::start().await;
    let requests = Arc::new(AtomicUsize::new(0));
    let second = Arc::new(Notify::new());
    let release = Arc::new((Mutex::new(false), Condvar::new()));
    let count = requests.clone();
    let seen = second.clone();
    let gate = release.clone();
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(move |request: &wiremock::Request| {
            let index = count.fetch_add(1, Ordering::SeqCst);
            assert!(index < 2);
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            assert_eq!(
                context_payload(system(&body, "responses"))["initial_request"]["text"],
                INITIAL
            );
            if index == 1 {
                assert_native_history(&body, "responses");
                assert!(body.to_string().contains("before"));
                seen.notify_one();
                let (lock, wake) = &*gate;
                let (released, timeout) = wake
                    .wait_timeout_while(lock.lock().unwrap(), Duration::from_secs(15), |released| {
                        !*released
                    })
                    .unwrap();
                assert!(
                    *released && !timeout.timed_out(),
                    "actual cancelled response readiness/release bound"
                );
            }
            response(
                "responses",
                index,
                Some((
                    if index == 0 { "read_file" } else { "edit_file" },
                    if index == 0 { read_args() } else { edit_args() },
                )),
                "unused cancelled answer",
            )
        })
        .expect(2)
        .mount(&server)
        .await;
    let cfg = named_config(&server, "responses");
    let saved = save_config(&cfg);
    let (mut session, approvals) =
        build_fixture_session(&cfg, directory, true, false, vec![]).await;
    let ready = second.notified();
    tokio::pin!(ready);
    let agent = session.agent.clone();
    let task = tokio::spawn(async move { agent.run_turn(INITIAL).await });
    tokio::time::timeout(Duration::from_secs(15), &mut ready)
        .await
        .unwrap();
    let steering = "Retain the actual read and keep this file unchanged.";
    assert!(session.agent.steer(steering));
    assert!(session.agent.request_cancel());
    *release.0.lock().unwrap() = true;
    release.1.notify_one();
    done(
        tokio::time::timeout(Duration::from_secs(15), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
        "Stopped by the user.",
    );
    let state = stored(&session).await;
    assert_eq!(requests.load(Ordering::SeqCst), 2);
    assert_eq!(
        approvals.load(Ordering::SeqCst),
        0,
        "cancelled edit was never dispatched"
    );
    assert_eq!(
        std::fs::read_to_string(directory.join("src/value.txt")).unwrap(),
        "before"
    );
    assert_eq!(*session.usage.lock().unwrap(), (6, 4));
    assert_eq!(state.custom[CONTEXT_KEY]["updates"][0]["text"], steering);
    assert_eq!(
        state.custom["_craftsman_durable_turn_v1"]["in_progress"],
        false
    );
    assert!(
        state.custom["_craftsman_native_model_history"]
            .to_string()
            .contains("context-original-opaque")
    );
    assert!(state.messages.iter().any(|message| matches!(message, Msg::Tool { content, is_error, .. } if content.contains("before") && !*is_error)));
    for (index, message) in state.messages.iter().enumerate() {
        if let Msg::Assistant { tool_calls, .. } = message {
            for (offset, call) in tool_calls.iter().enumerate() {
                assert!(
                    matches!(state.messages.get(index + offset + 1), Some(Msg::Tool { tool_call_id, .. }) if tool_call_id == &call.id),
                    "actual/cancelled result group is balanced before steering"
                );
            }
        }
    }
    assert!(state.messages.iter().any(|message| matches!(message, Msg::Tool { tool_call_id, content, is_error } if tool_call_id == "context-call-1" && content.contains("cancelled") && *is_error)));
    let steering_index = state
        .messages
        .iter()
        .position(|message| matches!(message, Msg::User { content } if content == steering))
        .unwrap();
    let cancelled_index = state.messages.iter().position(|message| matches!(message, Msg::Tool { tool_call_id, .. } if tool_call_id == "context-call-1")).unwrap();
    assert!(steering_index > cancelled_index);
    let thread = session.thread_id.clone();
    assert!(session.load_thread(&thread).await.unwrap());
    assert_eq!(session.agent.state.lock().unwrap().messages, state.messages);
    assert!(
        session.agent.recovery_info().is_none(),
        "a completed balanced checkpoint has no interrupted-turn recovery notice"
    );
    assert_eq!(
        requests.load(Ordering::SeqCst),
        2,
        "cancel/reopen performs no model or tool replay"
    );
    config_preserved(&saved);
    session.lsp.shutdown().await;
}

async fn nondurable_error_case(directory: &Path) {
    let server = MockServer::start().await;
    let requests = Arc::new(AtomicUsize::new(0));
    let count = requests.clone();
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(move |_: &wiremock::Request| {
            assert_eq!(count.fetch_add(1, Ordering::SeqCst), 0);
            response("flat", 0, None, "actual non-durable completed source")
        })
        .expect(1)
        .mount(&server)
        .await;
    let cfg = flat_config(server.uri(), Some(enabled()));
    let saved = save_config(&cfg);
    let session = build_session_ext(
        &cfg,
        Arc::new(Mutex::new(ApprovalState::default())),
        "",
        false,
        false,
        None,
        SessionExtras {
            context_compilation: true,
            workspace_root: Some(directory.to_path_buf()),
            token_sink: Some(Arc::new(|_| {})),
            activity_sink: Some(Arc::new(|_| {})),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(!session.agent.durable_turn_checkpoints_enabled());
    done(
        session.agent.run_turn(INITIAL).await.unwrap(),
        "actual non-durable completed source",
    );
    let original = stored(&session).await;
    let stamp = original.custom[CONTEXT_KEY]["last_input"].clone();
    session.agent.state.lock().unwrap().custom[CONTEXT_KEY]["version"] = json!(99);
    let input = "This original input must survive the selected ACP error path.";
    assert!(
        session
            .agent
            .run_turn(input)
            .await
            .unwrap_err()
            .to_string()
            .contains("Context compilation state is unavailable")
    );
    rollback_uncontrolled_user(&session.agent, session.controls.as_deref());
    let current = session.agent.state.lock().unwrap().clone();
    let checkpoint = stored(&session).await;
    assert_eq!(current.messages, checkpoint.messages);
    assert!(
        current
            .messages
            .iter()
            .any(|message| matches!(message, Msg::User { content } if content == input))
    );
    assert_eq!(current.custom[CONTEXT_KEY]["last_input"], stamp);
    assert_eq!(checkpoint.custom[CONTEXT_KEY]["last_input"], stamp);
    assert_eq!(
        requests.load(Ordering::SeqCst),
        1,
        "selected error path performs no additional sampling"
    );
    config_preserved(&saved);
    session.lsp.shutdown().await;
}

async fn selector_case(directory: &Path, mode: &str) {
    let server = MockServer::start().await;
    let context = match mode {
        "missing" => None,
        "disabled" => Some(json!({"enabled":false,"max_bytes":12288})),
        "invalid" => Some(json!({"enabled":"private-invalid-setting"})),
        _ => Some(enabled()),
    };
    let cfg = flat_config(server.uri(), context);
    let saved = save_config(&cfg);
    if mode == "invalid" {
        let notices = Arc::new(Mutex::new(Vec::<String>::new()));
        let observed = notices.clone();
        let built = build_session_ext(
            &cfg,
            Arc::new(Mutex::new(ApprovalState::default())),
            "",
            false,
            false,
            None,
            SessionExtras {
                context_compilation: true,
                workspace_root: Some(directory.to_path_buf()),
                activity_sink: Some(Arc::new(move |notice| {
                    observed.lock().unwrap().push(notice.into())
                })),
                ..Default::default()
            },
        )
        .await;
        assert!(built.is_none());
        assert!(
            !PathBuf::from(std::env::var_os("CRAFTSMAN_CONFIG_DIR").unwrap())
                .join("threads.db")
                .exists()
        );
        assert!(
            notices
                .lock()
                .unwrap()
                .iter()
                .any(|notice| notice.contains("Invalid context settings"))
        );
        assert!(
            notices
                .lock()
                .unwrap()
                .iter()
                .all(|notice| !notice.contains("private-invalid-setting"))
        );
        assert!(server.received_requests().await.unwrap().is_empty());
    } else {
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(|request: &wiremock::Request| {
                let body: Value = serde_json::from_slice(&request.body).unwrap();
                assert!(!system(&body, "flat").contains("Task context from earlier user inputs"));
                response("flat", 0, None, "default unchanged")
            })
            .expect(1)
            .mount(&server)
            .await;
        let (session, _) =
            build_fixture_session(&cfg, directory, mode != "legacy", false, vec![]).await;
        assert!(session.controls.is_none());
        done(
            session.agent.run_turn(INITIAL).await.unwrap(),
            "default unchanged",
        );
        assert!(!stored(&session).await.custom.contains_key(CONTEXT_KEY));
        session.lsp.shutdown().await;
    }
    config_preserved(&saved);
}

async fn refusal_case(directory: &Path, mode: &str) {
    let server = MockServer::start().await;
    let cfg = flat_config(server.uri(), Some(enabled()));
    let saved = save_config(&cfg);
    let requests = Arc::new(AtomicUsize::new(0));
    let count = requests.clone();
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(move |_: &wiremock::Request| {
            let index = count.fetch_add(1, Ordering::SeqCst);
            assert_eq!(index, 0);
            response("flat", 0, None, "original completed turn")
        })
        .expect(usize::from(mode == "overflow") as u64)
        .mount(&server)
        .await;
    let (session, _) = build_fixture_session(&cfg, directory, true, false, vec![]).await;
    if mode == "overflow" {
        done(
            session
                .agent
                .run_turn("original saved context input")
                .await
                .unwrap(),
            "original completed turn",
        );
        session.agent.state.lock().unwrap().custom[CONTEXT_KEY]["sequence"] = json!(u64::MAX);
    } else if mode == "collision" {
        session
            .agent
            .state
            .lock()
            .unwrap()
            .custom
            .insert(CONTEXT_KEY.into(), json!("private-context-collision"));
    }
    let input = if mode == "oversized" {
        "x".repeat(crate::context_compiler::MAX_OBSERVED_INPUT_BYTES + 1)
    } else {
        "actual input retained after local refusal".into()
    };
    let error = session
        .agent
        .run_turn(input.clone())
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("further sampling was stopped"));
    assert!(!error.contains("private-context-collision"));
    let state = stored(&session).await;
    assert!(
        state
            .messages
            .iter()
            .any(|message| matches!(message,Msg::User { content } if content==&input))
    );
    if mode == "oversized" {
        assert!(!state.custom.contains_key(CONTEXT_KEY));
    }
    assert_eq!(
        requests.load(Ordering::SeqCst),
        usize::from(mode == "overflow")
    );
    config_preserved(&saved);
    session.lsp.shutdown().await;
}

async fn delegated_case(directory: &Path) {
    let server = MockServer::start().await;
    let requests = Arc::new(AtomicUsize::new(0));
    let count = requests.clone();
    Mock::given(method("POST")).and(path("/chat/completions")).respond_with(move |request:&wiremock::Request| {
        let index=count.fetch_add(1,Ordering::SeqCst); assert!(index<3);
        let body:Value=serde_json::from_slice(&request.body).unwrap();
        if index==1 {
            assert!(!system(&body,"flat").contains("Task context from earlier user inputs"));
            assert!(body["tools"].as_array().unwrap().iter().all(|tool|tool["function"]["name"]!="task"));
        } else { assert_eq!(context_payload(system(&body,"flat"))["initial_request"]["text"],INITIAL); }
        response("flat",index,if index==0 {Some(("task",json!({"description":"Return one fixture answer.","subagent_type":"general-purpose","run_in_background":false})))}else{None},
            if index==1{"actual delegated answer"}else{"actual parent answer"})
    }).expect(3).mount(&server).await;
    let cfg = flat_config(server.uri(), Some(enabled()));
    let saved = save_config(&cfg);
    let (session, _) = build_fixture_session(&cfg, directory, true, false, vec![]).await;
    done(
        session.agent.run_turn(INITIAL).await.unwrap(),
        "actual parent answer",
    );
    assert_eq!(requests.load(Ordering::SeqCst), 3);
    assert_eq!(*session.usage.lock().unwrap(), (6, 4));
    let state = stored(&session).await;
    assert_eq!(
        state.custom[CONTEXT_KEY]["updates"],
        json!([]),
        "delegated/runtime input is not human task authority"
    );
    config_preserved(&saved);
    session.lsp.shutdown().await;
}

async fn private_case(directory: &Path) {
    use crate::exec::construction::{Options, Prepared, SessionSelection};
    let config = directory.join("private-config");
    let workspace = directory.join("private-workspace");
    std::fs::create_dir(&config).unwrap();
    std::fs::create_dir(&workspace).unwrap();
    std::fs::write(config.join("config.json"),json!({"model":"synthetic","api_key":"synthetic-secret",
        "base_url":"http://127.0.0.1:9","context":enabled(),"voice":{"enabled":false},"gadgets":{"enabled":false}}).to_string()).unwrap();
    let prepared = Prepared::prepare_with_environment_for_tests(
        Options {
            workspace: workspace.clone(),
            config_dir: config.clone(),
            session: SessionSelection::Ephemeral,
            trust_workspace: false,
            selectors: Default::default(),
        },
        BTreeMap::new(),
    )
    .await
    .unwrap();
    let (original, gate, meter) = build_prepared_session_with_request_limit(
        &prepared,
        crate::exec::meter::RequestLimit::default(),
    )
    .await
    .unwrap();
    assert!(original.controls.is_none());
    let refused = build_session_with_profile(
        SessionConstruction::Headless {
            prepared: &prepared,
            gate: &gate,
        },
        Arc::new(Mutex::new(ApprovalState::default())),
        "",
        true,
        false,
        None,
        SessionExtras {
            context_compilation: true,
            token_sink: Some(Arc::new(|_| {})),
            activity_sink: Some(Arc::new(|_| {})),
            ..Default::default()
        },
        Some(meter),
    )
    .await;
    assert!(refused.is_none());
    assert!(!config.join("threads.db").exists());
    original.lsp.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "context-control native fixture; unknown/unmarked startup returns before config access"]
async fn native_context_control_fixture() {
    let selected: Vec<_> = std::env::args_os()
        .filter_map(|arg| {
            arg.to_str()
                .and_then(|arg| arg.strip_prefix(MARKER))
                .map(str::to_owned)
        })
        .collect();
    if selected.len() != 1
        || !matches!(
            selected[0].as_str(),
            "chat"
                | "responses"
                | "claude"
                | "gemini"
                | "denied"
                | "unknown"
                | "steering"
                | "batch-count"
                | "batch-bytes"
                | "rebind-malformed"
                | "cancel-steering"
                | "nondurable-error"
                | "lifecycle"
                | "missing"
                | "disabled"
                | "legacy"
                | "invalid"
                | "collision"
                | "overflow"
                | "oversized"
                | "delegated"
                | "private"
        )
    {
        return;
    }
    let directory = std::env::current_dir().unwrap();
    match selected[0].as_str() {
        protocol @ ("chat" | "responses" | "claude" | "gemini") => {
            protocol_case(&directory, protocol, false, false).await
        }
        "denied" => protocol_case(&directory, "flat", true, false).await,
        "unknown" => protocol_case(&directory, "flat", false, true).await,
        "steering" => steering_case(&directory).await,
        "batch-count" => steering_batch_case(&directory, false).await,
        "batch-bytes" => steering_batch_case(&directory, true).await,
        "rebind-malformed" => malformed_rebind_case(&directory).await,
        "cancel-steering" => cancelled_steering_case(&directory).await,
        "nondurable-error" => nondurable_error_case(&directory).await,
        "lifecycle" => lifecycle_case(&directory).await,
        mode @ ("missing" | "disabled" | "legacy" | "invalid") => {
            selector_case(&directory, mode).await
        }
        mode @ ("collision" | "overflow" | "oversized") => refusal_case(&directory, mode).await,
        "delegated" => delegated_case(&directory).await,
        _ => private_case(&directory).await,
    }
    eprintln!("{DONE}");
}

async fn run_case(mode: &str) {
    let directory = tempfile::tempdir().unwrap();
    let workspace = directory.path().join("workspace");
    let config = directory.path().join("config");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::create_dir(&config).unwrap();
    let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
    command
        .args(["--ignored", "--exact", HELPER, "--nocapture"])
        .arg(format!("{MARKER}{mode}"))
        .current_dir(workspace)
        .env_clear()
        .env("CRAFTSMAN_CONFIG_DIR", config)
        .env("HOME", directory.path())
        .env("USERPROFILE", directory.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(path) = std::env::var_os("PATH") {
        command.env("PATH", path);
    }
    #[cfg(windows)]
    if let Some(root) = std::env::var_os("SystemRoot") {
        command.env("SystemRoot", root);
    }
    let mut child = command.spawn().unwrap();
    let mut stdout = child.stdout.take().unwrap().take(65537);
    let mut stderr = child.stderr.take().unwrap().take(65537);
    let result = tokio::time::timeout(Duration::from_secs(180), async {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let (status, _, _) = tokio::try_join!(
            child.wait(),
            stdout.read_to_end(&mut out),
            stderr.read_to_end(&mut err)
        )?;
        Ok::<_, std::io::Error>((status, out, err))
    })
    .await;
    let (status, out, err) = match result {
        Ok(Ok(result)) => result,
        _ => {
            let _ = tokio::time::timeout(Duration::from_secs(5), child.kill()).await;
            panic!("actual context-control fixture exceeded its original named-transport bound");
        }
    };
    assert!(out.len() <= 65536 && err.len() <= 65536);
    let error = std::str::from_utf8(&err).unwrap();
    assert!(status.success(), "{error}");
    assert_eq!(error.lines().filter(|line| *line == DONE).count(), 1);
}

#[tokio::test]
async fn actual_named_chat_context_preserves_real_file_results_and_checkpoints() {
    run_case("chat").await;
}
#[tokio::test]
async fn actual_responses_context_retains_original_opaque_history() {
    run_case("responses").await;
}
#[tokio::test]
async fn actual_claude_context_retains_original_signed_history() {
    run_case("claude").await;
}
#[tokio::test]
async fn actual_gemini_context_retains_original_signed_function_parts() {
    run_case("gemini").await;
}
#[tokio::test]
async fn actual_denied_edit_keeps_file_and_historical_observation() {
    run_case("denied").await;
}
#[tokio::test]
async fn actual_unknown_file_effect_invalidates_read_without_inventing_scope() {
    run_case("unknown").await;
}
#[tokio::test]
async fn actual_committed_steering_is_pinned_and_saved_without_promoting_pending_input() {
    run_case("steering").await;
}
#[tokio::test]
async fn actual_selected_steering_event_batch_limit_retains_all_inputs_without_more_sampling() {
    run_case("batch-count").await;
}
#[tokio::test]
async fn actual_selected_steering_byte_batch_limit_retains_all_inputs_without_more_sampling() {
    run_case("batch-bytes").await;
}
#[tokio::test]
async fn actual_malformed_context_reopen_preserves_data_and_refuses_later_sampling() {
    run_case("rebind-malformed").await;
}
#[tokio::test]
async fn actual_selected_cancel_retains_steering_after_balanced_native_tool_results() {
    run_case("cancel-steering").await;
}
#[tokio::test]
async fn actual_selected_nondurable_error_rollback_keeps_live_and_sqlite_input() {
    run_case("nondurable-error").await;
}
#[tokio::test]
async fn actual_compaction_reopen_and_model_transfer_retain_pins_without_replay() {
    run_case("lifecycle").await;
}
#[tokio::test]
async fn actual_missing_context_settings_keep_default_model_requests() {
    run_case("missing").await;
}
#[tokio::test]
async fn actual_false_context_setting_keeps_default_model_requests() {
    run_case("disabled").await;
}
#[tokio::test]
async fn actual_legacy_selector_ignores_context_configuration() {
    run_case("legacy").await;
}
#[tokio::test]
async fn actual_malformed_context_settings_refuse_before_store_or_provider_effects() {
    run_case("invalid").await;
}
#[tokio::test]
async fn actual_context_collision_retains_original_input_and_refuses_sampling() {
    run_case("collision").await;
}
#[tokio::test]
async fn actual_context_overflow_retains_original_input_and_refuses_sampling() {
    run_case("overflow").await;
}
#[tokio::test]
async fn actual_oversized_context_observation_preserves_original_input_in_sqlite() {
    run_case("oversized").await;
}
#[tokio::test]
async fn actual_delegated_agent_does_not_inherit_main_context_selector() {
    run_case("delegated").await;
}
#[tokio::test]
async fn actual_private_prepared_builder_refuses_context_selector() {
    run_case("private").await;
}
