//! Actual owned sessions across pause, compaction, transfer, cancellation and exit.
use super::*;
use crate::tui::input::handle_key;
use crate::tui::program_status::Status;
use craftsman_harness::{
    Agent, AgentError, Checkpointer, Msg, Tool, ToolCtx, ToolDef, ToolOutcome,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::{Value, json};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::io::AsyncReadExt;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const HELPER: &str = "tui::session_bridge::reliability_tests::owned_recovery_reliability_fixture";
const MARKER: &str = "craftsman-recovery-reliability=";
const DONE: &str = "actual recovery transition assertions completed";
const WAIT: Duration = Duration::from_secs(5);
const RECOVERY: &str = "_craftsman_durable_turn_v1";
const PRIOR_TURNS: usize = 26;
const INITIAL: &str = "Write effect.txt once and ask before continuing.";
const STEERING: [&str; 2] = [
    "Keep the completed file unchanged; retain α and the original result.",
    "保留這個已接受的輸入。\nContinue only after explicit follow-up.",
];
const CANCEL_STEERING: &str = "Keep the earlier approvals and this accepted cancellation input.";

fn response(items: Vec<Value>) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(
        format!(
            "data: {}\n\n",
            json!({"type":"response.completed","response":{"status":"completed","output":items,
                "usage":{"input_tokens":3,"output_tokens":2}}})
        ),
        "text/event-stream",
    )
}

fn text_response(text: &str) -> ResponseTemplate {
    response(vec![json!({"type":"message","role":"assistant",
        "content":[{"type":"output_text","text":text}]})])
}

fn call(id: &str, name: &str, arguments: Value) -> Value {
    json!({"type":"function_call","call_id":id,"name":name,"arguments":arguments.to_string()})
}

fn has_string(value: &Value, expected: &str) -> bool {
    match value {
        Value::String(text) => text == expected,
        Value::Array(values) => values.iter().any(|value| has_string(value, expected)),
        Value::Object(values) => values.values().any(|value| has_string(value, expected)),
        _ => false,
    }
}

fn config(server: &MockServer, rounds: usize, context: bool) -> Config {
    let mut value = json!({"model":"fixture/recovery-old","providers":{"fixture":{
        "protocol":"openai_responses","base_url":format!("{}/v1",server.uri()),
        "api_key":"isolated-recovery-key","models":{
            "recovery-old":{"settings":{"reasoning":null}},
            "recovery-new":{"settings":{"reasoning":null}}}}},
        "terminal":{"max_rounds":rounds},"harness_profile":"general",
        "permission_mode":"manual","mcp_servers":[],"lsp_servers":{},
        "voice":{"enabled":false},"gadgets":{"enabled":false}});
    if context {
        value["context"] = json!({"enabled":true,"max_bytes":32768});
    }
    serde_json::from_value(value).unwrap()
}

fn draft(cfg: &Config) -> App {
    let mut app = App::new(cfg);
    app.input = "keep this separate composer draft".into();
    app.cursor = 4;
    app.selection_anchor = None;
    app
}

fn assert_draft(app: &App) {
    assert_eq!(app.input, "keep this separate composer draft");
    assert_eq!(app.cursor, 4);
    assert_eq!(app.selection_anchor, None);
}

type RunningTurn = tokio::task::JoinHandle<Result<TurnResult, AgentError>>;

async fn until_question(
    session: &Session,
    app: &mut App,
    events: &mut mpsc::UnboundedReceiver<UiEvent>,
    input: &str,
) -> (RunningTurn, usize) {
    app.running = true;
    let agent = session.agent.clone();
    let input = input.to_owned();
    let mut task = tokio::spawn(async move { agent.run_turn(input).await });
    let mut approvals = 0;
    tokio::time::timeout(WAIT, async {
        loop {
            tokio::select! {
                result = &mut task => panic!("turn returned before its actual question: {result:?}"),
                event = events.recv() => handle_ui_event(app, event.unwrap(), Some(session)),
            }
            assert_draft(app);
            if matches!(app.modal, Some(Modal::Approval(_))) {
                let Some(Modal::Approval(modal)) = app.modal.take() else { unreachable!() };
                modal.response.send(ApprovalChoice::Approve).unwrap();
                approvals += 1;
            }
            if matches!(app.modal, Some(Modal::Ask(_))) {
                break;
            }
        }
    })
    .await
    .expect("actual approval/question readiness");
    assert!(session.agent.is_turn_active());
    (task, approvals)
}

async fn returned(task: RunningTurn) -> Result<TurnResult, AgentError> {
    tokio::time::timeout(WAIT, task)
        .await
        .expect("owned turn must actually return")
        .unwrap()
}

fn finish_ui(
    session: &Session,
    app: &mut App,
    events: &mut mpsc::UnboundedReceiver<UiEvent>,
    result: Result<TurnResult, AgentError>,
) {
    while let Ok(event) = events.try_recv() {
        handle_ui_event(app, event, Some(session));
    }
    let event = UiEvent::turn_result_with_controls(result, session.controls.as_deref());
    handle_ui_event(app, event, Some(session));
    assert!(!app.running && !app.cancel_requested);
    assert_draft(app);
}

fn cancel_question(session: &Session, app: &mut App) {
    assert!(!handle_key(
        app,
        KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
    ));
    apply_turn_cancellation(app, Some(&session.agent));
    assert!(app.running, "actual tool cleanup still owns the turn");
    assert_draft(app);
}

fn assert_group_results(messages: &[Msg]) {
    for (index, message) in messages.iter().enumerate() {
        if let Msg::Assistant { tool_calls, .. } = message {
            for (offset, call) in tool_calls.iter().enumerate() {
                assert!(
                    matches!(messages.get(index + offset + 1), Some(Msg::Tool { tool_call_id, .. })
                    if tool_call_id == &call.id),
                    "actual tool group must precede accepted steering"
                );
            }
        }
    }
}

fn write_evidence(directory: &Path, session: &Session, extra: Value) {
    std::fs::write(
        directory.join("recovery-evidence.json"),
        serde_json::to_vec(&json!({"thread":session.thread_id,"facts":extra})).unwrap(),
    )
    .unwrap();
}

