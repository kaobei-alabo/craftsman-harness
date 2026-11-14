//! Actual builder/HTTP/tool/store warnings travel through the UI event queue.
use super::*;
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::io::AsyncReadExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const HELPER: &str = "tui::session_bridge::checkpoint_tests::native_checkpoint_ui_fixture";
const MARKER: &str = "craftsman-checkpoint-ui=";
const DONE: &str = "actual checkpoint UI, draft, HTTP and store assertions completed";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "owned checkpoint UI fixture; unmarked startup returns before config access"]
async fn native_checkpoint_ui_fixture() {
    let selected: Vec<_> = std::env::args_os()
        .filter_map(|value| {
            value
                .to_str()
                .and_then(|value| value.strip_prefix(MARKER))
                .map(str::to_owned)
        })
        .collect();
    if selected.len() != 1
        || !matches!(
            selected[0].as_str(),
            "readonly" | "warning" | "listing" | "nonregular"
        )
    {
        return;
    }
    if selected[0] == "listing" {
        use std::io::Write;
        let mut chunk = [b'x'; 65536];
        chunk[65535] = 0;
        let mut output = std::io::stdout().lock();
        for _ in 0..32 {
            if output.write_all(&chunk).is_err() {
                break;
            }
        }
        return;
    }
    if selected[0] == "nonregular" {
        #[cfg(unix)]
        {
            use std::os::unix::fs::FileTypeExt;
            let directory = std::env::current_dir().unwrap();
            let path = directory.join("fifo");
            let name = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
            assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
            assert!(
                std::fs::symlink_metadata(&path)
                    .unwrap()
                    .file_type()
                    .is_fifo()
            );
            let store = Arc::new(
                crate::checkpoints::LibsqlCheckpointer::open_at(
                    crate::config::config_dir().join("threads.db"),
                )
                .await
                .unwrap(),
            );
            let manager = crate::workspace_checkpoint::WorkspaceCheckpointManager::new(
                directory,
                store,
                "fifo-fixture",
            )
            .with_warning_sink(Some(Arc::new(|_| {})));
            let ticket = manager.begin_file_mutation("fifo", "fixture".into()).await;
            assert!(ticket.is_some());
            manager.finish_mutation(ticket).await;
            assert!(manager.list().await.unwrap().is_empty());
            eprintln!("{DONE}");
        }
        return;
    }
    let warning = selected[0] == "warning";
    let directory = std::env::current_dir().unwrap().join("task");
    std::fs::create_dir(&directory).unwrap();
    let server = MockServer::start().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    Mock::given(method("POST")).and(path("/chat/completions")).respond_with(move |_: &wiremock::Request| {
        let index = observed.fetch_add(1, Ordering::SeqCst);
        let delta = if warning && index == 0 {
            serde_json::json!({"tool_calls":[{"index":0,"id":"checkpoint-warning-write","type":"function","function":{"name":"write_file","arguments":"{\"file_path\":\"../outside.txt\",\"content\":\"fixture bytes\"}"}}]})
        } else { serde_json::json!({"content":"done"}) };
        let finish = if warning && index == 0 { "tool_calls" } else { "stop" };
        let body = serde_json::json!({"choices":[{"delta":delta,"finish_reason":finish}],"usage":{"prompt_tokens":3,"completion_tokens":2}});
        ResponseTemplate::new(200).set_body_raw(format!("data: {body}\n\ndata: [DONE]\n\n"), "text/event-stream")
    }).expect(if warning { 2 } else { 1 }).mount(&server).await;
    let cfg: Config = serde_json::from_value(serde_json::json!({
        "base_url":server.uri(), "api_key":"fixture-key", "model":"fixture-model",
        "harness_profile":"general", "mcp_servers":[], "lsp_servers":{},
        "voice":{"enabled":false}, "gadgets":{"enabled":false}
    }))
    .unwrap();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let bridge = UiBridge { tx };
    let mut extras = bridge.session_extras();
    extras.workspace_root = Some(directory.clone());
    let session = agent::build_session_ext(
        &cfg,
        Arc::new(Mutex::new(ApprovalState::default())),
        "",
        false,
        false,
        None,
        extras,
    )
    .await
    .unwrap();
    if !warning {
        std::fs::remove_dir(&directory).unwrap();
    }
    let mut app = App::new(&cfg);
    app.running = true;
    app.input = "keep this draft".into();
    app.cursor = 4;
    let original = (app.input.clone(), app.cursor, app.selection_anchor);
    let running = session.agent.clone();
    let mut turn =
        tokio::spawn(async move { running.run_turn("Complete the fixture task.").await });
    let mut approvals = 0;
    let result = loop {
        tokio::select! {
            result = &mut turn => break result.unwrap(),
            event = rx.recv() => {
                handle_ui_event(&mut app, event.unwrap(), Some(&session));
                if matches!(app.modal, Some(Modal::Approval(_))) {
                    let Some(Modal::Approval(modal)) = app.modal.take() else { unreachable!(); };
                    modal.response.send(ApprovalChoice::Approve).unwrap();
                    approvals += 1;
                }
                assert_eq!((app.input.clone(), app.cursor, app.selection_anchor), original);
            }
        }
    };
    assert!(result.is_ok());
    while let Ok(event) = rx.try_recv() {
        handle_ui_event(&mut app, event, Some(&session));
    }
    handle_ui_event(
        &mut app,
        UiEvent::TurnFinished(result.map_err(|error| error.to_string())),
        Some(&session),
    );
    assert_eq!(
        (app.input.clone(), app.cursor, app.selection_anchor),
        original
    );
    let notices: Vec<_> = app
        .transcript
        .iter()
        .filter(|item| item.text.contains("Workspace file checkpoint unavailable"))
        .collect();
    assert_eq!(notices.len(), usize::from(warning));
    assert!(
        notices
            .iter()
            .all(|item| item.kind == TranscriptKind::Activity)
    );
    assert_eq!(approvals, usize::from(warning));
    assert_eq!(calls.load(Ordering::SeqCst), if warning { 2 } else { 1 });
    assert_eq!(
        *session.usage.lock().unwrap(),
        if warning { (6, 4) } else { (3, 2) }
    );
    assert!(
        session
            .workspace_checkpoints
            .as_ref()
            .unwrap()
            .list()
            .await
            .unwrap()
            .is_empty()
    );
    if warning {
        assert_eq!(
            std::fs::read(directory.parent().unwrap().join("outside.txt")).unwrap(),
            b"fixture bytes"
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
            panic!("owned checkpoint UI fixture exceeded its bound");
        }
    };
    assert!(out.len() <= 65536 && err.len() <= 65536);
    let error = std::str::from_utf8(&err).unwrap();
    assert!(status.success(), "{error}");
    assert_eq!(error.lines().filter(|line| *line == DONE).count(), 1);
    assert!(!error.contains("craftsman: Workspace file checkpoint"));
    assert!(!error.contains("workspace checkpoint unavailable"));
    assert!(config.join("threads.db").is_file());
}

#[tokio::test]
async fn actual_builder_checkpoint_warning_is_ui_activity_and_preserves_the_composer() {
    run_case("warning").await;
}

#[tokio::test]
async fn actual_builder_readonly_turn_succeeds_without_a_scannable_workspace() {
    run_case("readonly").await;
}

#[cfg(unix)]
#[tokio::test]
async fn actual_snapshot_refuses_a_fifo_without_waiting_for_a_writer() {
    run_case("nonregular").await;
}
