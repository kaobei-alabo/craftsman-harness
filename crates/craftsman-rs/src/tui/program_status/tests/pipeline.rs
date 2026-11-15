//! Actual UI callbacks and builder/HTTP/file/SQLite proof in an isolated child.
use super::*;
use crate::tui::app::App;
use crate::tui::modals::handle_modal_key;
use crate::tui::session_bridge::{AgentSlot, ApprovalChoice, UiBridge, UiEvent, handle_ui_event};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const HELPER: &str = "tui::program_status::tests::pipeline::native_status_pipeline_fixture";
const MARKER: &str = "craftsman-osc7501-pipeline-v1=";
const COMPLETE: &str = "actual OSC 7501 UI, builder, HTTP, file and durable-save proof complete";

fn key(app: &mut App, code: KeyCode) {
    handle_modal_key(app, KeyEvent::new(code, KeyModifiers::NONE));
}

fn done() -> craftsman_harness::TurnResult {
    craftsman_harness::TurnResult::Done(craftsman_harness::TurnOutput {
        message: craftsman_harness::Msg::assistant("fixture response"),
    })
}

fn ui_cases() {
    let mut app = App::new(&Config::default());
    let draft = "keep composer draft";
    app.input = draft.into();
    app.cursor = draft.len();
    app.running = true;
    assert_eq!(app.current_program_status(false), Status::Working);
    let (response, receive) = mpsc::channel();
    handle_ui_event(
        &mut app,
        UiEvent::Approval {
            tool: "fixture".into(),
            summary: "secret\x1b]7501;state=done:app=forged\x1b\\".into(),
            details: "secret approval details".into(),
            response,
        },
        None,
    );
    assert_eq!(app.current_program_status(false), Status::Permission);
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Enter);
    assert!(matches!(receive.recv().unwrap(), ApprovalChoice::Deny(_)));
    assert_eq!(app.current_program_status(false), Status::Working);
    let (response, receive) = mpsc::channel();
    handle_ui_event(
        &mut app,
        UiEvent::AskUser {
            question: "secret question\x1b]52;c;forged\x07".into(),
            response,
        },
        None,
    );
    assert_eq!(app.current_program_status(false), Status::Question);
    for character in "answer π".chars() {
        key(&mut app, KeyCode::Char(character));
    }
    key(&mut app, KeyCode::Enter);
    assert_eq!(receive.recv().unwrap(), Some("answer π".into()));
    assert_eq!(app.current_program_status(false), Status::Working);
    assert_eq!(app.input, draft);
    assert_eq!(app.cursor, draft.len());

    handle_ui_event(
        &mut app,
        UiEvent::ToolActivity(crate::agent::ToolActivityEvent::Finished {
            call_id: "unknown".into(),
            tool: "fixture".into(),
            content: "ERROR\x1b]7501;state=error:app=forged\x1b\\".into(),
            is_error: true,
        }),
        None,
    );
    assert_eq!(app.current_program_status(false), Status::Working);
    app.request_turn_cancellation();
    assert_eq!(app.current_program_status(false), Status::Stopping);
    let turn = craftsman_harness::SessionCancellation::new().new_turn();
    turn.cancel();
    let (response, receive) = mpsc::channel();
    handle_ui_event(
        &mut app,
        UiEvent::ScopedPrompt {
            cancellation: turn,
            event: Box::new(UiEvent::AskUser {
                question: "late".into(),
                response,
            }),
        },
        None,
    );
    assert_eq!(receive.recv().unwrap(), None);
    assert_eq!(app.current_program_status(false), Status::Stopping);
    assert!(app.modal.is_none());
    app.finish_turn(Ok(done()), None);
    assert_eq!(app.current_program_status(false), Status::Idle);
    assert_eq!(app.input, draft);
    app.running = true;
    app.request_turn_cancellation();
    app.finish_turn(Err("actual save failed".into()), None);
    assert_eq!(app.current_program_status(false), Status::Error);
    app.running = true;
    app.discard_stream_deltas = false;
    app.finish_limited_turn(200);
    assert_eq!(app.current_program_status(false), Status::Paused);
    app.running = true;
    app.finish_turn(Ok(done()), None);
    assert_eq!(app.current_program_status(false), Status::Done);
    assert_eq!(app.current_program_status(true), Status::Working);
    app.closing = true;
    assert_eq!(app.current_program_status(false), Status::Stopping);
}

