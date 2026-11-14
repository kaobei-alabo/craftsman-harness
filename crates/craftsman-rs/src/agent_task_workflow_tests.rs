//! Marked actual builder/HTTP/SQLite workflow proof; original approval and native history.
use super::*;
use craftsman_harness::Checkpointer;
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::io::AsyncReadExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const HELPER: &str = "agent::task_workflow_tests::native_task_workflow_fixture";
const MARKER: &str = "craftsman-task-workflow=";
const DONE: &str =
    "actual task workflow, admitted tools, HTTP, native history and SQLite assertions completed";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "owned task-workflow fixture; unmarked/unknown startup returns before config access"]
async fn native_task_workflow_fixture() {
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
            "focused" | "legacy" | "denied" | "responses"
        )
    {
        return;
    }
    let focused = selected[0] != "legacy";
    let denied = selected[0] == "denied";
    let responses = selected[0] == "responses";
    let directory = std::env::current_dir().unwrap();
    std::fs::write(directory.join("note.txt"), "retained fixture file").unwrap();
    let server = MockServer::start().await;
    let requests = Arc::new(AtomicUsize::new(0));
    let captured = Arc::new(Mutex::new(Vec::<Value>::new()));
    let observed = requests.clone();
    let bodies = captured.clone();
    Mock::given(method("POST")).and(path(if responses { "/v1/responses" } else { "/chat/completions" })).respond_with(move |request: &wiremock::Request| {
        let index = observed.fetch_add(1, Ordering::SeqCst);
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        let system = if responses { body["instructions"].as_str().unwrap() } else { body["messages"][0]["content"].as_str().unwrap() };
        assert_eq!(system.contains(crate::task_workflow::WORKFLOW_GUIDANCE), focused);
        assert_eq!(system.contains(crate::task_workflow::REPEATED_ERROR_GUIDANCE), focused && !denied && index == 2);
        if focused { assert_eq!(system.matches(crate::task_workflow::WORKFLOW_GUIDANCE).count(), 1); }
        if responses && index > 0 { assert!(serde_json::to_string(&body).unwrap().contains("original-workflow-opaque")); }
        bodies.lock().unwrap().push(body);
        let (name, args) = if index < 2 {
            if denied { ("write_file", json!({"file_path":"denied.txt","content":"must never be written"})) }
            else { ("read_file", json!({"file_path":"missing.txt"})) }
        } else { ("ls", json!({"path":"."})) };
        if responses {
            let mut output = Vec::new();
            if index == 0 { output.push(json!({"type":"reasoning","id":"original-workflow-reasoning","encrypted_content":"original-workflow-opaque","summary":[]})); }
            if index < 3 { output.push(json!({"type":"function_call","id":format!("function-{index}"),"call_id":format!("call-{index}"),"name":name,"arguments":args.to_string(),"status":"completed"})); }
            else { output.push(json!({"type":"message","id":"workflow-answer","role":"assistant","status":"completed","content":[{"type":"output_text","text":"done","annotations":[]}]})); }
            let value = json!({"type":"response.completed","response":{"id":format!("workflow-response-{index}"),"object":"response","status":"completed","output":output,"usage":{"input_tokens":3,"output_tokens":2}}});
            ResponseTemplate::new(200).set_body_raw(format!("event: response.completed\ndata: {value}\n\n"), "text/event-stream")
        } else {
            let delta = if index < 3 { json!({"tool_calls":[{"index":0,"id":format!("call-{index}"),"type":"function","function":{"name":name,"arguments":args.to_string()}}]}) }
                else { json!({"content":"done"}) };
            let body = json!({"choices":[{"delta":delta,"finish_reason":if index < 3 { "tool_calls" } else { "stop" }}],"usage":{"prompt_tokens":3,"completion_tokens":2}});
            ResponseTemplate::new(200).set_body_raw(format!("data: {body}\n\ndata: [DONE]\n\n"), "text/event-stream")
        }
    }).expect(4).mount(&server).await;
    let cfg: Config = if responses {
        serde_json::from_value(json!({"model":"test/workflow-fixture","providers":{"test":{"protocol":"openai_responses","base_url":format!("{}/v1",server.uri()),"api_key":"fixture-key","models":{"workflow-fixture":{"settings":{"reasoning":null}}}}},"harness_profile":"general","voice":{"enabled":false},"gadgets":{"enabled":false},"mcp_servers":[],"lsp_servers":{}})).unwrap()
    } else {
        serde_json::from_value(json!({"base_url":server.uri(),"api_key":"fixture-key","model":"fixture-model","harness_profile":"general","voice":{"enabled":false},"gadgets":{"enabled":false},"mcp_servers":[],"lsp_servers":{}})).unwrap()
    };
    let approvals = Arc::new(AtomicUsize::new(0));
    let refusal_count = approvals.clone();
    let session = build_session_ext(
        &cfg,
        Arc::new(Mutex::new(ApprovalState::default())),
        "",
        false,
        false,
        None,
        SessionExtras {
            workspace_root: Some(directory.clone()),
            cancellation_session: Some(craftsman_harness::SessionCancellation::default()),
            token_sink: Some(Arc::new(|_| {})),
            activity_sink: Some(Arc::new(|_| {})),
            task_workflow: focused,
            task_focused_memory: true,
            approval_prompt: Some(Arc::new(move |_, _, _| {
                refusal_count.fetch_add(1, Ordering::SeqCst);
                ApprovalPromptResult::Reject("keep the decision".into())
            })),
            ..SessionExtras::default()
        },
    )
    .await
    .unwrap();
    let result = session
        .agent
        .run_turn("Complete the fixture task with appropriate checks.")
        .await
        .unwrap();
    let craftsman_harness::TurnResult::Done(output) = result else {
        panic!("unexpected suspension")
    };
    assert_eq!(output.final_text(), "done");
    assert_eq!(requests.load(Ordering::SeqCst), 4);
    assert_eq!(approvals.load(Ordering::SeqCst), if denied { 2 } else { 0 });
    assert!(!directory.join("denied.txt").exists());
    assert_eq!(
        std::fs::read_to_string(directory.join("note.txt")).unwrap(),
        "retained fixture file"
    );
    assert_eq!(*session.usage.lock().unwrap(), (12, 8));
    let (_, stored, suspension) = session
        .checkpointer
        .as_ref()
        .unwrap()
        .load_latest(&session.thread_id)
        .await
        .unwrap()
        .unwrap();
    assert!(suspension.is_none());
    assert_eq!(
        stored.custom["_craftsman_durable_turn_v1"]["in_progress"],
        false
    );
    assert!(
        stored
            .custom
            .keys()
            .all(|key| !key.contains("workflow") && !key.contains("progress"))
    );
    let tools: Vec<_> = stored
        .messages
        .iter()
        .filter_map(|m| {
            if let craftsman_harness::Msg::Tool { is_error, .. } = m {
                Some(*is_error)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(tools, vec![true, true, false]);
    let original = stored.messages.clone();
    session.agent.load_thread(&session.thread_id).await.unwrap();
    assert_eq!(
        requests.load(Ordering::SeqCst),
        4,
        "restore must never sample or replay"
    );
    assert_eq!(session.agent.state.lock().unwrap().messages, original);
    assert_eq!(captured.lock().unwrap().len(), 4);
    session.lsp.shutdown().await;
    eprintln!("{DONE}");
}

async fn run_case(mode: &str) {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config");
    std::fs::create_dir(&config).unwrap();
    let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
    command
        .args(["--ignored", "--exact", HELPER, "--nocapture"])
        .arg(format!("{MARKER}{mode}"))
        .current_dir(directory.path())
        .env_clear()
        .env("CRAFTSMAN_CONFIG_DIR", &config)
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
    let mut stdout = child.stdout.take().unwrap().take(65537);
    let mut stderr = child.stderr.take().unwrap().take(65537);
    let result = tokio::time::timeout(Duration::from_secs(20), async {
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
            panic!("owned task-workflow fixture exceeded its bound");
        }
    };
    assert!(out.len() <= 65536 && err.len() <= 65536);
    let error = std::str::from_utf8(&err).unwrap();
    assert!(status.success(), "{error}");
    assert_eq!(error.lines().filter(|line| *line == DONE).count(), 1);
}

#[tokio::test]
async fn actual_builder_guidance_recovers_from_repeated_file_errors_with_original_results() {
    run_case("focused").await;
}

#[tokio::test]
async fn actual_builder_legacy_policy_keeps_prior_requests_without_workflow_guidance() {
    run_case("legacy").await;
}

#[tokio::test]
async fn actual_denied_operations_remain_denied_without_recovery_pressure() {
    run_case("denied").await;
}

#[tokio::test]
async fn actual_named_responses_history_and_usage_survive_dynamic_error_guidance() {
    run_case("responses").await;
}
