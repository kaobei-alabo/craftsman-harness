use super::*;
use craftsman_harness::testing::ScriptedModel;
use craftsman_harness::{Agent, Tool, ToolCtx, ToolDef, ToolOutcome};
use std::path::PathBuf;
use std::sync::Arc;

async fn temporary_store() -> (tempfile::TempDir, LibsqlCheckpointer) {
    let directory = tempfile::tempdir().unwrap();
    let store = LibsqlCheckpointer::open_at_for_workspace(
        directory.path().join("threads.db"),
        "/isolated/recovery".into(),
    )
    .await
    .unwrap();
    (directory, store)
}

#[tokio::test]
async fn failed_state_insert_rolls_back_thread_and_context_metadata() {
    let (_directory, store) = temporary_store().await;
    store.conn.execute("CREATE TRIGGER refuse_checkpoint BEFORE INSERT ON checkpoints BEGIN SELECT RAISE(ABORT, 'synthetic storage failure'); END", ()).await.unwrap();
    let state = AgentState {
        messages: vec![craftsman_harness::Msg::user("request")],
        ..Default::default()
    };
    assert!(store.save("fresh", 0, &state, None).await.is_err());
    for table in ["threads", "thread_project_contexts", "checkpoints"] {
        let mut rows = store
            .conn
            .query(&format!("SELECT COUNT(*) FROM {table}"), ())
            .await
            .unwrap();
        assert_eq!(
            rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
            0
        );
    }
    store
        .conn
        .execute("DROP TRIGGER refuse_checkpoint", ())
        .await
        .unwrap();
    store.save("fresh", 1, &state, None).await.unwrap();
    assert_eq!(
        store
            .load_latest("fresh")
            .await
            .unwrap()
            .unwrap()
            .1
            .messages,
        state.messages
    );
}

#[tokio::test]
async fn failed_pruning_rolls_back_new_state_and_title_without_losing_prior_checkpoints() {
    let (_directory, store) = temporary_store().await;
    let state = AgentState::default();
    for seq in 0..50 {
        store.save("existing", seq, &state, None).await.unwrap();
    }
    let prior_title = store.list_threads().await.unwrap()[0].title.clone();
    store.conn.execute("CREATE TRIGGER refuse_prune BEFORE DELETE ON checkpoints BEGIN SELECT RAISE(ABORT, 'synthetic prune failure'); END", ()).await.unwrap();
    let changed = AgentState {
        messages: vec![craftsman_harness::Msg::user(
            "must not become the saved title",
        )],
        ..Default::default()
    };
    assert!(store.save("existing", 50, &changed, None).await.is_err());
    let (seq, saved, _) = store.load_latest("existing").await.unwrap().unwrap();
    assert_eq!(seq, 49);
    assert!(saved.messages.is_empty());
    let threads = store.list_threads().await.unwrap();
    assert_eq!(threads[0].title, prior_title);
    assert_eq!(threads[0].checkpoints, 50);
    store
        .conn
        .execute("DROP TRIGGER refuse_prune", ())
        .await
        .unwrap();
    store.save("existing", 51, &changed, None).await.unwrap();
    assert_eq!(store.load_latest("existing").await.unwrap().unwrap().0, 51);
    assert_eq!(store.list_threads().await.unwrap()[0].checkpoints, 50);
}

#[tokio::test]
async fn checkpoint_sequences_refuse_unsigned_overflow_and_negative_stored_values() {
    let (_directory, store) = temporary_store().await;
    assert!(
        store
            .save("fresh", i64::MAX as u64 + 1, &AgentState::default(), None)
            .await
            .is_err()
    );
    assert!(store.list_threads().await.unwrap().is_empty());
    store
        .save("fresh", 0, &AgentState::default(), None)
        .await
        .unwrap();
    store
        .conn
        .execute(
            "UPDATE checkpoints SET seq = -1 WHERE thread_id = 'fresh'",
            (),
        )
        .await
        .unwrap();
    assert!(store.load_latest("fresh").await.is_err());
}

#[tokio::test]
async fn local_checkpointer_requests_full_wal_synchronization() {
    let (_directory, store) = temporary_store().await;
    let mut rows = store.conn.query("PRAGMA synchronous", ()).await.unwrap();
    assert_eq!(
        rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
        2
    );
    let connection = store.checkpoint_conn.lock().await;
    let mut rows = connection.query("PRAGMA synchronous", ()).await.unwrap();
    assert_eq!(
        rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
        2
    );
}

