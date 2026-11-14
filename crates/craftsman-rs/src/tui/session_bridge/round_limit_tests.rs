//! Actual TUI builder, HTTP requests, approved writes and durable limited turns.
use super::*;
use craftsman_harness::{AgentError, Checkpointer, Msg};
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::io::AsyncReadExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const HELPER: &str = "tui::session_bridge::round_limit_tests::native_terminal_round_limit_fixture";
const MARKER: &str = "craftsman-terminal-round-limit=";
const DONE: &str =
    "actual terminal round limit, HTTP, approved files, SQLite and UI assertions completed";

async fn drive_ui_turn(
    session: &Session,
    app: &mut App,
    rx: &mut mpsc::UnboundedReceiver<UiEvent>,
    prompt: &str,
) -> (usize, bool) {
    app.running = true;
    let original = (app.input.clone(), app.cursor);
    let agent = session.agent.clone();
    let prompt = prompt.to_owned();
    let mut task = tokio::spawn(async move { agent.run_turn(prompt).await });
    let mut approvals = 0;
    let result = loop {
        tokio::select! {
            result = &mut task => break result.unwrap(),
            event = rx.recv() => {
                handle_ui_event(app, event.unwrap(), Some(session));
                if matches!(app.modal, Some(Modal::Approval(_))) {
                    let Some(Modal::Approval(modal)) = app.modal.take() else { unreachable!() };
                    modal.response.send(ApprovalChoice::Approve).unwrap();
                    approvals += 1;
                }
                assert_eq!((app.input.clone(), app.cursor), original);
            }
        }
    };
    while let Ok(event) = rx.try_recv() {
        handle_ui_event(app, event, Some(session));
    }
    assert!(
        matches!(
            &result,
            Ok(TurnResult::Done(_)) | Err(AgentError::MaxRounds(_))
        ),
        "{result:?}"
    );
    let event = UiEvent::turn_result(result);
    let limited = matches!(event, UiEvent::TurnLimited(_));
    handle_ui_event(app, event, Some(session));
    assert!(!app.running && !app.cancel_requested);
    assert_eq!((app.input.clone(), app.cursor), original);
    (approvals, limited)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "owned terminal turn fixture; unmarked/unknown startup returns before config access"]
