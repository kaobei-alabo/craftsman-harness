//! Compatibility corpus: what the 0.4.0 baseline binary wrote must keep opening
//! with the current code.
//!
//! `tests/fixtures/v0.4.0/` is produced by `scripts/gen-fixtures-v040.py`, which
//! runs the binary built from the `baseline-0.4.0` tag against a mock model. It
//! holds four threads in one `threads.db` (version 0, never stamped) and one
//! journal per thread:
//!
//! - `fixture-plain`: a two-message exchange;
//! - `fixture-write-file`: `write_file` and `edit_file` in one round, so the
//!   workspace checkpoint tables hold a created file and a modified file with
//!   its pre-image;
//! - `fixture-http-400`: the second round failed, so the stored thread ends in a
//!   tool call with no result;
//! - `fixture-tools`: one round of three tool calls, `write_todos` (the state
//!   carries todos), a `read_file` of a missing file (a tool message with
//!   `is_error` true) and a `write_file` under `/memories` (a `store_files` row
//!   instead of a workspace file).
//!
//! The generator replaced thread and workspace-checkpoint ids, timestamps and
//! the workspace path (now `/workspace`, also inside the error message); everything
//! else is as the baseline wrote it. Each test works on a copy in a temp dir laid
//! out like `~/.craftsman`, because opening a database creates its WAL files.

use std::path::{Path, PathBuf};

use craftsman_harness::checkpoint::{Checkpointer, ThreadInfo};
use craftsman_harness::state::{AgentState, Msg, Todo, TodoStatus, ToolCall};
use craftsman_harness::{Backend, FileInfo};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::checkpoints::LibsqlCheckpointer;
use crate::store_fs::StoreFs;
use crate::threads_db::{self, Probe};
use crate::workspace_checkpoint::{
    NewWorkspaceCheckpoint, WorkspaceChangeKind, WorkspaceCheckpointFile,
};

const WORKSPACE: &str = "/workspace";
const PLAIN: &str = "fixture-plain";
const WRITE_FILE: &str = "fixture-write-file";
const HTTP_400: &str = "fixture-http-400";
const TOOLS: &str = "fixture-tools";

/// The README the generator seeds before each run, and the file `write_file`
/// creates in `fixture-write-file`.
const README: &str = "# Fixture workspace\n\nA tiny project.\n";
const NOTES: &str = "hello from the fixture\n";

/// What `fixture-tools` stored: the memory file, its text, and the message the
/// failed `read_file` left (the generator replaced the scratch path by `/workspace`).
const MEMORY_FILE: &str = "/memories/fixture.md";
const MEMORY_TEXT: &str = "remember me\n";
const MISSING_FILE_ERROR: &str =
    "state error: Error reading /workspace/missing.txt: No such file or directory (os error 2)";

/// A copy of the corpus in a temp dir laid out like `~/.craftsman`.
struct Corpus {
    dir: tempfile::TempDir,
}

impl Corpus {
    fn copy() -> Self {
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/v0.4.0");
        let dir = tempfile::tempdir().unwrap();
        std::fs::copy(source.join("threads.db"), dir.path().join("threads.db")).unwrap();
        std::fs::create_dir(dir.path().join("sessions-rs")).unwrap();
        for entry in std::fs::read_dir(source.join("sessions-rs")).unwrap() {
            let entry = entry.unwrap();
            std::fs::copy(
                entry.path(),
                dir.path().join("sessions-rs").join(entry.file_name()),
            )
            .unwrap();
        }
        Self { dir }
    }

    fn db(&self) -> PathBuf {
        self.dir.path().join("threads.db")
    }

    fn journals(&self) -> PathBuf {
        self.dir.path().join("sessions-rs")
    }

    /// Open the way a launch does: prepare the config dir, then open the store.
    async fn open(&self) -> LibsqlCheckpointer {
        let path = threads_db::prepare_at(self.dir.path()).await.unwrap();
        assert_eq!(path, self.db());
        LibsqlCheckpointer::open_at_for_workspace(path, WORKSPACE.into())
            .await
            .unwrap()
    }
}

fn sha256_hex(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

fn call(id: &str, name: &str, args: Value) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: name.into(),
        args,
    }
}

fn assistant_calling(tool_calls: Vec<ToolCall>) -> Msg {
    Msg::Assistant {
        content: String::new(),
        tool_calls,
    }
}

fn tool_result(id: &str, content: &str) -> Msg {
    Msg::Tool {
        tool_call_id: id.into(),
        content: content.into(),
        is_error: false,
    }
}

/// The generator's synthetic clock: thread number `n` (run order, 0 for plain)
/// was created at 1_760_000_000 + 3600 n and last updated ten seconds later;
/// its workspace checkpoint, if any, five seconds after creation. Hours apart on
/// purpose, so the newest-first order is a property of the data and not a tie.
const CLOCK: [(&str, i64); 4] = [
    (PLAIN, 1_760_000_000),
    (WRITE_FILE, 1_760_003_600),
    (HTTP_400, 1_760_007_200),
    (TOOLS, 1_760_010_800),
];