#[tokio::test]
async fn checkpoint_transaction_is_separate_from_catalog_and_workspace_connection() {
    let (_directory, store) = temporary_store().await;
    let connection = store.checkpoint_conn.lock().await;
    let transaction = connection
        .transaction_with_behavior(libsql::TransactionBehavior::Immediate)
        .await
        .unwrap();
    transaction.execute("INSERT INTO threads (id, created, updated, title, workspace) VALUES ('uncommitted', 1, 1, 'private', '/isolated/recovery')", ()).await.unwrap();
    let mut rows = store
        .conn
        .query("SELECT COUNT(*) FROM threads WHERE id = 'uncommitted'", ())
        .await
        .unwrap();
    assert_eq!(
        rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
        0
    );
    transaction.rollback().await.unwrap();
}

#[tokio::test]
async fn durable_input_remains_consistent_in_memory_and_storage_after_provider_error() {
    let (_directory, store) = temporary_store().await;
    let store = Arc::new(store);
    let model = Arc::new(ScriptedModel::new(vec![]));
    let agent = Agent::builder(model.clone())
        .checkpointer(store.clone())
        .durable_turn_checkpoints(true)
        .thread_id("provider-error")
        .build();
    assert!(agent.run_turn("retain this request").await.is_err());
    crate::agent::rollback_dangling_user(&agent);
    let live = agent.state.lock().unwrap().messages.clone();
    assert_eq!(
        live,
        vec![craftsman_harness::Msg::user("retain this request")]
    );
    assert_eq!(
        store
            .load_latest("provider-error")
            .await
            .unwrap()
            .unwrap()
            .1
            .messages,
        live
    );
    assert_eq!(model.requests.lock().unwrap().len(), 1);
    assert!(!agent.checkpoint_failed());
}

const HELPER: &str = "checkpoints::recovery_tests::interrupted_checkpoint_child";
const MARKER: &str = "craftsman-checkpoint-crash-v1=";

fn marker_directory() -> Option<PathBuf> {
    let args: Vec<_> = std::env::args_os().collect();
    let markers: Vec<_> = args
        .iter()
        .filter_map(|arg| arg.to_str().and_then(|arg| arg.strip_prefix(MARKER)))
        .collect();
    if markers.len() != 1 {
        return None;
    }
    let directory = PathBuf::from(markers[0]);
    directory.is_absolute().then_some(directory)
}

struct CrashTool {
    directory: PathBuf,
    store: Arc<LibsqlCheckpointer>,
}
#[async_trait]
impl Tool for CrashTool {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "crash_fixture".into(),
            description: "owned crash fixture".into(),
            parameters: serde_json::json!({"type":"object"}),
        }
    }
    async fn call(
        &self,
        args: serde_json::Value,
        ctx: &mut ToolCtx<'_>,
    ) -> Result<ToolOutcome, AgentError> {
        if args["phase"] == "first" {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(self.directory.join("effect.txt"))
                .unwrap();
            file.write_all(b"one actual effect").unwrap();
            file.sync_all().unwrap();
            ctx.state.todos.push(craftsman_harness::state::Todo {
                content: "first write completed".into(),
                status: craftsman_harness::state::TodoStatus::Completed,
            });
            return Ok(ToolOutcome::ok("first physical write completed"));
        }
        assert_eq!(args["phase"], "crash");
        let (_, persisted, _) = self
            .store
            .load_latest("crash-thread")
            .await
            .unwrap()
            .unwrap();
        assert!(persisted.messages.iter().any(|m| matches!(m, craftsman_harness::Msg::Tool { tool_call_id, content, is_error: false } if tool_call_id == "first" && content == "first physical write completed")));
        std::fs::write(
            self.directory.join("checkpoint-observed"),
            b"actual first result committed before exit",
        )
        .unwrap();
        // Deliberately skip destructors and Agent::finish; the next process
        // must recover the committed tool result, without re-running any call.
        std::process::exit(23);
    }
}