async fn pause_transfer_cancel(directory: &Path) {
    let server = MockServer::start().await;
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let summary_started = Arc::new(tokio::sync::Notify::new());
    let captured = requests.clone();
    let started = summary_started.clone();
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .and(header("authorization", "Bearer isolated-recovery-key"))
        .respond_with(move |request: &wiremock::Request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            let ordinal = {
                let mut requests = captured.lock().unwrap();
                let ordinal = requests.len();
                requests.push(body.clone());
                ordinal
            };
            if ordinal < PRIOR_TURNS {
                assert_eq!(body["model"], "recovery-old");
                return text_response(&format!("Completed prior fixture turn {ordinal}."));
            }
            match ordinal - PRIOR_TURNS {
                0 => {
                    assert_eq!(body["model"], "recovery-old");
                    response(vec![
                        json!({"type":"reasoning","id":"old-reasoning",
                            "encrypted_content":"old-recovery-opaque","summary":[]}),
                        call("write-once", "write_file", json!({"file_path":"effect.txt","content":"one retained physical write"})),
                        call("pause-question", "ask_user", json!({"question":"May this limited turn finish?"})),
                    ])
                }
                1 | 2 => {
                    assert!(body.get("tools").is_none());
                    assert!(has_string(&body["input"], INITIAL));
                    let response = text_response(&format!("Original accepted task: {INITIAL}"));
                    if ordinal == PRIOR_TURNS + 1 {
                        started.notify_one();
                        response.set_delay(Duration::from_secs(30))
                    } else {
                        response
                    }
                }
                3 => {
                    assert_eq!(body["model"], "recovery-new");
                    for input in STEERING {
                        assert!(has_string(&body["input"], input));
                    }
                    assert!(!body.to_string().contains("old-recovery-opaque"));
                    response(vec![
                        call("read-after-transfer", "read_file", json!({"file_path":"effect.txt"})),
                        call("cancel-question", "ask_user", json!({"question":"May the new model change the file?"})),
                        call("never-write", "write_file", json!({"file_path":"forbidden.txt","content":"must never be written"})),
                    ])
                }
                4 => {
                    assert_eq!(body["model"], "recovery-new");
                    assert!(has_string(&body["input"], CANCEL_STEERING));
                    assert!(!body.to_string().contains("old-recovery-opaque"));
                    text_response("Explicit continuation retained the completed work.")
                }
                _ => panic!("unexpected summary, model retry or replay"),
            }
        })
        .expect((PRIOR_TURNS + 5) as u64)
        .mount(&server)
        .await;
    let mut cfg = config(&server, 1, true);
    let approval = Arc::new(Mutex::new(ApprovalState::default()));
    let (tx, mut events) = mpsc::unbounded_channel();
    let mut slot = AgentSlot::spawn(
        &cfg,
        approval.clone(),
        UiBridge { tx },
        craftsman_harness::SessionCancellation::new(),
    );
    let retired_owner = slot.cancellation();
    let session = slot.get().await.as_ref().unwrap();
    // Real completed turns cross the existing 50-checkpoint retention window.
    // No transcript prefix or checkpoint sequence is planted by the fixture.
    for index in 0..PRIOR_TURNS {
        let input =
            format!("Record completed fixture turn {index} before the later owned recovery task.");
        let result = session.agent.run_turn(input).await.unwrap();
        assert!(matches!(result, TurnResult::Done(output)
            if output.final_text() == format!("Completed prior fixture turn {index}.")));
    }
    let listed = session
        .checkpointer
        .as_ref()
        .unwrap()
        .list_threads()
        .await
        .unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, session.thread_id);
    assert_eq!(listed[0].checkpoints, 50);
    let mut app = draft(&cfg);
    let (task, approvals) = until_question(session, &mut app, &mut events, INITIAL).await;
    assert_eq!(approvals, 1);
    for input in STEERING {
        assert!(session.agent.steer(input));
    }
    let Some(Modal::Ask(question)) = app.modal.take() else {
        unreachable!()
    };
    question
        .response
        .send(Some("Finish this turn without more sampling.".into()))
        .unwrap();
    let result = returned(task).await;
    assert!(matches!(&result, Err(AgentError::MaxRounds(1))));
    finish_ui(session, &mut app, &mut events, result);
    assert_eq!(app.current_program_status(false), Status::Paused);
    let original = session.agent.state.lock().unwrap().messages.clone();
    assert_group_results(&original);
    for input in STEERING {
        assert_eq!(
            original
                .iter()
                .filter(|message| **message == Msg::user(input))
                .count(),
            1
        );
    }
    let store = session.checkpointer.as_ref().unwrap().clone();
    let thread = session.thread_id.clone();
    let (paused_sequence, paused, _) = store.load_latest(&thread).await.unwrap().unwrap();
    assert_eq!(paused.messages, original);
    assert_eq!(paused.custom[RECOVERY]["in_progress"], false);
    assert_eq!(requests.lock().unwrap().len(), PRIOR_TURNS + 1);
    let modified = std::fs::metadata(directory.join("effect.txt"))
        .unwrap()
        .modified()
        .unwrap();

    // A real pending summary is cancelled after a saved pause; originals stay
    // both live and durable before a separately requested successful compaction.
    let turn = retired_owner.new_turn();
    let running = turn.clone();
    let agent = session.agent.clone();
    let compaction = session.compaction.clone();
    let task = tokio::spawn(async move {
        running
            .scope(crate::tui::commands::compact_session(agent, compaction))
            .await
    });
    tokio::time::timeout(WAIT, summary_started.notified())
        .await
        .unwrap();
    assert!(!task.is_finished());
    assert!(!session.agent.steer("queue this as a separate turn"));
    turn.cancel();
    let output = tokio::time::timeout(WAIT, task).await.unwrap().unwrap();
    assert!(output.contains("cancelled") && !output.contains("messages (saved)"));
    let (cancelled_sequence, cancelled, _) = store.load_latest(&thread).await.unwrap().unwrap();
    assert!(cancelled_sequence > paused_sequence);
    assert_eq!(cancelled.messages, original);
    assert_eq!(session.agent.state.lock().unwrap().messages, original);
    assert_eq!(cancelled.custom[RECOVERY]["in_progress"], true);
    assert!(!session.agent.is_turn_active());

    let counts = session
        .agent
        .compact_idle(&session.compaction)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(counts.0, original.len());
    let (compacted_sequence, compacted, _) = store.load_latest(&thread).await.unwrap().unwrap();
    assert!(compacted_sequence > cancelled_sequence);
    assert_eq!(compacted.custom[RECOVERY]["in_progress"], false);
    assert!(compacted.messages.len() < original.len());
    for message in original.iter().skip(PRIOR_TURNS * 2 + 1) {
        assert!(
            compacted.messages.contains(message),
            "the actual call group and accepted steering remain verbatim"
        );
    }
    assert_eq!(
        &compacted.messages[1..],
        &original[PRIOR_TURNS * 2 + 1..],
        "compaction retains the complete original tool group and accepted input order"
    );
    assert_group_results(&compacted.messages);
    let usage = *session.usage.lock().unwrap();
    assert_eq!(
        usage,
        ((PRIOR_TURNS as u64 + 1) * 3, (PRIOR_TURNS as u64 + 1) * 2)
    );
    cfg.model = Some("fixture/recovery-new".into());
    slot.rebuild(&cfg, approval.clone());
    let replacement_owner = slot.cancellation();
    let session = slot.get().await.as_ref().unwrap();
    assert!(retired_owner.is_cancelled());
    assert!(!replacement_owner.is_cancelled());
    assert_eq!(session.thread_id, thread);
    assert_eq!(
        session.agent.state.lock().unwrap().messages,
        compacted.messages
    );
    assert!(session.agent.state.lock().unwrap().last_usage.is_none());
    assert_eq!(*session.usage.lock().unwrap(), usage);
    assert_eq!(
        store.load_latest(&thread).await.unwrap().unwrap().0,
        compacted_sequence
    );
    assert_eq!(
        requests.lock().unwrap().len(),
        PRIOR_TURNS + 3,
        "transfer makes no request or checkpoint"
    );

    let (task, approvals) = until_question(
        session,
        &mut app,
        &mut events,
        "Use the selected new model to inspect the retained file.",
    )
    .await;
    assert_eq!(approvals, 0);
    assert!(session.agent.steer(CANCEL_STEERING));
    cancel_question(session, &mut app);
    assert!(!session.agent.steer("late input must be queued separately"));
    let result = returned(task).await;
    assert!(
        matches!(&result, Ok(TurnResult::Done(output)) if output.final_text() == "Stopped by the user.")
    );
    finish_ui(session, &mut app, &mut events, result);
    assert_eq!(app.current_program_status(false), Status::Idle);
    let (cancel_sequence, saved, _) = store.load_latest(&thread).await.unwrap().unwrap();
    assert!(cancel_sequence > compacted_sequence);
    assert_eq!(saved.custom[RECOVERY]["in_progress"], false);
    assert_group_results(&saved.messages);
    for input in STEERING.into_iter().chain([CANCEL_STEERING]) {
        assert_eq!(
            saved
                .messages
                .iter()
                .filter(|message| **message == Msg::user(input))
                .count(),
            1
        );
    }
    assert!(saved.messages.iter().any(
        |message| matches!(message, Msg::Tool { tool_call_id, content, is_error:false }
        if tool_call_id == "read-after-transfer" && content.contains("one retained physical write"))
    ));
    assert!(saved.messages.iter().any(
        |message| matches!(message, Msg::Tool { tool_call_id, content, is_error:true }
        if tool_call_id == "never-write" && content.contains("cancelled"))
    ));
    assert!(!directory.join("forbidden.txt").exists());
    assert!(!approval.lock().unwrap().auto_approve);
    assert_eq!(
        *session.usage.lock().unwrap(),
        ((PRIOR_TURNS as u64 + 2) * 3, (PRIOR_TURNS as u64 + 2) * 2)
    );
    assert!(slot.peek_mut().unwrap().load_thread(&thread).await.unwrap());
    let session = slot.peek().unwrap();
    assert!(session.agent.recovery_info().is_none());
    assert_eq!(session.agent.state.lock().unwrap().messages, saved.messages);
    assert_eq!(
        requests.lock().unwrap().len(),
        PRIOR_TURNS + 4,
        "reopen makes no request"
    );
    let result = session
        .agent
        .run_turn("Explicitly continue after inspecting the saved cancellation.")
        .await
        .unwrap();
    assert!(
        matches!(&result, TurnResult::Done(output) if output.final_text() == "Explicit continuation retained the completed work.")
    );
    assert_eq!(
        *session.usage.lock().unwrap(),
        ((PRIOR_TURNS as u64 + 3) * 3, (PRIOR_TURNS as u64 + 3) * 2)
    );
    assert_eq!(
        std::fs::read(directory.join("effect.txt")).unwrap(),
        b"one retained physical write"
    );
    assert_eq!(
        std::fs::metadata(directory.join("effect.txt"))
            .unwrap()
            .modified()
            .unwrap(),
        modified
    );
    write_evidence(
        directory,
        session,
        json!({"requests":requests.lock().unwrap().len(),"prior_completed_turns":PRIOR_TURNS,"paused_sequence":paused_sequence,
        "compacted_sequence":compacted_sequence,"cancel_sequence":cancel_sequence,"approvals":1}),
    );
    slot.shutdown().await;
    assert!(!slot.peek().unwrap().agent.checkpoint_failed());
}