/// (id, created, updated) of every listed thread must be the synthetic clock's,
/// newest first.
fn assert_clock(threads: &[ThreadInfo]) {
    let mut expected = CLOCK
        .iter()
        .map(|(id, created)| (*id, *created, created + 10))
        .collect::<Vec<_>>();
    expected.reverse();
    assert_eq!(
        threads
            .iter()
            .map(|thread| (thread.id.as_str(), thread.created, thread.updated))
            .collect::<Vec<_>>(),
        expected
    );
}

fn failed_tool_result(id: &str, content: &str) -> Msg {
    Msg::Tool {
        tool_call_id: id.into(),
        content: content.into(),
        is_error: true,
    }
}

/// `store_files` as committed right now: (path, content, modified).
async fn store_rows(outside: &Probe) -> Vec<(String, String, i64)> {
    let mut rows = outside
        .conn()
        .query(
            "SELECT path, content, modified FROM store_files ORDER BY path",
            (),
        )
        .await
        .unwrap();
    let mut out = Vec::new();
    while let Some(row) = rows.next().await.unwrap() {
        out.push((
            row.get::<String>(0).unwrap(),
            row.get::<String>(1).unwrap(),
            row.get::<i64>(2).unwrap(),
        ));
    }
    out
}

#[tokio::test]
async fn corpus_is_a_rust_schema_database_the_baseline_left_unstamped() {
    let corpus = Corpus::copy();
    let outside = Probe::open(&corpus.db()).await;
    assert_eq!(outside.user_version().await, 0);

    // Preparation takes it for a Rust-schema database: nothing is archived or
    // moved, and it still has no version.
    let path = threads_db::prepare_at(corpus.dir.path()).await.unwrap();
    assert_eq!(path, corpus.db());
    assert!(path.exists());
    for entry in std::fs::read_dir(corpus.dir.path()).unwrap() {
        let name = entry.unwrap().file_name().to_string_lossy().to_string();
        assert!(!name.contains("legacy"), "archived: {name}");
    }
    assert_eq!(outside.user_version().await, 0);
}

