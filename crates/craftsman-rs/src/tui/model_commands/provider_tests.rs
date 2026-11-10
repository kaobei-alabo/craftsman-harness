use super::*;
use crate::tui::session_bridge::UiBridge;
use craftsman_harness::Checkpointer;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
const HELPER: &str = "tui::model_commands::provider_tests::native_provider_fixture";
const MARKER: &str = "craftsman-native-provider-fixture=";
fn sse(value: Value) -> String {
    format!("data: {value}\n\n")
}
fn response(text: &str, items: Vec<Value>) -> ResponseTemplate {
    let items = if text.is_empty() {
        items
    } else {
        vec![
            json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":text}]}),
        ]
    };
    let mut body = String::new();
    if !text.is_empty() {
        body.push_str(&sse(
            json!({"type":"response.output_text.delta","delta":text}),
        ));
    }
    body.push_str(&sse(json!({"type":"response.completed","response":{"status":"completed","output":items,"usage":{"input_tokens":3,"output_tokens":2}}})));
    ResponseTemplate::new(200).set_body_raw(body, "text/event-stream")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "owned native-provider fixture; unmarked startup returns before path/config access"]
async fn native_provider_fixture() {
    let markers: Vec<_> = std::env::args_os()
        .filter_map(|arg| {
            arg.to_str()
                .and_then(|arg| arg.strip_prefix(MARKER))
                .map(str::to_owned)
        })
        .collect();
    if markers.len() != 1 {
        return;
    }
    let directory = std::path::PathBuf::from(&markers[0]);
    if !directory.is_absolute() {
        return;
    }
    let server = MockServer::start().await;
    let counts: Arc<[AtomicUsize; 4]> = Arc::new(std::array::from_fn(|_| AtomicUsize::new(0)));
    let counter = counts.clone();
    Mock::given(method("POST")).and(path("/v1/responses")).and(header("authorization","Bearer openai-key")).respond_with(move|req:&wiremock::Request|{
        let body:Value=serde_json::from_slice(&req.body).unwrap(); assert_eq!(body["model"],"gpt-fixture"); assert_eq!(body["reasoning"]["effort"],"high");
        if counter[0].fetch_add(1,Ordering::SeqCst)==0{
            response("",vec![json!({"type":"reasoning","id":"rs-fixture","encrypted_content":"openai-opaque-fixture","summary":[]}),json!({"type":"function_call","call_id":"native-write","name":"write_file","arguments":json!({"file_path":"proof.txt","content":"native proof"}).to_string()})])
        }else{assert!(body["input"].as_array().unwrap().iter().any(|item|item["call_id"]=="native-write"&&item["type"]=="function_call_output")); assert!(serde_json::to_string(&body).unwrap().contains("openai-opaque-fixture")); response("responses-ready",vec![])}
    }).expect(2).mount(&server).await;
    let counter = counts.clone();
    Mock::given(method("POST")).and(path("/claude/messages")).and(header("x-api-key","claude-key")).respond_with(move|req:&wiremock::Request|{
        let body:Value=serde_json::from_slice(&req.body).unwrap(); assert_eq!(body["model"],"claude-fixture"); assert_eq!(body["output_config"]["effort"],"high");
        let serialized=serde_json::to_string(&body).unwrap(); assert!(serialized.contains("responses-ready")); assert!(!serialized.contains("openai-opaque-fixture"));
        let first=counter[1].fetch_add(1,Ordering::SeqCst)==0;
        let blocks=if first{vec![json!({"type":"thinking","thinking":"fixture thought","signature":"claude-signature"}),json!({"type":"tool_use","id":"native-read","name":"read_file","input":{"file_path":"proof.txt"}})]}else{assert!(serialized.contains("claude-signature"));assert!(serialized.contains("native proof"));vec![json!({"type":"text","text":"claude-ready"})]};
        let mut stream=sse(json!({"type":"message_start","message":{"usage":{"input_tokens":5}}}));
        for(index,block)in blocks.into_iter().enumerate(){stream.push_str(&sse(json!({"type":"content_block_start","index":index,"content_block":block})));stream.push_str(&sse(json!({"type":"content_block_stop","index":index})));}
        stream.push_str(&sse(json!({"type":"message_delta","delta":{"stop_reason":if first{"tool_use"}else{"end_turn"}},"usage":{"output_tokens":3}})));stream.push_str(&sse(json!({"type":"message_stop"})));
        ResponseTemplate::new(200).set_body_raw(stream,"text/event-stream")
    }).expect(2).mount(&server).await;
    let counter = counts.clone();
    Mock::given(method("POST")).and(path("/gemini/models/gemini-fixture:streamGenerateContent")).and(header("x-goog-api-key","gemini-key")).respond_with(move|req:&wiremock::Request|{
        let body:Value=serde_json::from_slice(&req.body).unwrap(); assert_eq!(body["generationConfig"]["thinkingConfig"]["thinkingLevel"],"HIGH");
        let serialized=serde_json::to_string(&body).unwrap();assert!(serialized.contains("claude-ready"));assert!(!serialized.contains("claude-signature"));assert!(!serialized.contains("openai-opaque-fixture"));
        let first=counter[2].fetch_add(1,Ordering::SeqCst)==0;
        let parts=if first{vec![json!({"functionCall":{"name":"read_file","args":{"file_path":"proof.txt"}},"thoughtSignature":"gemini-signature"})]}else{
            assert!(serialized.contains("gemini-signature"));assert!(serialized.contains("native proof"));
            let result=body["contents"].as_array().unwrap().iter().flat_map(|content|content["parts"].as_array().unwrap()).filter_map(|part|part.get("functionResponse")).last().unwrap();assert!(result.get("id").is_none());vec![json!({"text":"gemini-ready"})]
        };
        ResponseTemplate::new(200).set_body_raw(sse(json!({"candidates":[{"content":{"parts":parts},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":7,"candidatesTokenCount":3,"thoughtsTokenCount":2}})),"text/event-stream")
    }).expect(2).mount(&server).await;
    let counter = counts.clone();
    Mock::given(method("POST")).and(path("/compat/chat/completions")).and(header("authorization","Bearer compat-key")).respond_with(move|req:&wiremock::Request|{
        let body:Value=serde_json::from_slice(&req.body).unwrap();assert_eq!(body["model"],"org/custom");assert_eq!(body["reasoning_effort"],"low");let serialized=serde_json::to_string(&body).unwrap();assert!(serialized.contains("gemini-ready"));assert!(!serialized.contains("gemini-signature"));counter[3].fetch_add(1,Ordering::SeqCst);
        ResponseTemplate::new(200).set_body_raw(format!("{}data: [DONE]\n\n",sse(json!({"choices":[{"delta":{"content":"compatible-ready"},"finish_reason":"stop"}],"usage":{"prompt_tokens":13,"completion_tokens":4}}))),"text/event-stream")
    }).expect(1).mount(&server).await;
    let mut providers = serde_json::Map::new();
    for (name, protocol, base, key, model, effort) in [
        (
            "openai",
            "openai_responses",
            "v1",
            "openai-key",
            "gpt-fixture",
            "high",
        ),
        (
            "anthropic",
            "anthropic_messages",
            "claude",
            "claude-key",
            "claude-fixture",
            "low",
        ),
        (
            "google",
            "gemini_generate_content",
            "gemini",
            "gemini-key",
            "gemini-fixture",
            "low",
        ),
        (
            "compat",
            "openai_chat_completions",
            "compat",
            "compat-key",
            "org/custom",
            "high",
        ),
    ] {
        providers.insert(name.into(),json!({"protocol":protocol,"base_url":format!("{}/{base}",server.uri()),"api_key":key,"models":{model:{"reasoning_controls":{"type":"effort","values":["low","high"]},"settings":{"reasoning":{"type":"effort","value":effort}}}}}));
    }
    let mut cfg: Config = serde_json::from_value(
        json!({"model":"openai/gpt-fixture","providers":providers,"harness_profile":"general"}),
    )
    .unwrap();
    let approval = Arc::new(Mutex::new(ApprovalState {
        auto_approve: true,
        mode: crate::config::PermissionMode::BypassPermissions,
        ..Default::default()
    }));
    let (tx, _events) = tokio::sync::mpsc::unbounded_channel();
    let root = craftsman_harness::SessionCancellation::new();
    let mut slot = AgentSlot::spawn(&cfg, approval.clone(), UiBridge { tx }, root.clone());
    let mut app = App::new(&cfg);
    let first = slot.get().await.as_ref().unwrap();
    first
        .agent
        .run_turn("Write proof.txt, then finish.")
        .await
        .unwrap();
    let thread = first.thread_id.clone();
    let mut sequence = first
        .checkpointer
        .as_ref()
        .unwrap()
        .load_latest(&thread)
        .await
        .unwrap()
        .unwrap()
        .0;
    assert_eq!(
        std::fs::read_to_string(directory.join("proof.txt")).unwrap(),
        "native proof"
    );
    for (model, effort) in [
        ("anthropic/claude-fixture", ReasoningEffort::High),
        ("google/gemini-fixture", ReasoningEffort::High),
        ("compat/org/custom", ReasoningEffort::Low),
    ] {
        select_model(
            &mut cfg,
            ModelSelection {
                model: Some(model.into()),
                effort: Some(Some(effort)),
            },
            &mut slot,
            &approval,
            &mut app,
        );
        let session = slot.get().await.as_ref().unwrap();
        assert_eq!(session.thread_id, thread);
        session
            .agent
            .run_turn("Continue, inspect the proof if needed, and finish.")
            .await
            .unwrap();
        let checkpoint = session
            .checkpointer
            .as_ref()
            .unwrap()
            .load_latest(&thread)
            .await
            .unwrap()
            .unwrap();
        assert!(checkpoint.0 > sequence);
        sequence = checkpoint.0;
        assert_eq!(
            std::fs::read_to_string(directory.join("proof.txt")).unwrap(),
            "native proof"
        );
    }
    assert_eq!(*slot.peek().unwrap().usage.lock().unwrap(), (43, 24));
    assert_eq!(
        counts
            .iter()
            .map(|count| count.load(Ordering::SeqCst))
            .collect::<Vec<_>>(),
        vec![2, 2, 2, 1]
    );
    let saved = Config::load().unwrap().unwrap();
    assert_eq!(saved.model.as_deref(), Some("compat/org/custom"));
    assert_eq!(
        crate::model_controls::display_effort(&saved).as_deref(),
        Some("low")
    );
    root.cancel();
    slot.shutdown().await;
    // Exercise the actual legacy builder, not only the client constructor.
    Mock::given(method("POST"))
        .and(path("/legacy/chat/completions"))
        .respond_with(ResponseTemplate::new(401).set_body_string("legacy-error-marker"))
        .expect(1)
        .mount(&server)
        .await;
    let legacy: Config = serde_json::from_value(json!({"base_url":format!("{}/legacy",server.uri()),"api_key":"legacy-key","model":"legacy-model","harness_profile":"general"})).unwrap();
    let (tx, _events) = tokio::sync::mpsc::unbounded_channel();
    let root = craftsman_harness::SessionCancellation::new();
    let mut legacy_slot = AgentSlot::spawn(&legacy, approval, UiBridge { tx }, root.clone());
    let legacy_session = legacy_slot.get().await.as_ref().unwrap();
    let error = legacy_session
        .agent
        .run_turn("Check the legacy constructor.")
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("legacy-error-marker"));
    root.cancel();
    legacy_slot.shutdown().await;
    server.verify().await;
    eprintln!(
        "actual native provider routing, durable switching, usage and no-replay assertions completed"
    );
}

#[tokio::test]
async fn actual_terminal_switches_all_protocols_with_durable_history_and_no_replay() {
    use std::process::Stdio;
    use tokio::io::AsyncReadExt;
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config");
    std::fs::create_dir(&config).unwrap();
    let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--ignored",
            "--exact",
            HELPER,
            "--test-threads=1",
            "--nocapture",
            "--skip",
        ])
        .arg(format!("{MARKER}{}", directory.path().to_str().unwrap()))
        .current_dir(directory.path())
        .env_clear()
        .env("CRAFTSMAN_CONFIG_DIR", config)
        .env("HOME", directory.path())
        .env("USERPROFILE", directory.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    if let Some(root) = std::env::var_os("SystemRoot") {
        command.env("SystemRoot", root);
    }
    let mut child = command.spawn().unwrap();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let outcome = tokio::time::timeout(Duration::from_secs(60), async {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut stdout = stdout.take(65537);
        let mut stderr = stderr.take(65537);
        let (status, _, _) = tokio::try_join!(
            child.wait(),
            stdout.read_to_end(&mut out),
            stderr.read_to_end(&mut err)
        )?;
        Ok::<_, std::io::Error>((status, out, err))
    })
    .await;
    let (status, out, err) = match outcome {
        Ok(Ok(result)) => result,
        _ => {
            let _ = tokio::time::timeout(Duration::from_secs(5), child.kill()).await;
            panic!("owned native provider fixture exceeded its bound");
        }
    };
    assert!(out.len() <= 65536 && err.len() <= 65536);
    let err = std::str::from_utf8(&err).unwrap();
    assert!(status.success(), "{err}");
    assert_eq!(err.lines().filter(|line|*line=="actual native provider routing, durable switching, usage and no-replay assertions completed").count(),1);
}

#[test]
fn provider_model_picker_and_budget_choices_are_scoped_to_the_current_model() {
    let cfg:Config=serde_json::from_value(json!({"model":"claude/m","providers":{"claude":{"protocol":"anthropic_messages","models":{"m":{"label":"Claude","reasoning_controls":{"type":"budget","min":1024,"max":4096},"settings":{"reasoning":null}}}}}})).unwrap();
    let mut app = App::new(&cfg);
    show_effort_picker(&cfg, &mut app);
    let Some(Modal::Selection(picker)) = &app.modal else {
        panic!("missing budget picker")
    };
    assert_eq!(picker.items.len(), 3);
    assert!(
        matches!(&picker.items[2].enter_action,Some(AppAction::SelectModelBudget{model,budget:4096})if model=="claude/m")
    );
    assert!(cfg.providers["claude"].models["m"].settings["reasoning"].is_null());
    let models = crate::model_controls::configured_catalog(&cfg).unwrap();
    show_model_catalog(&mut app, models, Some("claude/m"));
    let Some(Modal::Selection(picker)) = &app.modal else {
        panic!("missing model picker")
    };
    assert!(
        matches!(&picker.items[0].enter_action,Some(AppAction::SelectModel(id))if id=="claude/m")
    );
}