async fn native_terminal_round_limit_fixture() {
    let modes: Vec<_> = std::env::args_os()
        .filter_map(|arg| {
            arg.to_str()
                .and_then(|arg| arg.strip_prefix(MARKER))
                .map(str::to_owned)
        })
        .collect();
    if modes.len() != 1 || !matches!(modes[0].as_str(), "long" | "limit" | "legacy" | "invalid") {
        return;
    }
    let mode = &modes[0];
    let workspace = std::env::current_dir().unwrap();
    std::fs::write(workspace.join("data.txt"), "original").unwrap();
    let server = MockServer::start().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let requests = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let captured = requests.clone();
    let mutating = mode == "limit";
    let tools = if mutating { 3 } else { 51 };
    Mock::given(method("POST")).and(path("/chat/completions")).respond_with(move |request: &wiremock::Request| {
        let index = observed.fetch_add(1, Ordering::SeqCst);
        captured.lock().unwrap().push(serde_json::from_slice(&request.body).unwrap());
        let delta = if index < tools {
            let (name, args) = if mutating {
                if index == 0 {
                    ("write_file", serde_json::json!({"file_path":"effect.txt","content":"effect 1"}))
                } else {
                    ("edit_file", serde_json::json!({"file_path":"effect.txt","old_string":format!("effect {index}"),"new_string":format!("effect {}", index + 1)}))
                }
            } else { ("read_file", serde_json::json!({"file_path":"data.txt"})) };
            serde_json::json!({"tool_calls":[{"index":0,"id":format!("round-{index}"),"type":"function","function":{"name":name,"arguments":args.to_string()}}]})
        } else { serde_json::json!({"content":"done"}) };
        let finish = if index < tools { "tool_calls" } else { "stop" };
        let body = serde_json::json!({"choices":[{"delta":delta,"finish_reason":finish}],"usage":{"prompt_tokens":3,"completion_tokens":2}});
        ResponseTemplate::new(200).set_body_raw(format!("data: {body}\n\ndata: [DONE]\n\n"), "text/event-stream")
    }).mount(&server).await;
    let mut cfg: Config = serde_json::from_value(serde_json::json!({
        "base_url":server.uri(), "api_key":"fixture-key", "model":"fixture-model",
        "harness_profile":"general", "mcp_servers":[], "lsp_servers":{},
        "voice":{"enabled":false}, "gadgets":{"enabled":false}
    }))
    .unwrap();
    if mode != "long" {
        cfg.extra.insert(
            "terminal".into(),
            serde_json::json!({"max_rounds":if mode == "invalid" { 0 } else { 3 }}),
        );
    }
    let (tx, mut rx) = mpsc::unbounded_channel();
    let bridge = UiBridge { tx };
    let mut extras = bridge.session_extras();
    extras.cancellation_session = Some(craftsman_harness::SessionCancellation::default());
    extras.workspace_root = Some(workspace.clone());
    if mode == "legacy" {
        extras.terminal_round_limits = false;
    }
    let session = agent::build_session_ext(
        &cfg,
        Arc::new(Mutex::new(ApprovalState::default())),
        "",
        false,
        false,
        None,
        extras,
    )
    .await;
    if mode == "invalid" {
        assert!(session.is_none());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(
            !crate::config::config_dir().join("threads.db").exists(),
            "invalid limits fail before storage"
        );
        let UiEvent::Activity(text) = rx.try_recv().unwrap() else {
            panic!("missing configuration notice")
        };
        assert_eq!(
            text,
            "Invalid terminal.max_rounds: use a whole number from 1 to 1000."
        );
        eprintln!("{DONE}");
        return;
    }
    let session = session.unwrap();
    assert!(session.agent.durable_turn_checkpoints_enabled());
    let mut app = App::new(&cfg);
    app.input = "keep this draft".into();
    app.cursor = 4;
    let (approvals, limited) =
        drive_ui_turn(&session, &mut app, &mut rx, "Complete the fixture task.").await;
    let expected_calls = match mode.as_str() {
        "long" => 52,
        "limit" => 3,
        "legacy" => 50,
        _ => unreachable!(),
    };
    assert_eq!(calls.load(Ordering::SeqCst), expected_calls);
    assert_eq!(limited, mode != "long");
    assert_eq!(approvals, if mutating { 3 } else { 0 });
    assert_eq!(
        *session.usage.lock().unwrap(),
        (expected_calls as u64 * 3, expected_calls as u64 * 2)
    );
    assert!(
        app.transcript
            .iter()
            .all(|item| item.kind != TranscriptKind::Error)
    );
    let pauses: Vec<_> = app
        .transcript
        .iter()
        .filter(|item| item.text.starts_with("Paused after "))
        .collect();
    assert_eq!(pauses.len(), usize::from(limited));
    assert!(
        pauses
            .iter()
            .all(|item| item.kind == TranscriptKind::Activity)
    );
    let cp = session.checkpointer.as_ref().unwrap();
    let (_, saved, suspension) = cp.load_latest(&session.thread_id).await.unwrap().unwrap();
    assert!(suspension.is_none());
    assert_eq!(
        saved.custom["_craftsman_durable_turn_v1"]["in_progress"],
        mode == "legacy"
    );
    if mutating {
        assert_eq!(
            std::fs::read_to_string(workspace.join("effect.txt")).unwrap(),
            "effect 3"
        );
        assert_eq!(
            saved
                .messages
                .iter()
                .filter(|m| matches!(m, Msg::Tool { .. }))
                .count(),
            3
        );
        assert!(
            saved
                .messages
                .iter()
                .all(|m| !matches!(m, Msg::Tool { is_error: true, .. }))
        );
        assert_eq!(
            session
                .workspace_checkpoints
                .as_ref()
                .unwrap()
                .list()
                .await
                .unwrap()
                .len(),
            1
        );
        session.agent.load_thread(&session.thread_id).await.unwrap();
        assert!(session.agent.recovery_info().is_none());
        assert_eq!(
            calls.load(Ordering::SeqCst),
            3,
            "reopening does not replay tools or model calls"
        );
        assert_eq!(
            drive_ui_turn(&session, &mut app, &mut rx, "continue").await,
            (0, false)
        );
        assert_eq!(calls.load(Ordering::SeqCst), 4);
        assert_eq!(
            std::fs::read_to_string(workspace.join("effect.txt")).unwrap(),
            "effect 3"
        );
        let captured = requests.lock().unwrap();
        let messages = captured[3]["messages"].as_array().unwrap();
        assert_eq!(messages.iter().filter(|m| m["role"] == "tool").count(), 3);
        assert_eq!(messages.last().unwrap()["content"], "continue");
        assert_eq!(*session.usage.lock().unwrap(), (12, 8));
    } else {
        assert_eq!(
            std::fs::read_to_string(workspace.join("data.txt")).unwrap(),
            "original"
        );
    }
    session.lsp.shutdown().await;
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
        .current_dir(&workspace)
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
            panic!("owned terminal round-limit fixture exceeded its bound");
        }
    };
    assert!(out.len() <= 65536 && err.len() <= 65536);
    let error = std::str::from_utf8(&err).unwrap();
    assert!(status.success(), "{error}");
    assert_eq!(error.lines().filter(|line| *line == DONE).count(), 1);
    assert!(
        !error.contains("craftsman:"),
        "TUI diagnostics must stay in its event queue"
    );
}

#[tokio::test]
async fn actual_terminal_default_completes_a_task_beyond_fifty_rounds() {
    run_case("long").await;
}

#[tokio::test]
async fn actual_terminal_limit_saves_approved_work_and_continues_without_replay_or_draft_damage() {
    run_case("limit").await;
}

#[tokio::test]
async fn actual_legacy_builder_keeps_fifty_rounds_and_ignores_terminal_only_configuration() {
    run_case("legacy").await;
}

#[tokio::test]
async fn actual_terminal_invalid_round_budget_refuses_before_storage_or_model_effects() {
    run_case("invalid").await;
}

#[test]
fn round_limit_ui_uses_typed_errors_and_preserves_real_failure_messages() {
    assert!(matches!(
        UiEvent::turn_result(Err(AgentError::MaxRounds(200))),
        UiEvent::TurnLimited(200)
    ));
    let UiEvent::TurnFinished(Err(text)) =
        UiEvent::turn_result(Err(AgentError::Model("max rounds (50) exceeded".into())))
    else {
        panic!("model failure was hidden")
    };
    assert_eq!(text, "model error: max rounds (50) exceeded");
}