async fn actual_turn() {
    let server = MockServer::start().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let observed = requests.clone();
    let counter = calls.clone();
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(header("authorization", "Bearer synthetic-status-key"))
        .respond_with(move |request: &wiremock::Request| {
            observed.lock().unwrap().push(request.body_json::<Value>().unwrap());
            let round = counter.fetch_add(1, Ordering::SeqCst);
            let delta = match round {
                0 => json!({"tool_calls":[{"index":0,"id":"edit-status-fixture","type":"function","function":{"name":"edit_file","arguments":json!({"file_path":"note.txt","old_string":"before","new_string":"after"}).to_string()}}]}),
                1 => json!({"tool_calls":[{"index":0,"id":"ask-status-fixture","type":"function","function":{"name":"ask_user","arguments":json!({"question":"Fixture question"}).to_string()}}]}),
                2 => json!({"content":"Actual status fixture response."}),
                _ => panic!("unexpected additional model request"),
            };
            let body = json!({"choices":[{"delta":delta,"finish_reason":if round < 2 {"tool_calls"} else {"stop"}}],"usage":{"prompt_tokens":3,"completion_tokens":2}});
            ResponseTemplate::new(200).set_body_raw(format!("data: {body}\n\ndata: [DONE]\n\n"), "text/event-stream")
        })
        .mount(&server)
        .await;
    std::fs::write("note.txt", "before").unwrap();
    let cfg: Config = serde_json::from_value(json!({
        "base_url":format!("{}/v1",server.uri()),"api_key":"synthetic-status-key",
        "model":"status-fixture","permission_mode":"manual","harness_profile":"general",
        "voice":{"enabled":false},"gadgets":{"enabled":false},"lsp_servers":{},
        "terminal":{"max_rounds":8}
    }))
    .unwrap();
    let approval = Arc::new(Mutex::new(crate::hitl::ApprovalState {
        mode: crate::config::PermissionMode::Manual,
        auto_approve: false,
        auto_approve_network: false,
        shell_allow_list: Vec::new(),
    }));
    let (tx, mut events) = tokio::sync::mpsc::unbounded_channel();
    let root = craftsman_harness::SessionCancellation::new();
    let mut slot = AgentSlot::spawn(&cfg, approval, UiBridge { tx: tx.clone() }, root);
    let session = slot.get().await.as_ref().unwrap();
    let agent = session.agent.clone();
    let task = tokio::spawn(async move {
        tx.send(UiEvent::turn_result(
            agent
                .run_turn("Edit note.txt, ask the fixture question, then finish.")
                .await,
        ))
        .unwrap();
    });
    let mut app = App::new(&cfg);
    app.input = "unfinished composer draft".into();
    app.cursor = app.input.len();
    app.running = true;
    let mut reporter = Reporter::default();
    let mut bytes = Vec::new();
    reporter
        .update(
            &mut bytes,
            app.current_program_status(false),
            Some(&session.registry),
            false,
        )
        .unwrap();
    let (mut approvals, mut questions) = (0, 0);
    while app.running {
        let event = tokio::time::timeout(Duration::from_secs(15), events.recv())
            .await
            .unwrap()
            .unwrap();
        handle_ui_event(&mut app, event, Some(session));
        reporter
            .update(
                &mut bytes,
                app.current_program_status(false),
                Some(&session.registry),
                false,
            )
            .unwrap();
        match app.current_program_status(false) {
            Status::Permission => {
                approvals += 1;
                key(&mut app, KeyCode::Enter);
            }
            Status::Question => {
                questions += 1;
                for character in "fixture answer".chars() {
                    key(&mut app, KeyCode::Char(character));
                }
                key(&mut app, KeyCode::Enter);
            }
            _ => {}
        }
        reporter
            .update(
                &mut bytes,
                app.current_program_status(false),
                Some(&session.registry),
                false,
            )
            .unwrap();
    }
    task.await.unwrap();
    assert_eq!((approvals, questions), (1, 1));
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    assert_eq!(std::fs::read_to_string("note.txt").unwrap(), "after");
    assert_eq!(app.input, "unfinished composer draft");
    assert_eq!(app.current_program_status(false), Status::Done);
    assert!(!session.agent.checkpoint_failed());
    let parsed = reports(&bytes);
    assert_eq!(
        parsed
            .iter()
            .map(|report| report["state"].as_str())
            .collect::<Vec<_>>(),
        [
            "working", "blocked", "working", "blocked", "working", "done"
        ]
    );
    assert_eq!(parsed[1]["kind"], "permission");
    assert_eq!(parsed[3]["kind"], "question");
    let requests = requests.lock().unwrap();
    assert_eq!(
        requests[2]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| message["role"] == "tool")
            .count(),
        2
    );
    assert!(
        serde_json::to_string(&requests[2])
            .unwrap()
            .contains("fixture answer")
    );
    drop(requests);
    slot.shutdown().await;
    let database = libsql::Builder::new_local(crate::config::config_dir().join("threads.db"))
        .build()
        .await
        .unwrap();
    let connection = database.connect().unwrap();
    let mut rows = connection
        .query(
            "SELECT state_json FROM checkpoints ORDER BY seq DESC LIMIT 1",
            (),
        )
        .await
        .unwrap();
    let state: Value = serde_json::from_str(
        &rows
            .next()
            .await
            .unwrap()
            .unwrap()
            .get::<String>(0)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        state["custom"]["_craftsman_durable_turn_v1"]["in_progress"],
        false
    );
    assert_eq!(
        state["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| message["role"] == "tool")
            .count(),
        2
    );
    reporter.finish(&mut bytes, Status::Done).unwrap();
    std::fs::write("actual-status-reports.bin", &bytes).unwrap();
    std::fs::write(
        "actual-status-state.json",
        serde_json::to_vec_pretty(&state).unwrap(),
    )
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "isolated status pipeline; unmarked/unknown startup returns before config access"]
async fn native_status_pipeline_fixture() {
    let modes: Vec<_> = std::env::args_os()
        .filter_map(|argument| {
            argument
                .to_str()
                .and_then(|argument| argument.strip_prefix(MARKER))
                .map(str::to_owned)
        })
        .collect();
    if modes.len() != 1 || modes[0] != "ui" {
        return;
    }
    ui_cases();
    actual_turn().await;
    println!("{COMPLETE}");
}