/// What `sqlite_master` of a database 0.4.0 wrote holds, spelled out here instead
/// of read back from the file under test: (type, name, tbl_name, sql). A corpus
/// that was regenerated or edited with a binary that adds a column, an index or a
/// table, or that is the wrong file, fails here although every other test would
/// still open it. `sql` is the statement as SQLite stored it, whitespace
/// included; the automatic indexes behind the primary keys have none.
const LAYOUT_0_4_0: [(&str, &str, &str, Option<&str>); 15] = [
    (
        "index",
        "sqlite_autoindex_checkpoints_1",
        "checkpoints",
        None,
    ),
    (
        "index",
        "sqlite_autoindex_store_files_1",
        "store_files",
        None,
    ),
    (
        "index",
        "sqlite_autoindex_thread_project_contexts_1",
        "thread_project_contexts",
        None,
    ),
    ("index", "sqlite_autoindex_threads_1", "threads", None),
    (
        "index",
        "sqlite_autoindex_workspace_checkpoint_files_1",
        "workspace_checkpoint_files",
        None,
    ),
    (
        "index",
        "sqlite_autoindex_workspace_checkpoints_1",
        "workspace_checkpoints",
        None,
    ),
    (
        "index",
        "thread_project_contexts_position",
        "thread_project_contexts",
        Some(
            r"CREATE UNIQUE INDEX thread_project_contexts_position
             ON thread_project_contexts(thread_id, position)",
        ),
    ),
    (
        "index",
        "threads_workspace_updated",
        "threads",
        Some(
            r"CREATE INDEX threads_workspace_updated ON threads(workspace, updated DESC, id DESC)",
        ),
    ),
    (
        "index",
        "workspace_checkpoints_thread_created",
        "workspace_checkpoints",
        Some(
            r"CREATE INDEX workspace_checkpoints_thread_created
             ON workspace_checkpoints(thread_id, created DESC, id DESC)",
        ),
    ),
    (
        "table",
        "checkpoints",
        "checkpoints",
        Some(
            r"CREATE TABLE checkpoints (
                thread_id TEXT NOT NULL,
                seq INTEGER NOT NULL,
                state_json TEXT NOT NULL,
                suspension_json TEXT,
                PRIMARY KEY (thread_id, seq)
            )",
        ),
    ),
    (
        "table",
        "store_files",
        "store_files",
        Some(
            r"CREATE TABLE store_files (
                path TEXT PRIMARY KEY,
                content TEXT NOT NULL,
                modified INTEGER NOT NULL
            )",
        ),
    ),
    (
        "table",
        "thread_project_contexts",
        "thread_project_contexts",
        Some(
            r"CREATE TABLE thread_project_contexts (
                thread_id TEXT NOT NULL,
                path TEXT NOT NULL,
                position INTEGER NOT NULL,
                PRIMARY KEY (thread_id, path),
                FOREIGN KEY (thread_id) REFERENCES threads(id) ON DELETE CASCADE
            )",
        ),
    ),
    (
        "table",
        "threads",
        "threads",
        Some(
            r"CREATE TABLE threads (
                id TEXT PRIMARY KEY,
                created INTEGER NOT NULL,
                updated INTEGER NOT NULL DEFAULT 0,
                title TEXT NOT NULL,
                workspace TEXT NOT NULL DEFAULT ''
            )",
        ),
    ),
    (
        "table",
        "workspace_checkpoint_files",
        "workspace_checkpoint_files",
        Some(
            r"CREATE TABLE workspace_checkpoint_files (
                checkpoint_id TEXT NOT NULL,
                path TEXT NOT NULL,
                change_kind TEXT NOT NULL,
                before_hash TEXT,
                after_hash TEXT,
                before_mode INTEGER,
                after_mode INTEGER,
                before_content BLOB,
                after_content BLOB,
                restorable INTEGER NOT NULL,
                PRIMARY KEY (checkpoint_id, path),
                FOREIGN KEY (checkpoint_id) REFERENCES workspace_checkpoints(id) ON DELETE CASCADE
            )",
        ),
    ),
    (
        "table",
        "workspace_checkpoints",
        "workspace_checkpoints",
        Some(
            r"CREATE TABLE workspace_checkpoints (
                id TEXT PRIMARY KEY,
                thread_id TEXT NOT NULL,
                turn_seq INTEGER NOT NULL,
                created INTEGER NOT NULL,
                summary TEXT NOT NULL,
                partial INTEGER NOT NULL DEFAULT 0
            )",
        ),
    ),
];
/// (type, name, tbl_name, sql) of every `sqlite_master` row, as committed now.
async fn layout_of(outside: &Probe) -> Vec<(String, String, String, Option<String>)> {
    let mut rows = outside
        .conn()
        .query(
            "SELECT type, name, tbl_name, sql FROM sqlite_master ORDER BY type, name",
            (),
        )
        .await
        .unwrap();
    let mut found = Vec::new();
    while let Some(row) = rows.next().await.unwrap() {
        let sql = match row.get_value(3).unwrap() {
            libsql::Value::Null => None,
            libsql::Value::Text(text) => Some(text),
            other => panic!("sql is {other:?}"),
        };
        found.push((
            row.get::<String>(0).unwrap(),
            row.get::<String>(1).unwrap(),
            row.get::<String>(2).unwrap(),
            sql,
        ));
    }
    found
}

fn layout_0_4_0() -> Vec<(String, String, String, Option<String>)> {
    LAYOUT_0_4_0
        .iter()
        .map(|(kind, name, table, sql)| {
            (
                kind.to_string(),
                name.to_string(),
                table.to_string(),
                sql.map(str::to_string),
            )
        })
        .collect()
}

#[tokio::test]
async fn corpus_has_exactly_the_layout_0_4_0_wrote() {
    let corpus = Corpus::copy();
    let outside = Probe::open(&corpus.db()).await;

    assert_eq!(layout_of(&outside).await, layout_0_4_0());

    // The file-level properties the baseline's database had.
    let pragma = async |name: &str| -> String {
        let mut rows = outside
            .conn()
            .query(&format!("PRAGMA {name}"), ())
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        match row.get_value(0).unwrap() {
            libsql::Value::Integer(number) => number.to_string(),
            libsql::Value::Text(text) => text,
            other => panic!("{name} is {other:?}"),
        }
    };
    assert_eq!(pragma("page_size").await, "4096");
    assert_eq!(pragma("encoding").await, "UTF-8");
    assert_eq!(pragma("journal_mode").await, "wal");
    assert_eq!(pragma("application_id").await, "0");
}

/// The committed bytes themselves: no path from the machine that generated them,
/// and journals with LF line endings only (a checkout that converted them would
/// break the hashes the README lists; .gitattributes keeps that from happening).
#[test]
fn corpus_files_hold_no_machine_paths_and_the_journals_keep_their_line_endings() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/v0.4.0");
    let mut files = vec![root.join("threads.db")];
    for entry in std::fs::read_dir(root.join("sessions-rs")).unwrap() {
        files.push(entry.unwrap().path());
    }
    assert_eq!(files.len(), 5);
    for file in &files {
        let bytes = std::fs::read(file).unwrap();
        for forbidden in [
            "/Users/",
            "/home/",
            "/private/",
            "/var/folders/",
            "/tmp/",
            "\\Users\\",
        ] {
            assert!(
                !bytes
                    .windows(forbidden.len())
                    .any(|window| window == forbidden.as_bytes()),
                "{} holds {forbidden}",
                file.display()
            );
        }
        if file
            .extension()
            .is_some_and(|extension| extension == "jsonl")
        {
            assert!(!bytes.contains(&b'\r'), "{} has a CR", file.display());
            assert!(
                bytes.ends_with(b"\n"),
                "{} lacks a final LF",
                file.display()
            );
        }
    }
}