#[tokio::test]
#[ignore = "owned abrupt-exit fixture; refuses unmarked startup before path/config inspection"]
async fn interrupted_checkpoint_child() {
    let Some(directory) = marker_directory() else {
        return;
    };
    let store = Arc::new(
        LibsqlCheckpointer::open_at_for_workspace(
            directory.join("threads.db"),
            "/isolated/recovery".into(),
        )
        .await
        .unwrap(),
    );
    let model = Arc::new(ScriptedModel::new(vec![
        craftsman_harness::Msg::Assistant {
            content: String::new(),
            tool_calls: ["first", "crash"]
                .into_iter()
                .map(|id| craftsman_harness::ToolCall {
                    id: id.into(),
                    name: "crash_fixture".into(),
                    args: serde_json::json!({"phase":id}),
                })
                .collect(),
        },
    ]));
    let agent = Agent::builder(model)
        .checkpointer(store.clone())
        .durable_turn_checkpoints(true)
        .thread_id("crash-thread")
        .tool(Arc::new(CrashTool { directory, store }))
        .build();
    agent.run_turn("write once, then interrupt").await.unwrap();
    panic!("marked helper must actually exit during the second call");
}

#[tokio::test]
async fn abrupt_native_exit_recovers_actual_write_result_and_does_not_replay_tools() {
    use std::process::Stdio;
    use std::time::Duration;
    use tokio::io::AsyncReadExt;
    let directory = tempfile::tempdir().unwrap();
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
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    // Windows needs its OS directory to load native system libraries. No model,
    // config or credential environment is inherited by this isolated helper.
    #[cfg(windows)]
    if let Some(root) = std::env::var_os("SystemRoot") {
        command.env("SystemRoot", root);
    }
    let mut child = command.spawn().unwrap();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let outcome = tokio::time::timeout(Duration::from_secs(30), async {
        let mut out = vec![];
        let mut err = vec![];
        let mut out_pipe = stdout.take(65537);
        let mut err_pipe = stderr.take(65537);
        let (status, _, _) = tokio::try_join!(
            child.wait(),
            out_pipe.read_to_end(&mut out),
            err_pipe.read_to_end(&mut err)
        )?;
        Ok::<_, std::io::Error>((status, out, err))
    })
    .await;
    let (status, out, err) = match outcome {
        Ok(Ok(result)) => result,
        _ => {
            let _ = tokio::time::timeout(Duration::from_secs(5), child.kill()).await;
            panic!("owned checkpoint crash fixture failed its execution bound");
        }
    };
    assert!(out.len() <= 65536 && err.len() <= 65536);
    assert_eq!(
        status.code(),
        Some(23),
        "child did not reach actual abrupt exit: {}",
        String::from_utf8_lossy(&err)
    );
    assert_eq!(
        std::fs::read(directory.path().join("checkpoint-observed")).unwrap(),
        b"actual first result committed before exit"
    );
    assert_eq!(
        std::fs::read(directory.path().join("effect.txt")).unwrap(),
        b"one actual effect"
    );
    let store = Arc::new(
        LibsqlCheckpointer::open_at_for_workspace(
            directory.path().join("threads.db"),
            "/isolated/recovery".into(),
        )
        .await
        .unwrap(),
    );
    let model = Arc::new(ScriptedModel::new(vec![craftsman_harness::Msg::assistant(
        "inspection complete",
    )]));
    let recovered = Agent::builder(model.clone())
        .checkpointer(store)
        .durable_turn_checkpoints(true)
        .build();
    recovered.load_thread("crash-thread").await.unwrap();
    assert_eq!(recovered.recovery_info().unwrap().unresolved_tool_calls, 1);
    assert!(model.requests.lock().unwrap().is_empty());
    let state = recovered.state.lock().unwrap().clone();
    assert_eq!(
        state.todos[0].status,
        craftsman_harness::state::TodoStatus::Completed
    );
    assert!(state.messages.iter().any(|m| matches!(m, craftsman_harness::Msg::Tool { tool_call_id, content, is_error: false } if tool_call_id == "first" && content == "first physical write completed")));
    assert!(state.messages.iter().any(|m| matches!(m, craftsman_harness::Msg::Tool { tool_call_id, content, is_error: true } if tool_call_id == "crash" && content.contains("may have had effects"))));
    recovered.run_turn("inspect, do not retry").await.unwrap();
    assert_eq!(model.requests.lock().unwrap().len(), 1);
    assert_eq!(
        std::fs::read(directory.path().join("effect.txt")).unwrap(),
        b"one actual effect"
    );
    assert_eq!(recovered.recovery_info().unwrap().unresolved_tool_calls, 1);
}