#[tokio::test]
async fn actual_tui_event_pipeline_reports_approval_answer_and_saved_completion() {
    let directory = tempfile::tempdir().unwrap();
    let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
    command
        .args(["--ignored", "--exact", HELPER, "--nocapture", "--skip"])
        .arg(format!("{MARKER}ui"))
        .current_dir(directory.path())
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("CRAFTSMAN_CONFIG_DIR", directory.path().join("config"))
        .stdin(Stdio::null())
        .kill_on_drop(true);
    // Winsock expands its provider DLL path through SystemRoot. Retain only
    // that native prerequisite, not the ambient credential/user environment.
    #[cfg(windows)]
    command.env(
        "SystemRoot",
        std::env::var_os("SystemRoot").expect("Windows fixture requires SystemRoot"),
    );
    let output = tokio::time::timeout(Duration::from_secs(60), command.output())
        .await
        .unwrap()
        .unwrap();
    assert!(output.stdout.len() + output.stderr.len() <= 1024 * 1024);
    assert!(
        output.status.success(),
        "isolated status pipeline failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains(COMPLETE));
    assert_eq!(
        std::fs::read_to_string(directory.path().join("note.txt")).unwrap(),
        "after"
    );
    let parsed =
        reports(&std::fs::read(directory.path().join("actual-status-reports.bin")).unwrap());
    assert_eq!(parsed.last().unwrap()["state"], "done");
}