/// Test-only replacement for the registered write tool. The actual app approval
/// middleware still sees write_file and requires a separate approval per call.
/// Exit occurs after real create/write/sync, before a ToolOutcome can be saved.
struct CrashAfterWrite {
    directory: PathBuf,
    store: Arc<Mutex<Option<Arc<crate::checkpoints::LibsqlCheckpointer>>>>,
    thread: Arc<Mutex<Option<String>>>,
    approvals: Arc<AtomicUsize>,
    requests: Arc<Mutex<Vec<Value>>>,
}

#[async_trait::async_trait]
impl Tool for CrashAfterWrite {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "write_file".into(),
            description: "Owned recovery crash fixture.".into(),
            parameters: json!({"type":"object","properties":{"file_path":{"type":"string"},
                "content":{"type":"string"}},"required":["file_path","content"]}),
        }
    }
    async fn call(&self, args: Value, _: &mut ToolCtx<'_>) -> Result<ToolOutcome, AgentError> {
        let name = args["file_path"].as_str().unwrap();
        assert!(matches!(name, "first.txt" | "second.txt"));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(self.directory.join(name))
            .unwrap();
        file.write_all(args["content"].as_str().unwrap().as_bytes())
            .unwrap();
        file.sync_all().unwrap();
        if name == "first.txt" {
            return Ok(ToolOutcome::ok(
                "first physical write returned and was committed",
            ));
        }
        let store = self.store.lock().unwrap().clone().unwrap();
        let thread = self.thread.lock().unwrap().clone().unwrap();
        let (sequence, saved, suspension) = store.load_latest(&thread).await.unwrap().unwrap();
        let approvals = self.approvals.load(Ordering::SeqCst);
        let requests = self.requests.lock().unwrap().len();
        assert_eq!(approvals, 2);
        assert_eq!(requests, 3);
        assert!(suspension.is_none());
        assert_eq!(saved.custom[RECOVERY]["in_progress"], true);
        assert_eq!(
            saved
                .messages
                .iter()
                .filter(
                    |message| matches!(message, Msg::Tool { tool_call_id, is_error:false, .. }
            if tool_call_id == "reused-write")
                )
                .count(),
            1
        );
        assert!(
            matches!(saved.messages.last(), Some(Msg::Assistant { tool_calls, .. })
            if tool_calls.len() == 2 && tool_calls[0].id == "reused-write" && tool_calls[1].id == "never-started")
        );
        std::fs::write(
            self.directory.join("recovery-evidence.json"),
            serde_json::to_vec(
                &json!({"thread":thread,"facts":{"sequence_before_exit":sequence,
                "requests":requests,"approvals":approvals,"second_effect_returned":false}}),
            )
            .unwrap(),
        )
        .unwrap();
        std::process::exit(47);
    }
}

async fn reused_effect_exit(directory: &Path) {
    let server = MockServer::start().await;
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let captured = requests.clone();
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(move |request: &wiremock::Request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            let mut requests = captured.lock().unwrap();
            let ordinal = requests.len();
            requests.push(body);
            match ordinal {
                0 => response(vec![call(
                    "reused-write",
                    "write_file",
                    json!({"file_path":"first.txt","content":"first actual effect"}),
                )]),
                1 => text_response("First approved effect completed."),
                2 => response(vec![
                    call(
                        "reused-write",
                        "write_file",
                        json!({"file_path":"second.txt","content":"second effect before return"}),
                    ),
                    call(
                        "never-started",
                        "write_file",
                        json!({"file_path":"never.txt","content":"must remain absent"}),
                    ),
                ]),
                _ => panic!("unexpected request before actual crash"),
            }
        })
        .mount(&server)
        .await;
    let cfg = config(&server, 3, false);
    let approval = Arc::new(Mutex::new(ApprovalState::default()));
    let (tx, mut events) = mpsc::unbounded_channel();
    let mut extras = UiBridge { tx }.session_extras();
    extras.cancellation_session = Some(craftsman_harness::SessionCancellation::new());
    let store_slot = Arc::new(Mutex::new(None));
    let thread_slot = Arc::new(Mutex::new(None));
    let approvals = Arc::new(AtomicUsize::new(0));
    extras.extra_tools.push(Arc::new(CrashAfterWrite {
        directory: directory.into(),
        store: store_slot.clone(),
        thread: thread_slot.clone(),
        approvals: approvals.clone(),
        requests: requests.clone(),
    }));
    let session = agent::build_session_ext(&cfg, approval.clone(), "", false, false, None, extras)
        .await
        .unwrap();
    *store_slot.lock().unwrap() = session.checkpointer.clone();
    *thread_slot.lock().unwrap() = Some(session.thread_id.clone());
    let mut app = draft(&cfg);
    // Resolve real app approvals. No tool or model is replayed between turns.
    for input in [
        "Perform the first approved physical effect.",
        "Perform the second effect; retain β and the earlier result.",
    ] {
        let agent = session.agent.clone();
        let mut task = tokio::spawn(async move { agent.run_turn(input).await });
        app.running = true;
        let result = loop {
            tokio::select! {
                result = &mut task => break result.unwrap(),
                event = events.recv() => {
                    handle_ui_event(&mut app, event.unwrap(), Some(&session));
                    if matches!(app.modal, Some(Modal::Approval(_))) {
                        assert!(!approval.lock().unwrap().auto_approve);
                        let Some(Modal::Approval(modal)) = app.modal.take() else { unreachable!() };
                        approvals.fetch_add(1, Ordering::SeqCst);
                        modal.response.send(ApprovalChoice::Approve).unwrap();
                    }
                    assert_draft(&app);
                }
            }
        };
        assert!(matches!(&result, Ok(TurnResult::Done(_))));
        finish_ui(&session, &mut app, &mut events, result);
    }
    panic!("second actual write must exit before returning its result");
}