#[tokio::test]
async fn opening_the_corpus_stamps_it_with_version_one() {
    let corpus = Corpus::copy();
    let outside = Probe::open(&corpus.db()).await;
    assert_eq!(outside.user_version().await, 0);

    let _store = corpus.open().await;
    // Committed: a connection that was already open sees it.
    assert_eq!(outside.user_version().await, 1);
}

#[tokio::test]
async fn opening_the_corpus_changes_nothing_but_the_version() {
    let corpus = Corpus::copy();
    let outside = Probe::open(&corpus.db()).await;
    let before = outside.dump().await;
    // Every table of the 0.4.0 layout is there, and has rows worth comparing.
    for table in [
        "threads",
        "thread_project_contexts",
        "checkpoints",
        "workspace_checkpoints",
        "workspace_checkpoint_files",
        "store_files",
    ] {
        assert!(
            before
                .iter()
                .any(|line| line.starts_with(&format!("{table}: "))),
            "{table}"
        );
    }

    drop(corpus.open().await);
    assert_eq!(outside.user_version().await, 1);
    // Same tables, columns, indexes and rows as the baseline left them.
    assert_eq!(outside.dump().await, before);
}

#[tokio::test]
async fn thread_list_loads_newest_first() {
    let corpus = Corpus::copy();
    let store = corpus.open().await;

    let threads = store.list_threads().await.unwrap();
    let listed = threads
        .iter()
        .map(|thread| {
            (
                thread.id.as_str(),
                thread.title.as_str(),
                thread.checkpoints,
                thread.workspace.as_str(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        listed,
        [
            (
                TOOLS,
                "Plan the work, read missing.txt and remember the outcome.",
                3,
                WORKSPACE
            ),
            (HTTP_400, "Read README.md.", 1, WORKSPACE),
            (
                WRITE_FILE,
                "Create notes.txt and mention it in README.md.",
                3,
                WORKSPACE
            ),
            (PLAIN, "Say hello.", 2, WORKSPACE),
        ]
    );
    for thread in &threads {
        assert_eq!(thread.project_contexts, [WORKSPACE]);
    }
    assert_clock(&threads);

    // An unscoped page (every workspace) sees the same threads.
    let (all, has_more) = store.list_threads_page(0, 10, None).await.unwrap();
    assert!(!has_more);
    assert_eq!(
        all.iter()
            .map(|thread| thread.id.as_str())
            .collect::<Vec<_>>(),
        [TOOLS, HTTP_400, WRITE_FILE, PLAIN]
    );
}

#[tokio::test]
async fn catalog_listing_reads_and_stamps_the_corpus() {
    // The desktop catalog opens the database without a checkpointer and applies
    // the schema itself, so it stamps too.
    let corpus = Corpus::copy();
    let outside = Probe::open(&corpus.db()).await;
    let before = outside.dump().await;
    assert_eq!(outside.user_version().await, 0);

    let (threads, has_more) = LibsqlCheckpointer::list_all_threads_page_at(corpus.db(), 0, 10)
        .await
        .unwrap();
    assert!(!has_more);
    assert_eq!(
        threads
            .iter()
            .map(|thread| (thread.id.as_str(), thread.checkpoints))
            .collect::<Vec<_>>(),
        [(TOOLS, 3), (HTTP_400, 1), (WRITE_FILE, 3), (PLAIN, 2)]
    );
    for thread in &threads {
        assert_eq!(thread.workspace, WORKSPACE);
        assert_eq!(thread.project_contexts, [WORKSPACE]);
    }
    assert_clock(&threads);
    assert_eq!(outside.user_version().await, 1);
    assert_eq!(outside.dump().await, before);

    // A page of two says that more follow.
    let (page, more) = LibsqlCheckpointer::list_all_threads_page_at(corpus.db(), 0, 2)
        .await
        .unwrap();
    assert_eq!(page.len(), 2);
    assert!(more);
}

#[tokio::test]
async fn latest_checkpoint_of_each_thread_loads() {
    let corpus = Corpus::copy();
    let store = corpus.open().await;

    let (seq, state, suspension) = store.load_latest(PLAIN).await.unwrap().unwrap();
    assert_eq!(seq, 1);
    assert!(suspension.is_none());
    assert_eq!(
        state.messages,
        [
            Msg::user("Say hello."),
            Msg::assistant("Hello from the fixture model."),
        ]
    );

    let (seq, state, suspension) = store.load_latest(WRITE_FILE).await.unwrap().unwrap();
    assert_eq!(seq, 2);
    assert!(suspension.is_none());
    assert_eq!(
        state.messages,
        [
            Msg::user("Create notes.txt and mention it in README.md."),
            assistant_calling(vec![
                call(
                    "call_1",
                    "write_file",
                    json!({"file_path": "notes.txt", "content": NOTES}),
                ),
                call(
                    "call_2",
                    "edit_file",
                    json!({
                        "file_path": "README.md",
                        "old_string": "A tiny project.",
                        "new_string": "A tiny project. See notes.txt.",
                    }),
                ),
            ]),
            tool_result("call_1", "Updated file notes.txt"),
            tool_result("call_2", "Updated file README.md"),
            Msg::assistant("Created notes.txt and mentioned it in README.md."),
        ]
    );
    assert_eq!(
        state.custom["_todo_meta"],
        json!({"last_write": 0, "round": 2})
    );

    // The failed turn: checkpoints are written after an assistant message, not
    // after tool results, so the thread ends in a call that has no result.
    let (seq, state, suspension) = store.load_latest(HTTP_400).await.unwrap().unwrap();
    assert_eq!(seq, 0);
    assert!(suspension.is_none());
    assert_eq!(
        state.messages,
        [
            Msg::user("Read README.md."),
            assistant_calling(vec![call(
                "call_1",
                "read_file",
                json!({"file_path": "README.md"}),
            )]),
        ]
    );

    // The tool round: write_todos filled the todo list, the failed read_file left
    // a tool message with is_error set, and the memory write is only a result here
    // (its file is in store_files).
    let (seq, state, suspension) = store.load_latest(TOOLS).await.unwrap().unwrap();
    assert_eq!(seq, 2);
    assert!(suspension.is_none());
    assert_eq!(
        state.messages,
        [
            Msg::user("Plan the work, read missing.txt and remember the outcome."),
            assistant_calling(vec![
                call(
                    "call_1",
                    "write_todos",
                    json!({"todos": [
                        {"content": "step one", "status": "in_progress"},
                        {"content": "step two", "status": "pending"},
                    ]}),
                ),
                call("call_2", "read_file", json!({"file_path": "missing.txt"})),
                call(
                    "call_3",
                    "write_file",
                    json!({"file_path": MEMORY_FILE, "content": MEMORY_TEXT}),
                ),
            ]),
            tool_result(
                "call_1",
                r#"Updated todo list to [{"content":"step one","status":"in_progress"},{"content":"step two","status":"pending"}]"#
            ),
            failed_tool_result("call_2", MISSING_FILE_ERROR),
            tool_result("call_3", "Updated file /memories/fixture.md"),
            Msg::assistant("Planned two steps; missing.txt does not exist; noted in memory."),
        ]
    );
    assert_eq!(
        state.todos,
        [
            Todo {
                content: "step one".into(),
                status: TodoStatus::InProgress
            },
            Todo {
                content: "step two".into(),
                status: TodoStatus::Pending
            },
        ]
    );
    assert_eq!(
        state.custom["_todo_meta"],
        json!({"last_write": 1, "round": 3})
    );

    // The export paths read the same rows.
    for thread in [PLAIN, WRITE_FILE, HTTP_400, TOOLS] {
        let scoped = store.latest_state(thread).await.unwrap().unwrap();
        let global = store.latest_state_global(thread).await.unwrap().unwrap();
        assert_eq!(scoped.messages, global.messages);
    }
}

#[tokio::test]
async fn workspace_checkpoint_rows_of_the_write_file_thread_are_readable() {
    let corpus = Corpus::copy();
    let store = corpus.open().await;

    let summaries = store.list_workspace_checkpoints(WRITE_FILE).await.unwrap();
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].id, "wc-fixture-0001");
    assert_eq!(summaries[0].turn_seq, 1);
    // Five seconds after the thread was created (see CLOCK).
    assert_eq!(summaries[0].created, 1_760_003_605);
    assert_eq!(
        summaries[0].summary,
        "Create notes.txt and mention it in README.md."
    );
    assert_eq!(summaries[0].file_count, 2);
    assert!(!summaries[0].partial);

    let stored = store
        .load_workspace_checkpoint(WRITE_FILE, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.summary, summaries[0]);
    assert_eq!(stored.files.len(), 2);

    // Files come back ordered by path. The edit keeps the pre-turn bytes so the
    // turn can be rewound; the created file has nothing before it.
    let readme = &stored.files[0];
    assert_eq!(readme.path, "README.md");
    assert_eq!(readme.kind, WorkspaceChangeKind::Modified);
    assert_eq!(readme.before_hash, Some(sha256_hex(README)));
    assert_eq!(
        readme.after_hash,
        Some(sha256_hex(
            "# Fixture workspace\n\nA tiny project. See notes.txt.\n"
        ))
    );
    assert_eq!(readme.before_content.as_deref(), Some(README.as_bytes()));
    assert_eq!(readme.after_content, None);
    assert_eq!(
        (readme.before_mode, readme.after_mode),
        (Some(0o644), Some(0o644))
    );
    assert!(readme.restorable);

    let notes = &stored.files[1];
    assert_eq!(notes.path, "notes.txt");
    assert_eq!(notes.kind, WorkspaceChangeKind::Created);
    assert_eq!(notes.before_hash, None);
    assert_eq!(notes.after_hash, Some(sha256_hex(NOTES)));
    assert_eq!(notes.before_content, None);
    // Only the pre-image is kept; the file after the turn is on disk, not here.
    assert_eq!(notes.after_content, None);
    assert_eq!((notes.before_mode, notes.after_mode), (None, Some(0o644)));
    assert!(notes.restorable);

    // The other threads changed no files, so they have no workspace checkpoints.
    for thread in [PLAIN, HTTP_400, TOOLS] {
        assert!(
            store
                .list_workspace_checkpoints(thread)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            store
                .load_workspace_checkpoint(thread, None)
                .await
                .unwrap()
                .is_none()
        );
    }
}

#[tokio::test]
async fn journals_parse_as_json_lines_and_match_the_checkpoints() {
    let corpus = Corpus::copy();
    let store = corpus.open().await;

    let mut files = std::fs::read_dir(corpus.journals())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
        .collect::<Vec<_>>();
    files.sort();
    assert_eq!(
        files,
        [
            "fixture-http-400.jsonl",
            "fixture-plain.jsonl",
            "fixture-tools.jsonl",
            "fixture-write-file.jsonl"
        ]
    );

    // (thread, the event kinds the journal holds, in order). The failed thread's
    // journal is one event ahead of its checkpoint: the tool result was journaled
    // but never checkpointed.
    let expected: [(&str, &[&str]); 4] = [
        (PLAIN, &["user", "assistant"]),
        (
            WRITE_FILE,
            &["user", "assistant", "tool", "tool", "assistant"],
        ),
        (HTTP_400, &["user", "assistant", "tool"]),
        (
            TOOLS,
            &["user", "assistant", "tool", "tool", "tool", "assistant"],
        ),
    ];
    for (thread, kinds) in expected {
        let body =
            std::fs::read_to_string(corpus.journals().join(format!("{thread}.jsonl"))).unwrap();
        let events = body
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            events
                .iter()
                .map(|event| event["kind"].as_str().unwrap())
                .collect::<Vec<_>>(),
            kinds,
            "{thread}"
        );
        // The raw JSON is checked only for what a reader of the journal relies on:
        // the kind sequence above, a timestamp, and the role matching the kind.
        for event in &events {
            assert!(event["ts"].as_i64().unwrap() > 0, "{thread}");
            assert_eq!(event["data"]["role"], event["kind"], "{thread}");
        }

        // Everything else goes from the old bytes to today's type: each journaled
        // event must still read as a `Msg`, and as the very message the checkpoint
        // stored. How today's serializer would write that message is no business of
        // a 0.4.0 journal, so an additive change to it must not fail this test.
        let journaled = events
            .iter()
            .map(|event| {
                serde_json::from_value::<Msg>(event["data"].clone())
                    .unwrap_or_else(|error| panic!("{thread}: {error}: {event}"))
            })
            .collect::<Vec<_>>();
        let (_, state, _) = store.load_latest(thread).await.unwrap().unwrap();
        for (message, from_journal) in state.messages.iter().zip(&journaled) {
            assert_eq!(from_journal, message, "{thread}");
        }
        assert_eq!(
            events.len(),
            state.messages.len() + usize::from(thread == HTTP_400),
            "{thread}"
        );
    }
}

