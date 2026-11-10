//! Isolated actual-terminal HTTP/file/SQLite compaction and crash recovery.
use super::*;
use craftsman_harness::Checkpointer;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const HELPER: &str = "tui::session_bridge::compaction_tests::owned_compaction_fixture";
const MARKER: &str = "craftsman-idle-compaction-v1=";
const MODE: &str = "craftsman-idle-compaction-mode=";

fn response(items: Vec<Value>) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(
        format!("data: {}\n\n", json!({"type":"response.completed","response":{"status":"completed","output":items,"usage":{"input_tokens":3,"output_tokens":2}}})),
        "text/event-stream",
    )
}

fn text_response(text: &str) -> ResponseTemplate {
    response(vec![
        json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":text}]}),
    ])
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "owned compaction fixture; unmarked startup returns before path/config access"]
async fn owned_compaction_fixture() {
    let arguments: Vec<_> = std::env::args_os().collect();
    let directories: Vec<_> = arguments
        .iter()
        .filter_map(|arg| arg.to_str().and_then(|arg| arg.strip_prefix(MARKER)))
        .collect();
    let modes: Vec<_> = arguments
        .iter()
        .filter_map(|arg| arg.to_str().and_then(|arg| arg.strip_prefix(MODE)))
        .collect();
    if directories.len() != 1 || modes.len() != 1 || !matches!(modes[0], "saved" | "interrupted") {
        return;
    }
    let directory = std::path::PathBuf::from(directories[0]);
    if !directory.is_absolute() {
        return;
    }
    let interrupted = modes[0] == "interrupted";
    let server = MockServer::start().await;
    let requests = Arc::new(AtomicUsize::new(0));
    let summaries = Arc::new(AtomicUsize::new(0));
    let summary_started = Arc::new(tokio::sync::Notify::new());
    let observed = requests.clone();
    let observed_summary = summaries.clone();
    let notify = summary_started.clone();
    Mock::given(method("POST")).and(path("/v1/responses")).and(header("authorization", "Bearer isolated-compaction-key")).respond_with(move |request: &wiremock::Request| {
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        assert_eq!(body["model"], "compaction-fixture");
        let ordinal = observed.fetch_add(1, Ordering::SeqCst);
        if body.get("tools").is_none() {
            observed_summary.fetch_add(1, Ordering::SeqCst);
            assert!(serde_json::to_string(&body).unwrap().contains("first physical write"));
            notify.notify_one();
            let response = text_response("Saved summary: one physical write completed; continue without repeating it.");
            return if interrupted { response.set_delay(Duration::from_secs(30)) } else { response };
        }
        if ordinal == 0 {
            return response(vec![json!({"type":"reasoning","id":"original-reasoning","encrypted_content":"original-native-opaque","summary":[]}),json!({"type":"function_call","call_id":"write-once","name":"write_file","arguments":json!({"file_path":"effect.txt","content":"one physical write"}).to_string()})]);
        }
        let encoded = serde_json::to_string(&body).unwrap();
        if ordinal < 4 {
            assert!(encoded.contains("original-native-opaque"));
            text_response(if ordinal == 1 { "first physical write completed" } else { "continue completed" })
        } else {
            assert!(encoded.contains("Saved summary:"));
            assert!(!encoded.contains("original-native-opaque"));
            text_response("continued from the saved summary")
        }
    }).mount(&server).await;
    let cfg: Config = serde_json::from_value(json!({"model":"test/compaction-fixture","providers":{"test":{"protocol":"openai_responses","base_url":format!("{}/v1",server.uri()),"api_key":"isolated-compaction-key","models":{"compaction-fixture":{"settings":{"reasoning":null}}}}},"harness_profile":"general"})).unwrap();
    let approval = Arc::new(Mutex::new(ApprovalState {
        auto_approve: true,
        mode: crate::config::PermissionMode::BypassPermissions,
        ..Default::default()
    }));
    let (tx, _events) = mpsc::unbounded_channel();
    let root = craftsman_harness::SessionCancellation::new();
    let mut slot = AgentSlot::spawn(&cfg, approval, UiBridge { tx }, root.clone());
    let session = slot.get().await.as_ref().unwrap();
    session
        .agent
        .run_turn("Write effect.txt once, then finish.")
        .await
        .unwrap();
    session
        .agent
        .run_turn("Confirm the first completed task.")
        .await
        .unwrap();
    session
        .agent
        .run_turn("Keep the completed task history.")
        .await
        .unwrap();
    assert_eq!(requests.load(Ordering::SeqCst), 4);
    let thread = session.thread_id.clone();
    let original = session.agent.state.lock().unwrap().messages.clone();
    assert_eq!(original.len(), 8);
    let store = session.checkpointer.as_ref().unwrap().clone();
    let initial = store.load_latest(&thread).await.unwrap().unwrap().0;
    let file_modified = std::fs::metadata(directory.join("effect.txt"))
        .unwrap()
        .modified()
        .unwrap();
    std::fs::write(
        directory.join("thread.json"),
        serde_json::to_vec(&json!({"thread":thread,"initial_sequence":initial})).unwrap(),
    )
    .unwrap();
    if interrupted {
        let agent = session.agent.clone();
        let compaction = session.compaction.clone();
        let turn = slot.cancellation().new_turn();
        let task = tokio::spawn(async move {
            turn.scope(crate::tui::commands::compact_session(agent, compaction))
                .await
        });
        tokio::time::timeout(Duration::from_secs(5), summary_started.notified())
            .await
            .unwrap();
        assert!(!task.is_finished());
        let (seq, saved, _) = store.load_latest(&thread).await.unwrap().unwrap();
        assert!(seq > initial);
        assert_eq!(saved.messages, original);
        assert_eq!(
            saved.custom["_craftsman_durable_turn_v1"]["in_progress"],
            true
        );
        assert_eq!(requests.load(Ordering::SeqCst), 5);
        std::process::exit(32);
    }
    store.connection().execute("CREATE TRIGGER reject_idle_save BEFORE INSERT ON checkpoints BEGIN SELECT RAISE(ABORT, 'private SQL failure'); END", ()).await.unwrap();
    let output =
        crate::tui::commands::compact_session(session.agent.clone(), session.compaction.clone())
            .await;
    assert!(output.contains("Further work was stopped") && !output.contains("private SQL failure"));
    assert_eq!(requests.load(Ordering::SeqCst), 4);
    assert_eq!(session.agent.state.lock().unwrap().messages, original);
    store
        .connection()
        .execute("DROP TRIGGER reject_idle_save", ())
        .await
        .unwrap();
    store.connection().execute("CREATE TRIGGER reject_idle_summary BEFORE INSERT ON checkpoints WHEN instr(NEW.state_json, '<conversation_summary>') > 0 BEGIN SELECT RAISE(ABORT, 'private final SQL failure'); END", ()).await.unwrap();
    let output =
        crate::tui::commands::compact_session(session.agent.clone(), session.compaction.clone())
            .await;
    assert!(output.contains("Further work was stopped") && !output.contains("messages (saved)"));
    assert!(session.agent.checkpoint_failed());
    assert_eq!(requests.load(Ordering::SeqCst), 5);
    assert_eq!(
        store
            .load_latest(&thread)
            .await
            .unwrap()
            .unwrap()
            .1
            .messages,
        original
    );
    store
        .connection()
        .execute("DROP TRIGGER reject_idle_summary", ())
        .await
        .unwrap();
    assert!(session.agent.load_thread(&thread).await.unwrap());
    assert_eq!(requests.load(Ordering::SeqCst), 5);
    let output =
        crate::tui::commands::compact_session(session.agent.clone(), session.compaction.clone())
            .await;
    assert_eq!(output, "compacted conversation: 8 -> 5 messages (saved)");
    assert!(!session.agent.checkpoint_failed());
    let (seq, saved, _) = store.load_latest(&thread).await.unwrap().unwrap();
    assert!(seq > initial);
    assert_eq!(
        saved.custom["_craftsman_durable_turn_v1"]["in_progress"],
        false
    );
    assert_eq!(saved.messages, session.agent.state.lock().unwrap().messages);
    assert!(session.agent.load_thread(&thread).await.unwrap());
    assert!(session.agent.recovery_info().is_none());
    session
        .agent
        .run_turn("Continue using the saved summary.")
        .await
        .unwrap();
    assert_eq!(requests.load(Ordering::SeqCst), 7);
    assert_eq!(summaries.load(Ordering::SeqCst), 2);
    assert_eq!(
        std::fs::read(directory.join("effect.txt")).unwrap(),
        b"one physical write"
    );
    assert_eq!(
        std::fs::metadata(directory.join("effect.txt"))
            .unwrap()
            .modified()
            .unwrap(),
        file_modified
    );
    // Save has completed, but this process still owns the actual session.
    // Exit without destructors; another process must reopen it without work.
    std::process::exit(31);
}