async fn cancelled_save_failure(directory: &Path) {
    let server = MockServer::start().await;
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let captured = requests.clone();
    Mock::given(method("POST")).and(path("/v1/responses"))
        .respond_with(move |request: &wiremock::Request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            let mut requests = captured.lock().unwrap();
            let ordinal = requests.len();
            requests.push(body.clone());
            match ordinal {
                0 => response(vec![
                    call("saved-write", "write_file", json!({"file_path":"effect.txt","content":"effect survived cancelled final save"})),
                    call("failed-save-question", "ask_user", json!({"question":"May this turn continue?"})),
                    call("unsampled-sibling", "write_file", json!({"file_path":"forbidden.txt","content":"must remain absent"})),
                ]),
                1 => {
                    assert!(has_string(&body["input"], CANCEL_STEERING));
                    text_response("Explicit continuation preserved the unsaved accepted originals.")
                }
                _ => panic!("failed checkpoint must not cause extra sampling or effect replay"),
            }
        }).expect(2).mount(&server).await;
    let cfg = config(&server, 1, true);
    let (tx, mut events) = mpsc::unbounded_channel();
    let mut slot = AgentSlot::spawn(
        &cfg,
        Arc::new(Mutex::new(ApprovalState::default())),
        UiBridge { tx },
        craftsman_harness::SessionCancellation::new(),
    );
    let session = slot.get().await.as_ref().unwrap();
    let mut app = draft(&cfg);
    let (task, approvals) = until_question(session, &mut app, &mut events, INITIAL).await;
    assert_eq!(approvals, 1);
    let store = session.checkpointer.as_ref().unwrap().clone();
    let thread = session.thread_id.clone();
    for _ in 0..2 {
        assert!(session.agent.steer(CANCEL_STEERING));
    }
    store.connection().execute(r#"CREATE TRIGGER reject_cancel_final BEFORE INSERT ON checkpoints WHEN instr(NEW.state_json, '"in_progress":false') > 0 BEGIN SELECT RAISE(ABORT, 'private cancelled final save failure'); END"#, ()).await.unwrap();
    cancel_question(session, &mut app);
    let result = returned(task).await;
    assert!(
        matches!(&result, Err(AgentError::State(text)) if text.contains("checkpoint could not be saved")
        && !text.contains("private cancelled final save failure"))
    );
    assert!(session.agent.checkpoint_failed());
    finish_ui(session, &mut app, &mut events, result);
    assert_eq!(app.current_program_status(false), Status::Error);
    assert!(
        app.transcript
            .iter()
            .any(|item| item.kind == TranscriptKind::Error
                && item.text.contains("checkpoint could not be saved"))
    );
    assert!(
        app.transcript
            .iter()
            .all(|item| !item.text.starts_with("Paused after ")
                && !item.text.contains("Session saved")
                && !item.text.contains("private cancelled final save failure"))
    );
    let live = session.agent.state.lock().unwrap().messages.clone();
    assert_group_results(&live);
    assert_eq!(
        live.iter()
            .filter(|message| **message == Msg::user(CANCEL_STEERING))
            .count(),
        2
    );
    let (failed_sequence, prior, _) = store.load_latest(&thread).await.unwrap().unwrap();
    assert_eq!(prior.custom[RECOVERY]["in_progress"], true);
    assert!(prior.messages.iter().any(
        |message| matches!(message, Msg::Tool { tool_call_id, is_error:false, .. }
        if tool_call_id == "saved-write")
    ));
    assert_eq!(
        prior
            .messages
            .iter()
            .filter(|message| **message == Msg::user(CANCEL_STEERING))
            .count(),
        0,
        "a refused final save does not make accepted memory durable"
    );
    let model = Arc::new(craftsman_harness::testing::ScriptedModel::new(vec![]));
    let fresh = Agent::builder(model.clone())
        .checkpointer(store.clone())
        .durable_turn_checkpoints(true)
        .build();
    assert!(fresh.load_thread(&thread).await.unwrap());
    assert_eq!(fresh.recovery_info().unwrap().unresolved_tool_calls, 1);
    assert!(fresh.state.lock().unwrap().messages.iter().any(|message| matches!(message,
        Msg::Tool { tool_call_id, content, is_error:true } if tool_call_id == "unsampled-sibling" && content.contains("may have had effects"))));
    assert!(model.requests.lock().unwrap().is_empty());
    drop(fresh);
    let modified = std::fs::metadata(directory.join("effect.txt"))
        .unwrap()
        .modified()
        .unwrap();
    assert_eq!(requests.lock().unwrap().len(), 1);
    store.connection().execute("CREATE TRIGGER reject_recovery_readiness BEFORE INSERT ON checkpoints BEGIN SELECT RAISE(ABORT, 'private readiness failure'); END", ()).await.unwrap();
    let readiness_input = "Retain the failed-save memory and inspect before any retry.";
    assert!(
        session
            .agent
            .run_turn(readiness_input)
            .await
            .unwrap_err()
            .to_string()
            .contains("checkpoint could not be saved")
    );
    assert_eq!(
        requests.lock().unwrap().len(),
        1,
        "a refused readiness save does not sample"
    );
    assert_eq!(
        session
            .agent
            .state
            .lock()
            .unwrap()
            .messages
            .iter()
            .filter(|message| **message == Msg::user(CANCEL_STEERING))
            .count(),
        2
    );
    store
        .connection()
        .execute("DROP TRIGGER reject_recovery_readiness", ())
        .await
        .unwrap();
    store
        .connection()
        .execute("DROP TRIGGER reject_cancel_final", ())
        .await
        .unwrap();
    let result = session
        .agent
        .run_turn("Explicitly continue with the retained memory; do not repeat the write.")
        .await
        .unwrap();
    assert!(
        matches!(result, TurnResult::Done(output) if output.final_text() == "Explicit continuation preserved the unsaved accepted originals.")
    );
    assert!(!session.agent.checkpoint_failed());
    let (repaired_sequence, repaired, _) = store.load_latest(&thread).await.unwrap().unwrap();
    assert!(repaired_sequence > failed_sequence);
    assert_eq!(repaired.custom[RECOVERY]["in_progress"], false);
    assert_eq!(
        repaired.messages,
        session.agent.state.lock().unwrap().messages
    );
    assert_eq!(
        repaired
            .messages
            .iter()
            .filter(|message| **message == Msg::user(CANCEL_STEERING))
            .count(),
        2
    );
    assert_eq!(
        repaired
            .messages
            .iter()
            .filter(|message| **message == Msg::user(readiness_input))
            .count(),
        1
    );
    assert_eq!(
        std::fs::read(directory.join("effect.txt")).unwrap(),
        b"effect survived cancelled final save"
    );
    assert_eq!(
        std::fs::metadata(directory.join("effect.txt"))
            .unwrap()
            .modified()
            .unwrap(),
        modified
    );
    assert!(!directory.join("forbidden.txt").exists());
    assert_eq!(*session.usage.lock().unwrap(), (6, 4));
    write_evidence(
        directory,
        session,
        json!({"requests":requests.lock().unwrap().len(),"approvals":approvals,"observed_save_failure":true,
        "failed_sequence":failed_sequence,"repaired_sequence":repaired_sequence}),
    );
    slot.shutdown().await;
    assert!(!slot.peek().unwrap().agent.checkpoint_failed());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "owned recovery fixture; unmarked/unknown/duplicate startup returns before path/config access"]
async fn owned_recovery_reliability_fixture() {
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
            "pause-transfer-cancel" | "reused-effect-exit" | "cancelled-save-failure"
        )
    {
        return;
    }
    let directory = std::env::current_dir().unwrap();
    match selected[0].as_str() {
        "pause-transfer-cancel" => pause_transfer_cancel(&directory).await,
        "reused-effect-exit" => reused_effect_exit(&directory).await,
        "cancelled-save-failure" => cancelled_save_failure(&directory).await,
        _ => unreachable!(),
    }
    eprintln!("{DONE}");
}