/// `store_files.modified` of the memory file: the generator's synthetic clock
/// (epoch 1_760_000_000, plus three hours for the fourth thread, plus 3 s).
const MEMORY_MODIFIED: i64 = 1_760_010_803;

#[tokio::test]
async fn memory_file_the_baseline_wrote_is_readable_and_the_store_writable() {
    let corpus = Corpus::copy();
    let outside = Probe::open(&corpus.db()).await;
    let before = outside.dump().await;
    assert_eq!(
        store_rows(&outside).await,
        [(MEMORY_FILE.into(), MEMORY_TEXT.into(), MEMORY_MODIFIED)]
    );

    // The store the agent opens first (before the checkpointer) reads it back.
    let memory = StoreFs::open_at(corpus.db()).await.unwrap();
    assert_eq!(
        memory.ls("/memories/").await.unwrap(),
        [FileInfo {
            path: MEMORY_FILE.into(),
            size: MEMORY_TEXT.len() as u64
        }]
    );
    assert_eq!(
        memory.read(MEMORY_FILE, 0, 100).await.unwrap(),
        "     1\tremember me"
    );
    assert_eq!(memory.glob("/memories/*.md").await.unwrap(), [MEMORY_FILE]);
    assert_eq!(
        memory.grep("remember", None).await.unwrap(),
        "/memories/fixture.md:1:remember me\n"
    );
    // Opening it changed nothing, and stamped nothing: only the checkpointer
    // stamps, so a launch that stops after this leaves the file at version 0.
    assert_eq!(outside.dump().await, before);
    assert_eq!(outside.user_version().await, 0);

    // Writes work on the baseline's table: a new file, an edit, and a refusal to
    // overwrite.
    let error = memory.write(MEMORY_FILE, "again").await.unwrap_err();
    assert!(error.to_string().contains("already exists"), "{error}");
    memory.write("/memories/second.md", "two\n").await.unwrap();
    memory
        .edit(MEMORY_FILE, "remember me", "remember us", false)
        .await
        .unwrap();
    let rows = store_rows(&outside).await;
    assert_eq!(
        rows.iter()
            .map(|(path, content, _)| (path.as_str(), content.as_str()))
            .collect::<Vec<_>>(),
        [
            (MEMORY_FILE, "remember us\n"),
            ("/memories/second.md", "two\n")
        ]
    );
    // Both writes stamped the real clock over the generator's.
    assert!(
        rows.iter()
            .all(|(_, _, modified)| *modified > MEMORY_MODIFIED)
    );
}

