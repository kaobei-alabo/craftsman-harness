//! Synthetic normal named transports retain real native history around scheduled checks.
use super::*;
use std::{
    process::Stdio,
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};
use tokio::io::AsyncReadExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const HELPER: &str = "agent::verification_protocol_tests::native_verification_protocol_fixture";
const MARKER: &str = "craftsman-verification-protocol=";
const DONE: &str = "actual named verification checks, native history and results completed";

fn response(protocol: &str, index: usize) -> ResponseTemplate {
    let tool = matches!(index, 0 | 2);
    let args = json!({"file_path":"src/value.txt","old_string":if index==0{"broken"}else{"still broken"},"new_string":if index==0{"still broken"}else{"fixed"}});
    let text = if index == 4 {
        "verified final answer"
    } else {
        "premature answer"
    };
    let raw = match protocol {
        "responses" => {
            let mut output = Vec::new();
            if index == 0 {
                output.push(json!({"type":"reasoning","id":"verification-reasoning","encrypted_content":"original-verification-opaque","summary":[]}));
            }
            output.push(if tool{json!({"type":"function_call","id":format!("function-{index}"),"call_id":format!("call-{index}"),"name":"edit_file","arguments":args.to_string(),"status":"completed"})}
                else{json!({"type":"message","id":format!("message-{index}"),"role":"assistant","status":"completed","content":[{"type":"output_text","text":text,"annotations":[]} ]})});
            let value = json!({"type":"response.completed","response":{"id":format!("response-{index}"),"status":"completed","output":output,"usage":{"input_tokens":3,"output_tokens":2}}});
            format!("event: response.completed\ndata: {value}\n\n")
        }
        "claude" => {
            let mut events =
                vec![json!({"type":"message_start","message":{"usage":{"input_tokens":3}}})];
            if index == 0 {
                events.push(json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}));
                events.push(json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"fixture reasoning"}}));
                events.push(json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"original-verification-signature"}}));
                events.push(json!({"type":"content_block_stop","index":0}));
            }
            let block = usize::from(index == 0);
            events.push(json!({"type":"content_block_start","index":block,"content_block":if tool{json!({"type":"tool_use","id":format!("call-{index}"),"name":"edit_file","input":{}})}else{json!({"type":"text","text":""})}}));
            events.push(json!({"type":"content_block_delta","index":block,"delta":if tool{json!({"type":"input_json_delta","partial_json":args.to_string()})}else{json!({"type":"text_delta","text":text})}}));
            events.push(json!({"type":"content_block_stop","index":block}));
            events.push(json!({"type":"message_delta","delta":{"stop_reason":if tool{"tool_use"}else{"end_turn"}},"usage":{"output_tokens":2}}));
            events.push(json!({"type":"message_stop"}));
            events
                .iter()
                .map(|event| format!("data: {event}\n\n"))
                .collect()
        }
        _ => {
            let mut part = if tool {
                json!({"functionCall":{"id":format!("call-{index}"),"name":"edit_file","args":args}})
            } else {
                json!({"text":text})
            };
            if index == 0 {
                part["thoughtSignature"] = json!("original-verification-signature");
            }
            let event = json!({"candidates":[{"content":{"parts":[part]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":3,"candidatesTokenCount":2}});
            format!("data: {event}\n\n")
        }
    };
    ResponseTemplate::new(200).set_body_raw(raw, "text/event-stream")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "named verification fixture; marker required before configuration access"]
async fn native_verification_protocol_fixture() {
    let modes: Vec<_> = std::env::args_os()
        .filter_map(|arg| {
            arg.to_str()
                .and_then(|arg| arg.strip_prefix(MARKER))
                .map(str::to_owned)
        })
        .collect();
    if modes.len() != 1 || !matches!(modes[0].as_str(), "responses" | "claude" | "gemini") {
        return;
    }
    let protocol = modes[0].clone();
    let directory = std::env::current_dir().unwrap();
    std::fs::create_dir(directory.join("src")).unwrap();
    std::fs::write(directory.join("src/value.txt"), "broken").unwrap();
    let server = MockServer::start().await;
    let requests = Arc::new(AtomicUsize::new(0));
    let observed = requests.clone();
    let selected = protocol.clone();
    Mock::given(method("POST"))
        .and(path(match protocol.as_str() {
            "responses" => "/v1/responses",
            "claude" => "/v1/messages",
            _ => "/models/verification-fixture:streamGenerateContent",
        }))
        .respond_with(move |request: &wiremock::Request| {
            let index = observed.fetch_add(1, Ordering::SeqCst);
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            let serialized = serde_json::to_string(&body).unwrap();
            if index > 0 {
                assert!(serialized.contains(if selected == "responses" {
                    "original-verification-opaque"
                } else {
                    "original-verification-signature"
                }));
            }
            if index == 2 {
                assert!(
                    serialized.contains("actual check failed"),
                    "actual verification results before native repair: {serialized}"
                );
            }
            if index == 4 {
                assert!(serialized.contains("Verification passed."));
            }
            response(&selected, index)
        })
        .expect(5)
        .mount(&server)
        .await;
    let protocol_name = match protocol.as_str() {
        "responses" => "openai_responses",
        "claude" => "anthropic_messages",
        _ => "gemini_generate_content",
    };
    let base = if protocol == "gemini" {
        server.uri()
    } else {
        format!("{}/v1", server.uri())
    };
    let cfg:Config=serde_json::from_value(json!({"model":"test/verification-fixture","providers":{"test":{"protocol":protocol_name,"base_url":base,"api_key":"fixture-key","models":{"verification-fixture":{"settings":{"reasoning":null}}}}},"harness_profile":"general","voice":{"enabled":false},"gadgets":{"enabled":false},"mcp_servers":[],"lsp_servers":{},"verification":{"commands":[super::verification_tests::command("check")],"paths":["src"],"protected_paths":[]}})).unwrap();
    let session = build_session_ext(
        &cfg,
        Arc::new(Mutex::new(ApprovalState::default())),
        "",
        false,
        false,
        None,
        SessionExtras {
            verification: true,
            workspace_root: Some(directory.clone()),
            cancellation_session: Some(craftsman_harness::SessionCancellation::default()),
            token_sink: Some(Arc::new(|_| {})),
            activity_sink: Some(Arc::new(|_| {})),
            approval_prompt: Some(Arc::new(|_, _, _| ApprovalPromptResult::Approve)),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let craftsman_harness::TurnResult::Done(output) = session
        .agent
        .run_turn("Complete the fixture with declared checks")
        .await
        .unwrap()
    else {
        panic!("suspended")
    };
    assert_eq!(output.final_text(), "verified final answer");
    assert_eq!(requests.load(Ordering::SeqCst), 5);
    assert_eq!(*session.usage.lock().unwrap(), (15, 10));
    assert_eq!(
        std::fs::read_to_string(directory.join("src/value.txt")).unwrap(),
        "fixed"
    );
    let state = session.agent.state.lock().unwrap().clone();
    assert_eq!(state.messages.iter().filter(|message|matches!(message,craftsman_harness::Msg::Tool{tool_call_id,..}if tool_call_id.starts_with("_craftsman_completion_"))).count(),2);
    session.agent.load_thread(&session.thread_id).await.unwrap();
    assert_eq!(session.agent.state.lock().unwrap().messages, state.messages);
    assert_eq!(requests.load(Ordering::SeqCst), 5);
    session.lsp.shutdown().await;
    eprintln!("{DONE}");
}

async fn run_case(protocol: &str) {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("workspace");
    let config = temp.path().join("config");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::create_dir(&config).unwrap();
    let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
    command
        .args(["--ignored", "--exact", HELPER, "--nocapture"])
        .arg(format!("{MARKER}{protocol}"))
        .current_dir(&workspace)
        .env_clear()
        .env("HOME", temp.path())
        .env("USERPROFILE", temp.path())
        .env("CRAFTSMAN_CONFIG_DIR", config)
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
        Ok(Ok(value)) => value,
        _ => {
            let _ = tokio::time::timeout(Duration::from_secs(5), child.kill()).await;
            panic!("named verification fixture exceeded its bound");
        }
    };
    assert!(out.len() <= 65536 && err.len() <= 65536);
    let error = std::str::from_utf8(&err).unwrap();
    assert!(status.success(), "{error}");
    assert_eq!(error.lines().filter(|line| *line == DONE).count(), 1);
}

#[tokio::test]
async fn actual_responses_native_history_survives_scheduled_verification() {
    run_case("responses").await;
}
#[tokio::test]
async fn actual_claude_signed_history_survives_scheduled_verification() {
    run_case("claude").await;
}
#[tokio::test]
async fn actual_gemini_signed_parts_survive_scheduled_verification() {
    run_case("gemini").await;
}