async fn run_child(markers: &[&str], expected_exit: i32) -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    let workspace = directory.path().join("workspace");
    let config = directory.path().join("config");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::create_dir(&config).unwrap();
    let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
    command.args([
        "--ignored",
        "--exact",
        HELPER,
        "--test-threads=1",
        "--nocapture",
    ]);
    for marker in markers {
        command.arg("--skip").arg(format!("{MARKER}{marker}"));
    }
    command
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
    let outcome = tokio::time::timeout(Duration::from_secs(20), async {
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
    let (status, out, err) = match outcome {
        Ok(Ok(result)) => result,
        _ => {
            let _ = tokio::time::timeout(Duration::from_secs(5), child.kill()).await;
            panic!("owned recovery fixture exceeded its execution/cleanup bound");
        }
    };
    assert!(out.len() <= 65536 && err.len() <= 65536);
    let error = std::str::from_utf8(&err).unwrap();
    assert_eq!(status.code(), Some(expected_exit), "{error}");
    if expected_exit == 0 {
        assert!(
            std::str::from_utf8(&out)
                .unwrap()
                .contains("1 passed; 0 failed"),
            "fixture actually selected and ran"
        );
    }
    if markers.len() == 1
        && matches!(
            markers[0],
            "pause-transfer-cancel" | "cancelled-save-failure"
        )
    {
        assert_eq!(error.lines().filter(|line| *line == DONE).count(), 1);
    } else {
        assert!(!error.lines().any(|line| line == DONE));
    }
    directory
}

async fn reopen(directory: &Path) -> (Agent, Arc<crate::checkpoints::LibsqlCheckpointer>, Value) {
    let workspace = directory.join("workspace");
    let info: Value =
        serde_json::from_slice(&std::fs::read(workspace.join("recovery-evidence.json")).unwrap())
            .unwrap();
    let store = Arc::new(
        crate::checkpoints::LibsqlCheckpointer::open_at_for_workspace(
            directory.join("config/threads.db"),
            workspace.canonicalize().unwrap().to_string_lossy().into(),
        )
        .await
        .unwrap(),
    );
    let model = Arc::new(craftsman_harness::testing::ScriptedModel::new(vec![]));
    let agent = Agent::builder(model.clone())
        .checkpointer(store.clone())
        .durable_turn_checkpoints(true)
        .build();
    assert!(
        agent
            .load_thread(info["thread"].as_str().unwrap())
            .await
            .unwrap()
    );
    assert!(
        model.requests.lock().unwrap().is_empty(),
        "fresh owner reopens without sampling or dispatch"
    );
    (agent, store, info)
}

#[tokio::test]
async fn saved_pause_steering_survives_cancelled_compaction_model_rebuild_and_cancel_reopen() {
    let directory = run_child(&["pause-transfer-cancel"], 0).await;
    let (fresh, store, info) = reopen(directory.path()).await;
    assert!(fresh.recovery_info().is_none());
    let (sequence, saved, _) = store
        .load_latest(info["thread"].as_str().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert!(sequence > info["facts"]["cancel_sequence"].as_u64().unwrap());
    assert_eq!(saved.custom[RECOVERY]["in_progress"], false);
    assert_group_results(&saved.messages);
    for input in STEERING.into_iter().chain([CANCEL_STEERING]) {
        assert_eq!(
            saved
                .messages
                .iter()
                .filter(|message| **message == Msg::user(input))
                .count(),
            1
        );
    }
    assert_eq!(
        std::fs::read(directory.path().join("workspace/effect.txt")).unwrap(),
        b"one retained physical write"
    );
    assert!(!directory.path().join("workspace/forbidden.txt").exists());
}

#[tokio::test]
async fn abrupt_exit_after_reused_id_effect_preserves_old_result_and_marks_new_effect_unknown() {
    let directory = run_child(&["reused-effect-exit"], 47).await;
    let workspace = directory.path().join("workspace");
    assert_eq!(
        std::fs::read(workspace.join("first.txt")).unwrap(),
        b"first actual effect"
    );
    assert_eq!(
        std::fs::read(workspace.join("second.txt")).unwrap(),
        b"second effect before return"
    );
    assert!(!workspace.join("never.txt").exists());
    let modified = ["first.txt", "second.txt"].map(|name| {
        std::fs::metadata(workspace.join(name))
            .unwrap()
            .modified()
            .unwrap()
    });
    let (fresh, store, info) = reopen(directory.path()).await;
    let recovery = fresh.recovery_info().unwrap();
    assert!(recovery.unfinished_turn);
    assert_eq!(recovery.unresolved_tool_calls, 2);
    let messages = fresh.state.lock().unwrap().messages.clone();
    assert_group_results(&messages);
    let results: Vec<_> = messages.iter().filter(|message| matches!(message, Msg::Tool { tool_call_id, .. } if tool_call_id == "reused-write")).collect();
    assert_eq!(results.len(), 2);
    assert!(
        matches!(results[0], Msg::Tool { content, is_error:false, .. } if content == "first physical write returned and was committed")
    );
    assert!(
        matches!(results[1], Msg::Tool { content, is_error:true, .. } if content.contains("may have had effects"))
    );
    assert!(messages.iter().any(
        |message| matches!(message, Msg::Tool { tool_call_id, content, is_error:true }
        if tool_call_id == "never-started" && content.contains("may have had effects"))
    ));
    assert!(messages.contains(&Msg::user(
        "Perform the second effect; retain β and the earlier result."
    )));
    let latest = store
        .load_latest(info["thread"].as_str().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        latest.0,
        info["facts"]["sequence_before_exit"].as_u64().unwrap()
    );
    drop(fresh);
    let model = Arc::new(craftsman_harness::testing::ScriptedModel::new(vec![
        Msg::assistant("Inspection completed without retrying either write."),
    ]));
    let continued = Agent::builder(model.clone())
        .checkpointer(store.clone())
        .durable_turn_checkpoints(true)
        .build();
    assert!(
        continued
            .load_thread(info["thread"].as_str().unwrap())
            .await
            .unwrap()
    );
    continued
        .run_turn("Inspect the two actual files; do not replay unknown calls.")
        .await
        .unwrap();
    assert_eq!(model.requests.lock().unwrap().len(), 1);
    for (name, prior) in ["first.txt", "second.txt"].into_iter().zip(modified) {
        assert_eq!(
            std::fs::metadata(workspace.join(name))
                .unwrap()
                .modified()
                .unwrap(),
            prior
        );
    }
    assert!(!workspace.join("never.txt").exists());
    let saved = store
        .load_latest(info["thread"].as_str().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert!(saved.0 > latest.0);
    assert_eq!(saved.1.custom[RECOVERY]["in_progress"], false);
    assert_group_results(&saved.1.messages);
}

#[tokio::test]
async fn cancelled_final_sql_failure_reports_error_retains_memory_and_refuses_unready_sampling() {
    let directory = run_child(&["cancelled-save-failure"], 0).await;
    let (fresh, store, info) = reopen(directory.path()).await;
    assert!(fresh.recovery_info().is_none());
    assert_eq!(info["facts"]["observed_save_failure"], true);
    let (_, saved, _) = store
        .load_latest(info["thread"].as_str().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.custom[RECOVERY]["in_progress"], false);
    assert_eq!(
        saved
            .messages
            .iter()
            .filter(|message| **message == Msg::user(CANCEL_STEERING))
            .count(),
        2
    );
    assert_eq!(
        std::fs::read(directory.path().join("workspace/effect.txt")).unwrap(),
        b"effect survived cancelled final save"
    );
    assert!(!directory.path().join("workspace/forbidden.txt").exists());
}

#[tokio::test]
async fn recovery_fixture_rejects_unmarked_unknown_and_duplicate_selection_before_storage() {
    for markers in [
        vec![],
        vec!["unknown"],
        vec!["pause-transfer-cancel", "pause-transfer-cancel"],
    ] {
        let directory = run_child(&markers, 0).await;
        assert!(!directory.path().join("config/threads.db").exists());
        assert_eq!(
            std::fs::read_dir(directory.path().join("workspace"))
                .unwrap()
                .count(),
            0
        );
    }
}

// Separate marked fixture: the earlier Responses/crash helpers remain unchanged.
const SIGNED_HELPER: &str = "tui::session_bridge::reliability_tests::owned_signed_recovery_fixture";
const SIGNED_MARKER: &str = "craftsman-signed-recovery=";
const SIGNED_DONE: &str = "actual owned signed recovery assertions completed";
const NATIVE_HISTORY: &str = "_craftsman_native_model_history";
const SIGNED_INITIAL: &str = "Create signed-effect.txt once, then ask before more work.";
const SIGNED_NO_ANSWER: &str =
    "The user did not provide an answer. Proceed with your best assumption.";
const SIGNED_INPUTS: [&str; 2] = [
    "Retain this accepted α input and the completed physical write.",
    "保留已接受的原文。\nContinue only after inspecting the saved effects.",
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum SignedProtocol {
    Claude,
    Gemini,
}

impl SignedProtocol {
    fn name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Gemini => "gemini",
        }
    }
    fn messages(self, body: &Value) -> &Value {
        &body[match self {
            Self::Claude => "messages",
            Self::Gemini => "contents",
        }]
    }
    fn output_tokens(self) -> u64 {
        if self == Self::Gemini { 4 } else { 2 }
    }
}

fn signed_write_args() -> Value {
    json!({"file_path":"signed-effect.txt","content":"one original signed-history effect"})
}

fn signed_original_items(protocol: SignedProtocol) -> Vec<Value> {
    let question = json!({"question":"May the signed-history task continue?"});
    let forbidden = json!({"file_path":"signed-forbidden.txt","content":"must remain absent"});
    match protocol {
        SignedProtocol::Claude => vec![
            json!({"type":"thinking","thinking":"owned fixture reasoning α","signature":"claude-owned-signature=="}),
            json!({"type":"redacted_thinking","data":"owned opaque α\n=="}),
            json!({"type":"tool_use","id":"native-signed-write","name":"write_file","input":signed_write_args()}),
            json!({"type":"tool_use","id":"native-signed-question","name":"ask_user","input":question}),
            json!({"type":"tool_use","id":"native-signed-never","name":"write_file","input":forbidden}),
        ],
        SignedProtocol::Gemini => vec![
            json!({"thought":true,"text":"owned fixture reasoning α","thoughtSignature":"gemini-owned-thought=="}),
            // No API id is returned for the actual write. Its framework id is
            // retained portably, but must never be invented in native responses.
            json!({"functionCall":{"name":"write_file","args":signed_write_args()},"thoughtSignature":"gemini-owned-write=="}),
            json!({"functionCall":{"id":"native-signed-question","name":"ask_user","args":question},"thoughtSignature":"gemini-owned-question=="}),
            json!({"functionCall":{"id":"native-signed-never","name":"write_file","args":forbidden}}),
        ],
    }
}

fn signed_response(protocol: SignedProtocol, items: Vec<Value>) -> ResponseTemplate {
    let raw = match protocol {
        SignedProtocol::Claude => {
            let has_tools = items.iter().any(|part| part["type"] == "tool_use");
            let mut events =
                vec![json!({"type":"message_start","message":{"usage":{"input_tokens":3}}})];
            for (index, part) in items.into_iter().enumerate() {
                events
                    .push(json!({"type":"content_block_start","index":index,"content_block":part}));
                events.push(json!({"type":"content_block_stop","index":index}));
            }
            events.push(json!({"type":"message_delta","delta":{"stop_reason":if has_tools {"tool_use"} else {"end_turn"}},"usage":{"output_tokens":2}}));
            events.push(json!({"type":"message_stop"}));
            events
                .iter()
                .map(|event| format!("data: {event}\n\n"))
                .collect()
        }
        SignedProtocol::Gemini => format!(
            "data: {}\n\n",
            json!({"candidates":[{"content":{"parts":items},"finishReason":"STOP"}],
                "usageMetadata":{"promptTokenCount":3,"candidatesTokenCount":2,"thoughtsTokenCount":2}})
        ),
    };
    ResponseTemplate::new(200).set_body_raw(raw, "text/event-stream")
}

fn signed_text(protocol: SignedProtocol, text: &str) -> ResponseTemplate {
    signed_response(
        protocol,
        vec![match protocol {
            SignedProtocol::Claude => json!({"type":"text","text":text}),
            SignedProtocol::Gemini => json!({"text":text}),
        }],
    )
}

fn signed_config(server: &MockServer, protocol: SignedProtocol) -> Config {
    serde_json::from_value(json!({"model":"fixture/signed-fixture","providers":{"fixture":{
        "protocol":if protocol == SignedProtocol::Claude {"anthropic_messages"} else {"gemini_generate_content"},
        "base_url":format!("{}/signed",server.uri()),"api_key":"isolated-signed-key",
        "models":{"signed-fixture":{"settings":{"reasoning":null}}}}},
        "context":{"enabled":true,"max_bytes":32768},"terminal":{"max_rounds":2},
        "harness_profile":"general","permission_mode":"manual","mcp_servers":[],"lsp_servers":{},
        "voice":{"enabled":false},"gadgets":{"enabled":false}})).unwrap()
}

fn assert_signed_record(state: &craftsman_harness::AgentState, protocol: SignedProtocol) {
    let expected = Value::Array(signed_original_items(protocol));
    let connections = state.custom[NATIVE_HISTORY]["connections"]
        .as_object()
        .unwrap();
    assert_eq!(
        connections.len(),
        1,
        "only the original configured connection was sampled"
    );
    let records = connections.values().next().unwrap().as_array().unwrap();
    let record = records
        .iter()
        .find(|record| record["items"] == expected)
        .expect("original signed/opaque items remain exact in the real checkpoint");
    let index = record["index"].as_u64().unwrap() as usize;
    let message = serde_json::to_value(&state.messages[index]).unwrap();
    assert_eq!(record["message"], message);
}

fn assert_signed_request(protocol: SignedProtocol, body: &Value, write_result: &str) {
    let field = if protocol == SignedProtocol::Claude {
        "content"
    } else {
        "parts"
    };
    let role = if protocol == SignedProtocol::Claude {
        "assistant"
    } else {
        "model"
    };
    let messages = protocol.messages(body).as_array().unwrap();
    let original = Value::Array(signed_original_items(protocol));
    assert!(
        messages
            .iter()
            .any(|message| message["role"] == role && message[field] == original),
        "the original connection receives the exact native group after recovery"
    );
    for input in SIGNED_INPUTS {
        assert!(
            has_string(protocol.messages(body), input),
            "actual portable accepted input reaches the provider"
        );
    }
    let parts: Vec<_> = messages
        .iter()
        .filter(|message| message["role"] == "user")
        .flat_map(|message| message[field].as_array().unwrap())
        .collect();
    if protocol == SignedProtocol::Claude {
        assert!(parts.iter().any(|part| part["type"] == "tool_result"
            && part["tool_use_id"] == "native-signed-write"
            && part["content"] == write_result
            && part["is_error"] == false));
        assert!(parts.iter().any(|part| part["type"] == "tool_result"
            && part["tool_use_id"] == "native-signed-question"
            && part["content"] == SIGNED_NO_ANSWER
            && part["is_error"] == false));
        assert!(parts.iter().any(|part| part["type"] == "tool_result"
            && part["tool_use_id"] == "native-signed-never"
            && part["is_error"] == true));
    } else {
        let results: Vec<_> = parts
            .iter()
            .filter_map(|part| part.get("functionResponse"))
            .collect();
        assert!(
            results.iter().any(|result| result["name"] == "write_file"
                && result.get("id").is_none()
                && result["response"]["result"] == write_result),
            "a generated framework id is never echoed for the id-less original native write"
        );
        assert!(
            results
                .iter()
                .any(|result| result["id"] == "native-signed-question"
                    && result["response"]["result"] == SIGNED_NO_ANSWER)
        );
        assert!(
            results
                .iter()
                .any(|result| result["id"] == "native-signed-never"
                    && result["response"].get("error").is_some())
        );
        assert!(
            !protocol
                .messages(body)
                .to_string()
                .contains("craftsman-gemini-")
        );
    }
}

async fn signed_recovery_case(directory: &Path, protocol: SignedProtocol) {
    let server = MockServer::start().await;
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let write_result = Arc::new(Mutex::new(None::<String>));
    let captured = requests.clone();
    let completed_write = write_result.clone();
    let recovery_sample = usize::from(protocol == SignedProtocol::Gemini);
    Mock::given(method("POST"))
        .and(path(if protocol == SignedProtocol::Claude {
            "/signed/messages"
        } else {
            "/signed/models/signed-fixture:streamGenerateContent"
        }))
        .and(header(if protocol == SignedProtocol::Claude { "x-api-key" } else { "x-goog-api-key" }, "isolated-signed-key"))
        .respond_with(move |request: &wiremock::Request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            let ordinal = {
                let mut requests = captured.lock().unwrap();
                let ordinal = requests.len();
                requests.push(body.clone());
                ordinal
            };
            if protocol == SignedProtocol::Claude {
                assert_eq!(body["model"], "signed-fixture");
            } else {
                assert_eq!(request.url.query(), Some("alt=sse"));
            }
            if ordinal < PRIOR_TURNS {
                return signed_text(protocol, &format!("Completed signed-history fixture turn {ordinal}."));
            }
            let phase = ordinal - PRIOR_TURNS;
            if phase == 0 {
                return signed_response(protocol, signed_original_items(protocol));
            }
            assert_signed_request(protocol, &body, completed_write.lock().unwrap().as_ref().unwrap());
            if protocol == SignedProtocol::Gemini && phase == 1 {
                return signed_text(protocol, "The restored store saved the retained original input.");
            }
            match phase - recovery_sample {
                1 => signed_response(protocol, vec![match protocol {
                    SignedProtocol::Claude => json!({"type":"tool_use","id":"native-restored-read","name":"read_file","input":{"file_path":"signed-effect.txt"}}),
                    SignedProtocol::Gemini => json!({"functionCall":{"name":"read_file","args":{"file_path":"signed-effect.txt"}}}),
                }]),
                2 => {
                    assert!(protocol.messages(&body).to_string().contains("one original signed-history effect"));
                    if protocol == SignedProtocol::Gemini {
                        assert!(body["contents"].as_array().unwrap().iter()
                            .flat_map(|message| message["parts"].as_array().unwrap())
                            .filter_map(|part| part.get("functionResponse"))
                            .any(|result| result["name"] == "read_file" && result.get("id").is_none()));
                    }
                    signed_text(protocol, "Fresh owner inspected the original effect without replay.")
                }
                _ => panic!("unexpected signed recovery request or replay"),
            }
        })
        .expect((PRIOR_TURNS + 3 + recovery_sample) as u64)
        .mount(&server).await;
    let cfg = signed_config(&server, protocol);
    let approval = Arc::new(Mutex::new(ApprovalState::default()));
    let (tx, mut events) = mpsc::unbounded_channel();
    let mut slot = AgentSlot::spawn(
        &cfg,
        approval.clone(),
        UiBridge { tx },
        craftsman_harness::SessionCancellation::new(),
    );
    let retired_owner = slot.cancellation();
    let session = slot.get().await.as_ref().unwrap();
    for index in 0..PRIOR_TURNS {
        let result = session
            .agent
            .run_turn(format!(
                "Record original signed-history fixture turn {index}."
            ))
            .await
            .unwrap();
        assert!(matches!(result, TurnResult::Done(output)
            if output.final_text() == format!("Completed signed-history fixture turn {index}.")));
    }
    let store = session.checkpointer.as_ref().unwrap().clone();
    assert_eq!(store.list_threads().await.unwrap()[0].checkpoints, 50);
    let thread = session.thread_id.clone();
    let mut app = draft(&cfg);
    let (task, approvals) = until_question(session, &mut app, &mut events, SIGNED_INITIAL).await;
    assert_eq!(approvals, 1);
    let state = session.agent.state.lock().unwrap().clone();
    let calls = state
        .messages
        .iter()
        .find_map(|message| match message {
            Msg::Assistant { tool_calls, .. } if tool_calls.len() == 3 => Some(tool_calls.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(calls[0].name, "write_file");
    assert_eq!(calls[1].id, "native-signed-question");
    assert_eq!(calls[2].id, "native-signed-never");
    if protocol == SignedProtocol::Gemini {
        assert!(calls[0].id.starts_with("craftsman-gemini-"));
    } else {
        assert_eq!(calls[0].id, "native-signed-write");
    }
    let completed = state
        .messages
        .iter()
        .find_map(|message| match message {
            Msg::Tool {
                tool_call_id,
                content,
                is_error: false,
            } if tool_call_id == &calls[0].id => Some(content.clone()),
            _ => None,
        })
        .unwrap();
    *write_result.lock().unwrap() = Some(completed.clone());
    let modified = std::fs::metadata(directory.join("signed-effect.txt"))
        .unwrap()
        .modified()
        .unwrap();
    for input in SIGNED_INPUTS {
        assert!(session.agent.steer(input));
    }
    if protocol == SignedProtocol::Gemini {
        store.connection().execute(r#"CREATE TRIGGER reject_gemini_signed_final BEFORE INSERT ON checkpoints WHEN instr(NEW.state_json, '"in_progress":false') > 0 BEGIN SELECT RAISE(ABORT, 'private signed final failure'); END"#, ()).await.unwrap();
    }
    cancel_question(session, &mut app);
    assert!(!session.agent.steer("late signed input is not accepted"));
    let result = returned(task).await;
    if protocol == SignedProtocol::Gemini {
        assert!(
            matches!(&result, Err(AgentError::State(text)) if text.contains("checkpoint could not be saved")
            && !text.contains("private signed final failure"))
        );
        assert!(session.agent.checkpoint_failed());
    } else {
        assert!(
            matches!(&result, Ok(TurnResult::Done(output)) if output.final_text() == "Stopped by the user.")
        );
        assert!(!session.agent.checkpoint_failed());
    }
    finish_ui(session, &mut app, &mut events, result);
    assert_eq!(
        app.current_program_status(false),
        if protocol == SignedProtocol::Gemini {
            Status::Error
        } else {
            Status::Idle
        }
    );
    let live = session.agent.state.lock().unwrap().clone();
    assert_group_results(&live.messages);
    assert_signed_record(&live, protocol);
    assert!(live.messages.iter().any(|message| matches!(message,
        Msg::Tool { tool_call_id, content, is_error:false }
        if tool_call_id == "native-signed-question" && content == SIGNED_NO_ANSWER)));
    assert!(live.messages.iter().any(|message| matches!(message,
        Msg::Tool { tool_call_id, content, is_error:true }
        if tool_call_id == "native-signed-never" && content.contains("cancelled"))));
    let accepted: Vec<_> = live
        .messages
        .iter()
        .filter_map(|message| match message {
            Msg::User { content } if SIGNED_INPUTS.contains(&content.as_str()) => {
                Some(content.as_str())
            }
            _ => None,
        })
        .collect();
    assert_eq!(accepted, SIGNED_INPUTS);
    let (cancel_sequence, saved, _) = store.load_latest(&thread).await.unwrap().unwrap();
    assert_signed_record(&saved, protocol);
    if protocol == SignedProtocol::Gemini {
        assert_eq!(saved.custom[RECOVERY]["in_progress"], true);
        assert!(
            saved
                .messages
                .iter()
                .all(|message| !matches!(message, Msg::User { content }
            if SIGNED_INPUTS.contains(&content.as_str())))
        );
        assert!(
            app.transcript
                .iter()
                .all(|item| !item.text.contains("Session saved")
                    && !item.text.starts_with("Paused after "))
        );
        let model = Arc::new(craftsman_harness::testing::ScriptedModel::new(vec![]));
        let reader = Agent::builder(model.clone())
            .checkpointer(store.clone())
            .durable_turn_checkpoints(true)
            .build();
        assert!(reader.load_thread(&thread).await.unwrap());
        assert_eq!(reader.recovery_info().unwrap().unresolved_tool_calls, 1);
        assert!(reader.state.lock().unwrap().messages.iter().any(|message| matches!(message,
            Msg::Tool { tool_call_id, content, is_error:true } if tool_call_id == "native-signed-never" && content.contains("may have had effects"))));
        assert!(model.requests.lock().unwrap().is_empty());
        drop(reader);
        assert_eq!(requests.lock().unwrap().len(), PRIOR_TURNS + 1);
        store
            .connection()
            .execute("DROP TRIGGER reject_gemini_signed_final", ())
            .await
            .unwrap();
        let result = session.agent.run_turn("Save the retained original input after restoring local storage, without repeating the effect.").await.unwrap();
        assert!(
            matches!(result, TurnResult::Done(output) if output.final_text() == "The restored store saved the retained original input.")
        );
        assert!(!session.agent.checkpoint_failed());
    } else {
        assert_eq!(saved.custom[RECOVERY]["in_progress"], false);
        assert_eq!(saved.messages, live.messages);
    }
    let (durable_sequence, durable, _) = store.load_latest(&thread).await.unwrap().unwrap();
    assert_eq!(durable.custom[RECOVERY]["in_progress"], false);
    for input in SIGNED_INPUTS {
        assert_eq!(
            durable
                .messages
                .iter()
                .filter(|message| **message == Msg::user(input))
                .count(),
            1
        );
    }
    let prior_requests = requests.lock().unwrap().len();
    assert_eq!(
        *session.usage.lock().unwrap(),
        (
            (PRIOR_TURNS as u64 + 1 + recovery_sample as u64) * 3,
            (PRIOR_TURNS as u64 + 1 + recovery_sample as u64) * protocol.output_tokens()
        )
    );
    slot.shutdown().await;
    assert!(retired_owner.is_cancelled());

    // Construct a distinct real owner on the exact original connection. Loading
    // does no model work, and subsequent native continuation uses the saved cache.
    let (tx, _events) = mpsc::unbounded_channel();
    let mut replacement = AgentSlot::spawn(
        &cfg,
        approval.clone(),
        UiBridge { tx },
        craftsman_harness::SessionCancellation::new(),
    );
    let _ = replacement.get().await;
    assert!(
        replacement
            .peek_mut()
            .unwrap()
            .load_thread(&thread)
            .await
            .unwrap()
    );
    let session = replacement.peek().unwrap();
    assert!(session.agent.recovery_info().is_none());
    assert_eq!(
        session.agent.state.lock().unwrap().messages,
        durable.messages
    );
    assert_signed_record(&session.agent.state.lock().unwrap(), protocol);
    assert_eq!(requests.lock().unwrap().len(), prior_requests);
    assert_eq!(
        *session.usage.lock().unwrap(),
        (0, 0),
        "new owner does not fabricate recovered aggregate usage"
    );
    assert_eq!(
        store.load_latest(&thread).await.unwrap().unwrap().0,
        durable_sequence
    );
    let result = session
        .agent
        .run_turn("Inspect the original signed-history effect without repeating its write.")
        .await
        .unwrap();
    assert!(
        matches!(result, TurnResult::Done(output) if output.final_text() == "Fresh owner inspected the original effect without replay.")
    );
    assert_eq!(
        *session.usage.lock().unwrap(),
        (6, protocol.output_tokens() * 2)
    );
    let (final_sequence, final_state, _) = store.load_latest(&thread).await.unwrap().unwrap();
    assert!(final_sequence > durable_sequence);
    assert_eq!(final_state.custom[RECOVERY]["in_progress"], false);
    assert_signed_record(&final_state, protocol);
    assert_group_results(&final_state.messages);
    assert_eq!(store.list_threads().await.unwrap()[0].checkpoints, 50);
    assert!(!approval.lock().unwrap().auto_approve);
    assert_eq!(
        std::fs::read(directory.join("signed-effect.txt")).unwrap(),
        b"one original signed-history effect"
    );
    assert_eq!(
        std::fs::metadata(directory.join("signed-effect.txt"))
            .unwrap()
            .modified()
            .unwrap(),
        modified
    );
    assert!(!directory.join("signed-forbidden.txt").exists());
    std::fs::write(directory.join("signed-recovery-evidence.json"), serde_json::to_vec(&json!({
        "thread":thread,"protocol":protocol.name(),"cancel_sequence":cancel_sequence,
        "durable_sequence":durable_sequence,"final_sequence":final_sequence,
        "requests":requests.lock().unwrap().len(),"observed_final_save_failure":protocol == SignedProtocol::Gemini})).unwrap()).unwrap();
    replacement.shutdown().await;
    assert!(!replacement.peek().unwrap().agent.checkpoint_failed());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "owned signed recovery fixture; explicit unique protocol marker required before path/config access"]
async fn owned_signed_recovery_fixture() {
    let selected: Vec<_> = std::env::args_os()
        .filter_map(|arg| {
            arg.to_str()
                .and_then(|arg| arg.strip_prefix(SIGNED_MARKER))
                .map(str::to_owned)
        })
        .collect();
    if selected.len() != 1 || !matches!(selected[0].as_str(), "claude" | "gemini") {
        return;
    }
    let protocol = if selected[0] == "claude" {
        SignedProtocol::Claude
    } else {
        SignedProtocol::Gemini
    };
    let directory = std::env::current_dir().unwrap();
    signed_recovery_case(&directory, protocol).await;
    eprintln!("{SIGNED_DONE}");
}

async fn run_signed_child(markers: &[&str]) -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    let workspace = directory.path().join("workspace");
    let config = directory.path().join("config");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::create_dir(&config).unwrap();
    let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
    command.args([
        "--ignored",
        "--exact",
        SIGNED_HELPER,
        "--test-threads=1",
        "--nocapture",
    ]);
    for marker in markers {
        command
            .arg("--skip")
            .arg(format!("{SIGNED_MARKER}{marker}"));
    }
    command
        .current_dir(&workspace)
        .env_clear()
        .env("HOME", directory.path())
        .env("USERPROFILE", directory.path())
        .env("CRAFTSMAN_CONFIG_DIR", &config)
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
    let outcome = tokio::time::timeout(Duration::from_secs(20), async {
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
    let (status, out, err) = match outcome {
        Ok(Ok(result)) => result,
        _ => {
            let _ = tokio::time::timeout(Duration::from_secs(5), child.kill()).await;
            panic!("owned signed recovery fixture exceeded its execution/cleanup bound");
        }
    };
    assert!(out.len() <= 65536 && err.len() <= 65536);
    assert!(status.success(), "{}", std::str::from_utf8(&err).unwrap());
    assert!(
        std::str::from_utf8(&out)
            .unwrap()
            .contains("1 passed; 0 failed")
    );
    let completions = std::str::from_utf8(&err)
        .unwrap()
        .lines()
        .filter(|line| *line == SIGNED_DONE)
        .count();
    let selected = markers.len() == 1 && matches!(markers[0], "claude" | "gemini");
    assert_eq!(completions, usize::from(selected));
    if !selected {
        assert!(!config.join("threads.db").exists());
        assert_eq!(std::fs::read_dir(&workspace).unwrap().count(), 0);
    }
    directory
}

async fn verify_signed_reopen(directory: &Path, protocol: SignedProtocol) {
    let workspace = directory.join("workspace");
    let info: Value = serde_json::from_slice(
        &std::fs::read(workspace.join("signed-recovery-evidence.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(info["protocol"], protocol.name());
    assert_eq!(
        info["requests"],
        PRIOR_TURNS + 3 + usize::from(protocol == SignedProtocol::Gemini)
    );
    let store = Arc::new(
        crate::checkpoints::LibsqlCheckpointer::open_at_for_workspace(
            directory.join("config/threads.db"),
            workspace.canonicalize().unwrap().to_string_lossy().into(),
        )
        .await
        .unwrap(),
    );
    let model = Arc::new(craftsman_harness::testing::ScriptedModel::new(vec![]));
    let fresh = Agent::builder(model.clone())
        .checkpointer(store.clone())
        .durable_turn_checkpoints(true)
        .build();
    assert!(
        fresh
            .load_thread(info["thread"].as_str().unwrap())
            .await
            .unwrap()
    );
    assert!(fresh.recovery_info().is_none());
    assert!(model.requests.lock().unwrap().is_empty());
    let (sequence, saved, _) = store
        .load_latest(info["thread"].as_str().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(sequence, info["final_sequence"].as_u64().unwrap());
    assert_eq!(saved.custom[RECOVERY]["in_progress"], false);
    assert_signed_record(&saved, protocol);
    assert_group_results(&saved.messages);
    for input in SIGNED_INPUTS {
        assert_eq!(
            saved
                .messages
                .iter()
                .filter(|message| **message == Msg::user(input))
                .count(),
            1
        );
    }
    assert_eq!(
        std::fs::read(workspace.join("signed-effect.txt")).unwrap(),
        b"one original signed-history effect"
    );
    assert!(!workspace.join("signed-forbidden.txt").exists());
}

#[tokio::test]
async fn actual_claude_signed_and_opaque_blocks_survive_long_session_cancel_and_fresh_owner() {
    let guard = run_signed_child(&[]).await;
    assert!(!guard.path().join("config/threads.db").exists());
    let unknown = run_signed_child(&["unknown"]).await;
    assert!(!unknown.path().join("config/threads.db").exists());
    let directory = run_signed_child(&["claude"]).await;
    verify_signed_reopen(directory.path(), SignedProtocol::Claude).await;
}

#[tokio::test]
async fn actual_gemini_native_ids_survive_failed_cancel_save_recovery_and_fresh_owner() {
    let guard = run_signed_child(&["gemini", "gemini"]).await;
    assert!(!guard.path().join("config/threads.db").exists());
    let directory = run_signed_child(&["gemini"]).await;
    verify_signed_reopen(directory.path(), SignedProtocol::Gemini).await;
}