/// How many rows of `table` a [`Probe::dump`] holds.
fn rows_in(dump: &[String], table: &str) -> usize {
    dump.iter()
        .filter(|line| line.starts_with(&format!("{table}: ")))
        .count()
}

/// Resume and extend a database the baseline wrote, the way a newer binary does
/// every day, then reopen it. Every row the baseline stored must still be there,
/// and the only ones that may differ are the thread rows the new writes updated
/// on purpose; no table, column or index may appear.
#[tokio::test]
async fn a_0_4_0_database_can_be_resumed_extended_and_reopened() {
    let corpus = Corpus::copy();
    let outside = Probe::open(&corpus.db()).await;
    let before = outside.dump().await;
    let store = corpus.open().await;

    // Resume a thread: load its latest checkpoint, save the next one on top.
    let (seq, mut state, _) = store.load_latest(PLAIN).await.unwrap().unwrap();
    assert_eq!(seq, 1);
    state.messages.push(Msg::user("And once more."));
    state.messages.push(Msg::assistant("Hello once more."));
    store.save(PLAIN, seq + 1, &state, None).await.unwrap();

    // Rename another one.
    store
        .rename_thread(WRITE_FILE, "Renamed by a newer version")
        .await
        .unwrap();

    // Start a new conversation next to the old ones.
    let mut fresh = AgentState::default();
    fresh.messages.push(Msg::user("A new question."));
    fresh.messages.push(Msg::assistant("A new answer."));
    store.save("fixture-new", 0, &fresh, None).await.unwrap();

    // A workspace checkpoint on the thread that already has one from 0.4.0.
    store
        .save_workspace_checkpoint(&NewWorkspaceCheckpoint {
            id: "wc-new".into(),
            thread_id: WRITE_FILE.into(),
            turn_seq: 3,
            created: 1_760_003_700,
            summary: "A later edit".into(),
            partial: false,
            files: vec![WorkspaceCheckpointFile {
                path: "notes.txt".into(),
                kind: WorkspaceChangeKind::Modified,
                before_hash: Some(sha256_hex(NOTES)),
                after_hash: Some(sha256_hex("changed\n")),
                before_mode: Some(0o644),
                after_mode: Some(0o644),
                before_content: Some(NOTES.as_bytes().to_vec()),
                after_content: None,
                restorable: true,
            }],
        })
        .await
        .unwrap();

    // Reopen from scratch: everything below comes back from the file.
    drop(store);
    let store = LibsqlCheckpointer::open_at_for_workspace(corpus.db(), WORKSPACE.into())
        .await
        .unwrap();
    assert_eq!(outside.user_version().await, 1);

    let (seq, state, suspension) = store.load_latest(PLAIN).await.unwrap().unwrap();
    assert_eq!(seq, 2);
    assert!(suspension.is_none());
    assert_eq!(
        state.messages,
        [
            Msg::user("Say hello."),
            Msg::assistant("Hello from the fixture model."),
            Msg::user("And once more."),
            Msg::assistant("Hello once more."),
        ]
    );
    let (seq, state, _) = store.load_latest("fixture-new").await.unwrap().unwrap();
    assert_eq!(seq, 0);
    assert_eq!(state.messages.len(), 2);

    // The three threads written to are the newest, stamped with the real clock;
    // the two the writes did not touch keep their place and their synthetic one.
    let threads = store.list_threads().await.unwrap();
    assert_eq!(threads.len(), 5);
    assert_eq!(
        threads[3..]
            .iter()
            .map(|thread| (thread.id.as_str(), thread.created, thread.updated))
            .collect::<Vec<_>>(),
        [
            (TOOLS, 1_760_010_800, 1_760_010_810),
            (HTTP_400, 1_760_007_200, 1_760_007_210)
        ]
    );
    let by_id = |id: &str| threads.iter().find(|thread| thread.id == id).unwrap();
    for id in [PLAIN, WRITE_FILE, "fixture-new"] {
        assert!(threads[..3].iter().any(|thread| thread.id == id), "{id}");
        assert!(by_id(id).updated > 1_760_010_810, "{id}");
    }
    let plain = by_id(PLAIN);
    assert_eq!(
        (plain.title.as_str(), plain.created, plain.checkpoints),
        ("Say hello.", 1_760_000_000, 3)
    );
    let renamed = by_id(WRITE_FILE);
    assert_eq!(
        (renamed.title.as_str(), renamed.created, renamed.checkpoints),
        ("Renamed by a newer version", 1_760_003_600, 3)
    );
    let new = by_id("fixture-new");
    assert_eq!(new.title, "A new question.");
    assert_eq!((new.checkpoints, new.workspace.as_str()), (1, WORKSPACE));
    assert_eq!(new.project_contexts, [WORKSPACE]);

    // The new workspace checkpoint sits on top of the one 0.4.0 wrote, which
    // still reads exactly as before.
    let summaries = store.list_workspace_checkpoints(WRITE_FILE).await.unwrap();
    assert_eq!(
        summaries
            .iter()
            .map(|summary| (summary.id.as_str(), summary.file_count))
            .collect::<Vec<_>>(),
        [("wc-new", 1), ("wc-fixture-0001", 2)]
    );
    let latest = store
        .load_workspace_checkpoint(WRITE_FILE, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(latest.summary.id, "wc-new");
    assert_eq!(
        latest.files[0].before_content.as_deref(),
        Some(NOTES.as_bytes())
    );
    let old = store
        .load_workspace_checkpoint(WRITE_FILE, Some("wc-fixture-0001"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(old.summary.created, 1_760_003_605);
    assert_eq!(
        old.files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>(),
        ["README.md", "notes.txt"]
    );

    // On disk: nothing the baseline wrote is gone or changed, except the two
    // thread rows the resume and the rename updated on purpose; the rest is new.
    let after = outside.dump().await;
    let changed = before
        .iter()
        .filter(|line| !after.contains(line))
        .collect::<Vec<_>>();
    assert_eq!(changed.len(), 2, "{changed:#?}");
    for line in &changed {
        assert!(
            [PLAIN, WRITE_FILE]
                .iter()
                .any(|id| line.starts_with(&format!("threads: Text({id:?}) | "))),
            "{line}"
        );
    }
    for (table, rows) in [
        ("threads", 5),
        ("thread_project_contexts", 5),
        ("checkpoints", 11),
        ("workspace_checkpoints", 2),
        ("workspace_checkpoint_files", 3),
        ("store_files", 1),
    ] {
        assert_eq!(rows_in(&after, table), rows, "{table}");
    }
    assert_eq!(layout_of(&outside).await, layout_0_4_0());
}

/// The README next to the corpus lists the SHA-256 of every file in it. A
/// regeneration that changes a byte without updating the README fails here, and
/// so does a README that lists a file that is gone or leaves one out: the corpus
/// changes only through a reviewed diff.
#[test]
fn readme_lists_the_sha256_of_every_corpus_file() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/v0.4.0");

    // The "Files" section holds rows like: | `path` | bytes | `sha256` |
    let readme = std::fs::read_to_string(root.join("README.md")).unwrap();
    let files_section = readme
        .split("\n## ")
        .find(|section| section.starts_with("Files\n"))
        .expect("README.md has a Files section");
    let mut listed = std::collections::BTreeMap::new();
    for line in files_section.lines().filter(|line| line.starts_with("| `")) {
        let ticks = line.split('`').collect::<Vec<_>>();
        assert_eq!(ticks.len(), 5, "{line}");
        assert!(
            listed
                .insert(ticks[1].to_string(), ticks[3].to_string())
                .is_none(),
            "listed twice: {line}"
        );
    }

    let mut present = std::collections::BTreeMap::new();
    let mut paths = vec![("threads.db".to_string(), root.join("threads.db"))];
    for entry in std::fs::read_dir(root.join("sessions-rs")).unwrap() {
        let entry = entry.unwrap();
        paths.push((
            format!("sessions-rs/{}", entry.file_name().to_string_lossy()),
            entry.path(),
        ));
    }
    for (name, path) in paths {
        let hash = format!("{:x}", Sha256::digest(std::fs::read(path).unwrap()));
        present.insert(name, hash);
    }
    assert_eq!(listed, present);
}