async fn run_fixture(mode: &str, exit: i32) {
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
        .arg("--skip")
        .arg(format!("{MODE}{mode}"))
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
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let outcome = tokio::time::timeout(Duration::from_secs(30), async {
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
            panic!("owned compaction fixture exceeded its execution/cleanup bound");
        }
    };
    assert!(out.len() <= 65536 && err.len() <= 65536);
    assert_eq!(
        status.code(),
        Some(exit),
        "{}",
        String::from_utf8_lossy(&err)
    );
    assert_eq!(
        std::fs::read(directory.path().join("effect.txt")).unwrap(),
        b"one physical write"
    );
    let info: Value =
        serde_json::from_slice(&std::fs::read(directory.path().join("thread.json")).unwrap())
            .unwrap();
    let thread = info["thread"].as_str().unwrap();
    let store = Arc::new(
        crate::checkpoints::LibsqlCheckpointer::open_at_for_workspace(
            config.join("threads.db"),
            directory
                .path()
                .canonicalize()
                .unwrap()
                .to_string_lossy()
                .into(),
        )
        .await
        .unwrap(),
    );
    let model = Arc::new(craftsman_harness::testing::ScriptedModel::new(vec![]));
    let fresh = craftsman_harness::Agent::builder(model.clone())
        .checkpointer(store.clone())
        .durable_turn_checkpoints(true)
        .build();
    assert!(fresh.load_thread(thread).await.unwrap());
    let (seq, saved, _) = store.load_latest(thread).await.unwrap().unwrap();
    assert!(seq > info["initial_sequence"].as_u64().unwrap());
    assert!(model.requests.lock().unwrap().is_empty());
    if mode == "saved" {
        assert!(fresh.recovery_info().is_none());
        assert_eq!(
            saved.custom["_craftsman_durable_turn_v1"]["in_progress"],
            false
        );
        assert!(
            matches!(&fresh.state.lock().unwrap().messages[0],craftsman_harness::Msg::User{content}if content.contains("Saved summary:"))
        );
    } else {
        let recovery = fresh.recovery_info().unwrap();
        assert!(recovery.unfinished_turn);
        assert_eq!(recovery.unresolved_tool_calls, 0);
        assert_eq!(fresh.state.lock().unwrap().messages.len(), 8);
        assert!(fresh.state.lock().unwrap().messages.iter().any(|message|matches!(message,craftsman_harness::Msg::Tool{tool_call_id,is_error:false,..}if tool_call_id=="write-once")));
    }
}

#[tokio::test]
async fn actual_terminal_compaction_survives_abrupt_exit_and_sql_failures_without_replay() {
    run_fixture("saved", 31).await;
}

#[tokio::test]
async fn abrupt_exit_during_actual_summary_reopens_original_results_without_replay() {
    run_fixture("interrupted", 32).await;
}
