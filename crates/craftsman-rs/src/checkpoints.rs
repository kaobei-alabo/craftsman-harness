//! App-layer durable state: a libSQL [`Checkpointer`] and a JSONL
//! [`TranscriptSink`]. The framework (`craftsman-harness`) defines *when* to persist
//! (see `checkpoint.rs`); this module is the concrete storage the CLI wires in.
//!
//! Store lives in `~/.craftsman/threads.db`. On first launch after the Rust-repo
//! rename, the legacy `threads-rs.db` is migrated automatically and an
//! incompatible legacy database is archived without deletion. The store
//! is a plain local file: there is no remote sync.
//!
//! The journal follows Claude Code's per-session JSONL: one line per event under
//! `~/.craftsman/sessions-rs/{thread_id}.jsonl`, restamped with the app's clock
//! (the framework passes `ts: 0`).
//!
use std::sync::Once;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use craftsman_harness::checkpoint::{Checkpointer, ThreadInfo, TranscriptEvent, TranscriptSink};
use craftsman_harness::error::AgentError;
use craftsman_harness::interrupt::Suspension;
use craftsman_harness::state::{AgentState, Msg};

use crate::workspace_checkpoint::{
    NewWorkspaceCheckpoint, StoredWorkspaceCheckpoint, WorkspaceChangeKind,
    WorkspaceCheckpointFile, WorkspaceCheckpointSummary,
};

/// Keep at most this many checkpoints per thread; older rows are pruned on save.
const MAX_CHECKPOINTS_PER_THREAD: i64 = 50;
/// Workspace snapshots contain file bytes, so retain a smaller bounded window.
const MAX_WORKSPACE_CHECKPOINTS_PER_THREAD: i64 = 20;

fn unix_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn state_err(e: impl std::fmt::Display) -> AgentError {
    AgentError::State(e.to_string())
}

/// First user message in the state, one-lined and truncated — the thread title.
fn thread_title(state: &AgentState) -> String {
    for m in &state.messages {
        if let Msg::User { content } = m {
            return crate::util::one_line(content, 60);
        }
    }
    String::new()
}

/// libSQL-backed [`Checkpointer`] over the local `~/.craftsman/threads.db`.
pub struct LibsqlCheckpointer {
    /// Held for the connection's lifetime; never read.
    _db: libsql::Database,
    conn: libsql::Connection,
    /// Dedicated connection so concurrent workspace/catalog operations cannot
    /// accidentally participate in a checkpoint transaction on `conn`.
    checkpoint_conn: tokio::sync::Mutex<libsql::Connection>,
    workspace: String,
}

impl LibsqlCheckpointer {
    /// Open (creating if needed) the checkpoint store at
    /// `~/.craftsman/threads.db`, applying the schema.
    pub async fn open_for_workspace(workspace: &std::path::Path) -> Result<Self, String> {
        let path = crate::threads_db::path()
            .await
            .map_err(|error| error.to_string())?;
        let workspace = workspace
            .canonicalize()
            .unwrap_or_else(|_| workspace.to_path_buf())
            .display()
            .to_string();
        Self::open_at_for_workspace(path, workspace).await
    }

    #[cfg(test)]
    async fn list_all_threads_at(path: std::path::PathBuf) -> Result<Vec<ThreadInfo>, String> {
        Ok(Self::list_all_threads_page_at(path, 0, usize::MAX).await?.0)
    }

    #[cfg(test)]
    pub(crate) async fn list_all_threads_page_at(
        path: std::path::PathBuf,
        offset: usize,
        limit: usize,
    ) -> Result<(Vec<ThreadInfo>, bool), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let db = libsql::Builder::new_local(path.to_string_lossy().to_string())
            .build()
            .await
            .map_err(|error| error.to_string())?;
        let init_guard = crate::threads_db::local_database_init_guard().await;
        let conn = db.connect().map_err(|error| error.to_string())?;
        crate::threads_db::configure_local_connection(&conn).await?;
        Self::apply_schema(&conn)
            .await
            .map_err(|error| error.to_string())?;
        Self::retire_legacy_workspace_metadata(&conn)
            .await
            .map_err(|error| error.to_string())?;
        drop(init_guard);
        let requested = limit.max(1);
        let query_limit = requested.saturating_add(1).min(i64::MAX as usize) as i64;
        let mut rows = conn
            .query(
                "SELECT t.id, t.created, t.updated, t.title, COUNT(c.seq), t.workspace
                 FROM threads t LEFT JOIN checkpoints c ON c.thread_id = t.id
                 GROUP BY t.id, t.created, t.updated, t.title, t.workspace
                 ORDER BY t.updated DESC, t.id DESC
                 LIMIT ?1 OFFSET ?2",
                libsql::params![query_limit, offset.min(i64::MAX as usize) as i64],
            )
            .await
            .map_err(|error| error.to_string())?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().await.map_err(|error| error.to_string())? {
            out.push(ThreadInfo {
                id: row.get(0).map_err(|error| error.to_string())?,
                created: row.get(1).map_err(|error| error.to_string())?,
                updated: row.get(2).map_err(|error| error.to_string())?,
                title: row.get(3).map_err(|error| error.to_string())?,
                checkpoints: row.get::<i64>(4).map_err(|error| error.to_string())?.max(0) as u64,
                project_contexts: Vec::new(),
                workspace: row.get(5).map_err(|error| error.to_string())?,
            });
        }
        Self::hydrate_project_contexts(&conn, &mut out)
            .await
            .map_err(|error| error.to_string())?;
        let has_more = out.len() > requested;
        out.truncate(requested);
        Ok((out, has_more))
    }

    /// Path-explicit open — the testable core behind [`open_for_workspace`].
    #[cfg(test)]
    pub(crate) async fn open_at(path: std::path::PathBuf) -> Result<Self, String> {
        Self::open_at_for_workspace(path, current_workspace()).await
    }

    /// The connection this store works through. Tests in other modules read the
    /// database with it instead of opening one more connection.
    #[cfg(test)]
    pub(crate) fn connection(&self) -> &libsql::Connection {
        &self.conn
    }

    pub(crate) async fn open_at_for_workspace(
        path: std::path::PathBuf,
        workspace: String,
    ) -> Result<Self, String> {
        Self::open_explicit(path, workspace, true).await
    }

    /// Headless preparation must not silently retire/claim legacy workspace
    /// metadata. The caller validates the existing schema and selected thread.
    pub(crate) async fn open_at_for_headless(
        path: std::path::PathBuf,
        workspace: String,
    ) -> Result<Self, String> {
        Self::open_explicit(path, workspace, false).await
    }

    async fn open_explicit(
        path: std::path::PathBuf,
        workspace: String,
        retire_legacy: bool,
    ) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let path_str = path.to_string_lossy().to_string();

        let db = libsql::Builder::new_local(path_str)
            .build()
            .await
            .map_err(|e| e.to_string())?;

        let init_guard = crate::threads_db::local_database_init_guard().await;
        let conn = db.connect().map_err(|e| e.to_string())?;
        crate::threads_db::configure_local_connection(&conn).await?;
        // A returned checkpoint commit must ask SQLite to sync its WAL. This
        // remains a local database, not a power-loss/OS-storage guarantee.
        conn.execute("PRAGMA synchronous = FULL", ())
            .await
            .map_err(|e| e.to_string())?;
        let checkpoint_conn = db.connect().map_err(|e| e.to_string())?;
        checkpoint_conn
            .busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|e| e.to_string())?;
        checkpoint_conn
            .execute("PRAGMA synchronous = FULL", ())
            .await
            .map_err(|e| e.to_string())?;
        checkpoint_conn
            .execute("PRAGMA foreign_keys = ON", ())
            .await
            .map_err(|e| e.to_string())?;
        Self::apply_schema(&conn).await.map_err(|e| e.to_string())?;
        if retire_legacy {
            Self::retire_legacy_workspace_metadata(&conn)
                .await
                .map_err(|e| e.to_string())?;
        }
        drop(init_guard);

        Ok(Self {
            _db: db,
            conn,
            checkpoint_conn: tokio::sync::Mutex::new(checkpoint_conn),
            workspace,
        })
    }

    /// Create a fresh scoped thread without overwriting an existing identity.
    /// False means collision; no context/snapshot mutation occurred in that case.
    pub(crate) async fn create_headless_thread(&self, id: &str) -> Result<bool, AgentError> {
        let tx = self.conn.transaction().await.map_err(state_err)?;
        let inserted = tx.execute(
            "INSERT OR IGNORE INTO threads (id, created, updated, title, workspace) VALUES (?1, ?2, ?2, 'New conversation', ?3)",
            libsql::params![id, unix_secs(), self.workspace.clone()],
        ).await.map_err(state_err)?;
        if inserted == 0 {
            return Ok(false);
        }
        tx.execute(
            "INSERT INTO thread_project_contexts (thread_id, path, position) VALUES (?1, ?2, 0)",
            libsql::params![id, self.workspace.clone()],
        )
        .await
        .map_err(state_err)?;
        tx.commit().await.map_err(state_err)?;
        Ok(true)
    }

    /// Create or bring up to date every table this module owns, then record the
    /// schema version if the database has none.
    async fn apply_schema(conn: &libsql::Connection) -> Result<(), libsql::Error> {
        conn.execute("PRAGMA foreign_keys = ON", ()).await?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS threads (
                id TEXT PRIMARY KEY,
                created INTEGER NOT NULL,
                updated INTEGER NOT NULL DEFAULT 0,
                title TEXT NOT NULL,
                workspace TEXT NOT NULL DEFAULT ''
            )",
            (),
        )
        .await?;
        // Existing desktop databases predate workspace scoping. SQLite lacks
        // `ADD COLUMN IF NOT EXISTS`, so duplicate-column errors are the expected
        // idempotent migration path.
        let _ = conn
            .execute(
                "ALTER TABLE threads ADD COLUMN updated INTEGER NOT NULL DEFAULT 0",
                (),
            )
            .await;
        let _ = conn
            .execute(
                "ALTER TABLE threads ADD COLUMN workspace TEXT NOT NULL DEFAULT ''",
                (),
            )
            .await;
        conn.execute("UPDATE threads SET updated = created WHERE updated = 0", ())
            .await?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS threads_workspace_updated ON threads(workspace, updated DESC, id DESC)",
            (),
        ).await?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS checkpoints (
                thread_id TEXT NOT NULL,
                seq INTEGER NOT NULL,
                state_json TEXT NOT NULL,
                suspension_json TEXT,
                PRIMARY KEY (thread_id, seq)
            )",
            (),
        )
        .await?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS workspace_checkpoints (
                id TEXT PRIMARY KEY,
                thread_id TEXT NOT NULL,
                turn_seq INTEGER NOT NULL,
                created INTEGER NOT NULL,
                summary TEXT NOT NULL,
                partial INTEGER NOT NULL DEFAULT 0
            )",
            (),
        )
        .await?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS workspace_checkpoints_thread_created
             ON workspace_checkpoints(thread_id, created DESC, id DESC)",
            (),
        )
        .await?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS workspace_checkpoint_files (
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
            (),
        )
        .await?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS thread_project_contexts (
                thread_id TEXT NOT NULL,
                path TEXT NOT NULL,
                position INTEGER NOT NULL,
                PRIMARY KEY (thread_id, path),
                FOREIGN KEY (thread_id) REFERENCES threads(id) ON DELETE CASCADE
            )",
            (),
        )
        .await?;
        conn.execute(
            "CREATE UNIQUE INDEX IF NOT EXISTS thread_project_contexts_position
             ON thread_project_contexts(thread_id, position)",
            (),
        )
        .await?;
        // Last: the stamp says the layout above is in place.
        crate::threads_db::stamp_schema_version(conn).await?;
        Ok(())
    }

    async fn retire_legacy_workspace_metadata(
        conn: &libsql::Connection,
    ) -> Result<(), libsql::Error> {
        let transaction = conn
            .transaction_with_behavior(libsql::TransactionBehavior::Immediate)
            .await?;
        transaction
            .execute(
                "DELETE FROM workspace_checkpoints
                 WHERE thread_id IN (
                   SELECT t.id FROM threads t
                   LEFT JOIN thread_project_contexts p ON p.thread_id = t.id
                   WHERE t.workspace != '' AND p.thread_id IS NULL
                 )",
                (),
            )
            .await?;
        transaction
            .execute(
                "UPDATE threads SET workspace = ''
                 WHERE workspace != '' AND id NOT IN (
                   SELECT DISTINCT thread_id FROM thread_project_contexts
                 )",
                (),
            )
            .await?;
        transaction.commit().await
    }

    async fn project_contexts_at(
        conn: &libsql::Connection,
        thread_id: &str,
    ) -> Result<Vec<String>, libsql::Error> {
        let mut rows = conn
            .query(
                "SELECT path FROM thread_project_contexts
                 WHERE thread_id = ?1 ORDER BY position ASC",
                libsql::params![thread_id],
            )
            .await?;
        let mut contexts = Vec::new();
        while let Some(row) = rows.next().await? {
            contexts.push(row.get(0)?);
        }
        Ok(contexts)
    }

    async fn hydrate_project_contexts(
        conn: &libsql::Connection,
        threads: &mut [ThreadInfo],
    ) -> Result<(), libsql::Error> {
        for thread in threads {
            thread.project_contexts = Self::project_contexts_at(conn, &thread.id).await?;
        }
        Ok(())
    }

    /// Rename an existing thread, storing the trimmed title.
    ///
    /// This is an app-layer operation rather than part of the framework's
    /// [`Checkpointer`] trait. Replica-backed stores retain the same
    /// best-effort post-write sync behavior used by saves and deletes.
    pub async fn rename_thread(&self, thread_id: &str, title: &str) -> Result<(), AgentError> {
        let title = title.trim();
        if title.is_empty() {
            return Err(AgentError::State(
                "thread title must not be empty".to_string(),
            ));
        }

        let changed = self
            .conn
            .execute(
                "UPDATE threads SET title = ?1, updated = ?2 WHERE id = ?3 AND workspace = ?4",
                libsql::params![title, unix_secs(), thread_id, self.workspace.clone()],
            )
            .await
            .map_err(state_err)?;
        if changed == 0 {
            return Err(AgentError::State(format!("thread not found: {thread_id}")));
        }
        Ok(())
    }

    #[cfg(test)]
    pub async fn rename_thread_global(
        &self,
        thread_id: &str,
        title: &str,
    ) -> Result<(), AgentError> {
        let title = title.trim();
        if title.is_empty() {
            return Err(AgentError::State(
                "thread title must not be empty".to_string(),
            ));
        }
        let changed = self
            .conn
            .execute(
                "UPDATE threads SET title = ?1, updated = ?2 WHERE id = ?3",
                libsql::params![title, unix_secs(), thread_id],
            )
            .await
            .map_err(state_err)?;
        if changed == 0 {
            return Err(AgentError::State(format!("thread not found: {thread_id}")));
        }
        Ok(())
    }

    /// Replace the ordered project directories attached to a conversation.
    /// Message checkpoints remain conversation-owned. File checkpoints are
    /// discarded because their paths were captured against the old context.
    #[cfg(test)]
    pub async fn set_thread_project_contexts(
        &self,
        thread_id: &str,
        project_contexts: &[String],
        default_workspace: &str,
    ) -> Result<Vec<String>, AgentError> {
        let mut normalized = Vec::new();
        for path in project_contexts {
            let path = path.trim();
            if !path.is_empty() && !normalized.iter().any(|existing| existing == path) {
                normalized.push(path.to_string());
            }
        }
        let workspace = normalized
            .first()
            .map(String::as_str)
            .unwrap_or(default_workspace)
            .trim();
        if workspace.is_empty() {
            return Err(AgentError::State(
                "default workspace must not be empty".to_string(),
            ));
        }

        let transaction = self.conn.transaction().await.map_err(state_err)?;
        transaction
            .execute(
                "INSERT INTO threads (id, created, updated, title, workspace)
                 VALUES (?1, ?2, ?3, 'New conversation', ?4)
                 ON CONFLICT(id) DO UPDATE SET workspace = excluded.workspace, updated = excluded.updated",
                libsql::params![thread_id, unix_secs(), unix_secs(), workspace],
            )
            .await
            .map_err(state_err)?;
        transaction
            .execute(
                "DELETE FROM thread_project_contexts WHERE thread_id = ?1",
                libsql::params![thread_id],
            )
            .await
            .map_err(state_err)?;
        for (position, path) in normalized.iter().enumerate() {
            transaction
                .execute(
                    "INSERT INTO thread_project_contexts (thread_id, path, position)
                     VALUES (?1, ?2, ?3)",
                    libsql::params![thread_id, path.clone(), position as i64],
                )
                .await
                .map_err(state_err)?;
        }
        transaction
            .execute(
                "DELETE FROM workspace_checkpoints WHERE thread_id = ?1",
                libsql::params![thread_id],
            )
            .await
            .map_err(state_err)?;
        transaction.commit().await.map_err(state_err)?;
        Ok(normalized)
    }

    /// Fetch only the latest persisted [`AgentState`] for export or preview.
    ///
    /// Unlike [`Checkpointer::load_latest`], this intentionally omits sequence
    /// and suspension metadata so callers do not need to depend on framework
    /// resume details.
    #[cfg(test)]
    pub async fn latest_state_global(
        &self,
        thread_id: &str,
    ) -> Result<Option<AgentState>, AgentError> {
        let mut rows = self
            .conn
            .query(
                "SELECT c.state_json FROM checkpoints c
                 JOIN threads t ON t.id = c.thread_id
                 WHERE c.thread_id = ?1
                 ORDER BY c.seq DESC LIMIT 1",
                libsql::params![thread_id],
            )
            .await
            .map_err(state_err)?;

        let Some(row) = rows.next().await.map_err(state_err)? else {
            return Ok(None);
        };
        let state_json: String = row.get(0).map_err(state_err)?;
        serde_json::from_str(&state_json)
            .map(Some)
            .map_err(state_err)
    }

    pub async fn latest_state(&self, thread_id: &str) -> Result<Option<AgentState>, AgentError> {
        let mut rows = self
            .conn
            .query(
                "SELECT c.state_json FROM checkpoints c
                 JOIN threads t ON t.id = c.thread_id
                 WHERE c.thread_id = ?1 AND t.workspace = ?2
                 ORDER BY c.seq DESC LIMIT 1",
                libsql::params![thread_id, self.workspace.clone()],
            )
            .await
            .map_err(state_err)?;
        let Some(row) = rows.next().await.map_err(state_err)? else {
            return Ok(None);
        };
        let state_json: String = row.get(0).map_err(state_err)?;
        serde_json::from_str(&state_json)
            .map(Some)
            .map_err(state_err)
    }

    #[cfg(test)]
    pub async fn thread_info(&self, thread_id: &str) -> Result<Option<ThreadInfo>, AgentError> {
        let mut rows = self
            .conn
            .query(
                "SELECT t.id, t.created, t.updated, t.title, COUNT(c.seq), t.workspace
                 FROM threads t LEFT JOIN checkpoints c ON c.thread_id = t.id
                 WHERE t.id = ?1
                 GROUP BY t.id, t.created, t.updated, t.title, t.workspace
                 LIMIT 1",
                libsql::params![thread_id],
            )
            .await
            .map_err(state_err)?;
        let Some(row) = rows.next().await.map_err(state_err)? else {
            return Ok(None);
        };
        let mut thread = ThreadInfo {
            id: row.get(0).map_err(state_err)?,
            created: row.get(1).map_err(state_err)?,
            updated: row.get(2).map_err(state_err)?,
            title: row.get(3).map_err(state_err)?,
            checkpoints: row.get::<i64>(4).map_err(state_err)?.max(0) as u64,
            project_contexts: Vec::new(),
            workspace: row.get(5).map_err(state_err)?,
        };
        thread.project_contexts = Self::project_contexts_at(&self.conn, &thread.id)
            .await
            .map_err(state_err)?;
        Ok(Some(thread))
    }

    #[cfg(test)]
    pub async fn list_threads_page(
        &self,
        offset: usize,
        limit: usize,
        exclude_thread_id: Option<&str>,
    ) -> Result<(Vec<ThreadInfo>, bool), AgentError> {
        let requested = limit.max(1);
        let query_limit = requested.saturating_add(1).min(i64::MAX as usize) as i64;
        let query_offset = offset.min(i64::MAX as usize) as i64;
        let mut rows = match exclude_thread_id {
            Some(excluded) => {
                self.conn
                    .query(
                        "SELECT t.id, t.created, t.updated, t.title, COUNT(c.seq), t.workspace
                     FROM threads t LEFT JOIN checkpoints c ON c.thread_id = t.id
                     WHERE t.id != ?1
                     GROUP BY t.id, t.created, t.updated, t.title, t.workspace
                     ORDER BY t.updated DESC, t.id DESC
                     LIMIT ?2 OFFSET ?3",
                        libsql::params![excluded, query_limit, query_offset],
                    )
                    .await
            }
            None => {
                self.conn
                    .query(
                        "SELECT t.id, t.created, t.updated, t.title, COUNT(c.seq), t.workspace
                     FROM threads t LEFT JOIN checkpoints c ON c.thread_id = t.id
                     GROUP BY t.id, t.created, t.updated, t.title, t.workspace
                     ORDER BY t.updated DESC, t.id DESC
                     LIMIT ?1 OFFSET ?2",
                        libsql::params![query_limit, query_offset],
                    )
                    .await
            }
        }
        .map_err(state_err)?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().await.map_err(state_err)? {
            out.push(ThreadInfo {
                id: row.get(0).map_err(state_err)?,
                created: row.get(1).map_err(state_err)?,
                updated: row.get(2).map_err(state_err)?,
                title: row.get(3).map_err(state_err)?,
                checkpoints: row.get::<i64>(4).map_err(state_err)?.max(0) as u64,
                project_contexts: Vec::new(),
                workspace: row.get(5).map_err(state_err)?,
            });
        }
        Self::hydrate_project_contexts(&self.conn, &mut out)
            .await
            .map_err(state_err)?;
        let has_more = out.len() > requested;
        out.truncate(requested);
        Ok((out, has_more))
    }

    pub(crate) async fn save_workspace_checkpoint(
        &self,
        checkpoint: &NewWorkspaceCheckpoint,
    ) -> Result<(), AgentError> {
        let transaction = self.conn.transaction().await.map_err(state_err)?;
        transaction
            .execute(
                "DELETE FROM workspace_checkpoint_files WHERE checkpoint_id = ?1",
                libsql::params![checkpoint.id.clone()],
            )
            .await
            .map_err(state_err)?;
        transaction
            .execute(
                "INSERT INTO workspace_checkpoints
                 (id, thread_id, turn_seq, created, summary, partial)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(id) DO UPDATE SET
                    thread_id = excluded.thread_id,
                    turn_seq = excluded.turn_seq,
                    created = excluded.created,
                    summary = excluded.summary,
                    partial = excluded.partial",
                libsql::params![
                    checkpoint.id.clone(),
                    checkpoint.thread_id.clone(),
                    checkpoint.turn_seq as i64,
                    checkpoint.created,
                    checkpoint.summary.clone(),
                    i64::from(checkpoint.partial),
                ],
            )
            .await
            .map_err(state_err)?;
        for file in &checkpoint.files {
            transaction
                .execute(
                    "INSERT INTO workspace_checkpoint_files
                     (checkpoint_id, path, change_kind, before_hash, after_hash,
                      before_mode, after_mode, before_content, after_content, restorable)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                    libsql::params![
                        checkpoint.id.clone(),
                        file.path.clone(),
                        file.kind.as_str(),
                        file.before_hash.clone(),
                        file.after_hash.clone(),
                        file.before_mode.map(i64::from),
                        file.after_mode.map(i64::from),
                        file.before_content.clone(),
                        file.after_content.clone(),
                        i64::from(file.restorable),
                    ],
                )
                .await
                .map_err(state_err)?;
        }
        transaction
            .execute(
                "DELETE FROM workspace_checkpoints
                 WHERE thread_id = ?1 AND id NOT IN (
                     SELECT id FROM workspace_checkpoints WHERE thread_id = ?2
                     ORDER BY created DESC, id DESC LIMIT ?3
                 )",
                libsql::params![
                    checkpoint.thread_id.clone(),
                    checkpoint.thread_id.clone(),
                    MAX_WORKSPACE_CHECKPOINTS_PER_THREAD,
                ],
            )
            .await
            .map_err(state_err)?;
        transaction.commit().await.map_err(state_err)?;
        Ok(())
    }

    pub(crate) async fn delete_workspace_checkpoint(
        &self,
        checkpoint_id: &str,
    ) -> Result<(), AgentError> {
        self.conn
            .execute(
                "DELETE FROM workspace_checkpoints WHERE id = ?1",
                libsql::params![checkpoint_id],
            )
            .await
            .map_err(state_err)?;
        Ok(())
    }

    pub async fn list_workspace_checkpoints(
        &self,
        thread_id: &str,
    ) -> Result<Vec<WorkspaceCheckpointSummary>, AgentError> {
        let mut rows = self
            .conn
            .query(
                "SELECT w.id, w.turn_seq, w.created, w.summary, w.partial, COUNT(f.path)
                 FROM workspace_checkpoints w
                 LEFT JOIN workspace_checkpoint_files f ON f.checkpoint_id = w.id
                 WHERE w.thread_id = ?1
                 GROUP BY w.id, w.turn_seq, w.created, w.summary, w.partial
                 ORDER BY w.created DESC, w.id DESC",
                libsql::params![thread_id],
            )
            .await
            .map_err(state_err)?;
        let mut checkpoints = Vec::new();
        while let Some(row) = rows.next().await.map_err(state_err)? {
            let turn_seq: i64 = row.get(1).map_err(state_err)?;
            let file_count: i64 = row.get(5).map_err(state_err)?;
            let partial: i64 = row.get(4).map_err(state_err)?;
            checkpoints.push(WorkspaceCheckpointSummary {
                id: row.get(0).map_err(state_err)?,
                turn_seq: turn_seq.max(0) as u64,
                created: row.get(2).map_err(state_err)?,
                summary: row.get(3).map_err(state_err)?,
                file_count: file_count.max(0) as usize,
                partial: partial != 0,
            });
        }
        Ok(checkpoints)
    }

    pub(crate) async fn load_workspace_checkpoint(
        &self,
        thread_id: &str,
        checkpoint_id: Option<&str>,
    ) -> Result<Option<StoredWorkspaceCheckpoint>, AgentError> {
        let mut rows = match checkpoint_id {
            Some(id) => {
                self.conn
                    .query(
                        "SELECT id, turn_seq, created, summary, partial
                         FROM workspace_checkpoints WHERE thread_id = ?1 AND id = ?2 LIMIT 1",
                        libsql::params![thread_id, id],
                    )
                    .await
            }
            None => {
                self.conn
                    .query(
                        "SELECT id, turn_seq, created, summary, partial
                         FROM workspace_checkpoints WHERE thread_id = ?1
                         ORDER BY created DESC, id DESC LIMIT 1",
                        libsql::params![thread_id],
                    )
                    .await
            }
        }
        .map_err(state_err)?;
        let Some(row) = rows.next().await.map_err(state_err)? else {
            return Ok(None);
        };
        let id: String = row.get(0).map_err(state_err)?;
        let turn_seq: i64 = row.get(1).map_err(state_err)?;
        let partial: i64 = row.get(4).map_err(state_err)?;
        let mut file_rows = self
            .conn
            .query(
                "SELECT path, change_kind, before_hash, after_hash, before_mode,
                        after_mode, before_content, after_content, restorable
                 FROM workspace_checkpoint_files WHERE checkpoint_id = ?1 ORDER BY path",
                libsql::params![id.clone()],
            )
            .await
            .map_err(state_err)?;
        let mut files = Vec::new();
        while let Some(file) = file_rows.next().await.map_err(state_err)? {
            let kind: String = file.get(1).map_err(state_err)?;
            let before_mode: Option<i64> = file.get(4).map_err(state_err)?;
            let after_mode: Option<i64> = file.get(5).map_err(state_err)?;
            let restorable: i64 = file.get(8).map_err(state_err)?;
            files.push(WorkspaceCheckpointFile {
                path: file.get(0).map_err(state_err)?,
                kind: WorkspaceChangeKind::parse(&kind)?,
                before_hash: file.get(2).map_err(state_err)?,
                after_hash: file.get(3).map_err(state_err)?,
                before_mode: before_mode.map(|mode| mode as u32),
                after_mode: after_mode.map(|mode| mode as u32),
                before_content: file.get(6).map_err(state_err)?,
                after_content: file.get(7).map_err(state_err)?,
                restorable: restorable != 0,
            });
        }
        Ok(Some(StoredWorkspaceCheckpoint {
            summary: WorkspaceCheckpointSummary {
                id,
                turn_seq: turn_seq.max(0) as u64,
                created: row.get(2).map_err(state_err)?,
                summary: row.get(3).map_err(state_err)?,
                file_count: files.len(),
                partial: partial != 0,
            },
            files,
        }))
    }

    /// Generate a fresh thread id: `<unix-secs>-<pid hex>-<4 hex counter>`.
    /// The PID disambiguates two `craftsman-rs` processes started in the same
    /// wall-clock second — a time-scoped process-local counter alone would
    /// mint the same id in both, and with `INSERT OR REPLACE` on
    /// `(thread_id, seq)` the two conversations would clobber each other's
    /// checkpoints.
    pub fn new_thread_id() -> String {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed) & 0xffff;
        format!("{}-{:x}-{:04x}", unix_secs(), std::process::id(), n)
    }
}

#[async_trait]
impl Checkpointer for LibsqlCheckpointer {
    async fn save(
        &self,
        thread_id: &str,
        seq: u64,
        state: &AgentState,
        suspension: Option<&Suspension>,
    ) -> Result<(), AgentError> {
        let seq = i64::try_from(seq).map_err(|_| {
            AgentError::State("checkpoint sequence exceeds the storage range".into())
        })?;
        // Serialize before any database mutation. Metadata, state and pruning
        // share one commit so a failed save cannot partially advance a thread.
        let state_json = serde_json::to_string(state).map_err(state_err)?;
        let suspension_json = match suspension {
            Some(s) => Some(serde_json::to_string(s).map_err(state_err)?),
            None => None,
        };
        let connection = self.checkpoint_conn.lock().await;
        let transaction = connection
            .transaction_with_behavior(libsql::TransactionBehavior::Immediate)
            .await
            .map_err(state_err)?;
        // Insert the thread row once (title/created fixed at creation).
        transaction
            .execute(
                "INSERT INTO threads (id, created, updated, title, workspace) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(id) DO UPDATE SET
                    updated = excluded.updated,
                    title = CASE
                        WHEN threads.title = 'New conversation' AND excluded.title != '' THEN excluded.title
                        ELSE threads.title
                    END",
                libsql::params![thread_id, unix_secs(), unix_secs(), thread_title(state), self.workspace.clone()],
            )
            .await
            .map_err(state_err)?;

        if !is_general_chat_workspace(&self.workspace) {
            transaction
                .execute(
                    "INSERT OR IGNORE INTO thread_project_contexts (thread_id, path, position)
                     VALUES (?1, ?2, 0)",
                    libsql::params![thread_id, self.workspace.clone()],
                )
                .await
                .map_err(state_err)?;
        }

        transaction
            .execute(
                "INSERT OR REPLACE INTO checkpoints (thread_id, seq, state_json, suspension_json)
                 VALUES (?1, ?2, ?3, ?4)",
                libsql::params![thread_id, seq, state_json, suspension_json],
            )
            .await
            .map_err(state_err)?;

        // Prune all but the newest N checkpoints for this thread.
        transaction
            .execute(
                "DELETE FROM checkpoints
                 WHERE thread_id = ?1 AND seq NOT IN (
                     SELECT seq FROM checkpoints WHERE thread_id = ?2
                     ORDER BY seq DESC LIMIT ?3
                 )",
                libsql::params![thread_id, thread_id, MAX_CHECKPOINTS_PER_THREAD],
            )
            .await
            .map_err(state_err)?;
        transaction.commit().await.map_err(state_err)?;
        Ok(())
    }

    async fn load_latest(
        &self,
        thread_id: &str,
    ) -> Result<Option<(u64, AgentState, Option<Suspension>)>, AgentError> {
        let mut rows = self
            .conn
            .query(
                "SELECT c.seq, c.state_json, c.suspension_json FROM checkpoints c
                 JOIN threads t ON t.id = c.thread_id
                 WHERE c.thread_id = ?1 AND t.workspace = ?2
                 ORDER BY c.seq DESC LIMIT 1",
                libsql::params![thread_id, self.workspace.clone()],
            )
            .await
            .map_err(state_err)?;

        let Some(row) = rows.next().await.map_err(state_err)? else {
            return Ok(None);
        };

        let seq: i64 = row.get(0).map_err(state_err)?;
        let state_json: String = row.get(1).map_err(state_err)?;
        let suspension_json: Option<String> = row.get(2).map_err(state_err)?;

        let state: AgentState = serde_json::from_str(&state_json).map_err(state_err)?;
        let suspension = match suspension_json {
            Some(s) => Some(serde_json::from_str::<Suspension>(&s).map_err(state_err)?),
            None => None,
        };
        let seq = u64::try_from(seq)
            .map_err(|_| AgentError::State("checkpoint sequence is invalid".into()))?;
        Ok(Some((seq, state, suspension)))
    }

    async fn list_threads(&self) -> Result<Vec<ThreadInfo>, AgentError> {
        let mut rows = self
            .conn
            .query(
                "SELECT t.id, t.created, t.updated, t.title, COUNT(c.seq), t.workspace
                 FROM threads t LEFT JOIN checkpoints c ON c.thread_id = t.id
                 WHERE t.workspace = ?1
                 GROUP BY t.id, t.created, t.updated, t.title, t.workspace
                 ORDER BY t.updated DESC, t.id DESC",
                libsql::params![self.workspace.clone()],
            )
            .await
            .map_err(state_err)?;

        let mut out = Vec::new();
        while let Some(row) = rows.next().await.map_err(state_err)? {
            let id: String = row.get(0).map_err(state_err)?;
            let created: i64 = row.get(1).map_err(state_err)?;
            let updated: i64 = row.get(2).map_err(state_err)?;
            let title: String = row.get(3).map_err(state_err)?;
            let count: i64 = row.get(4).map_err(state_err)?;
            let workspace: String = row.get(5).map_err(state_err)?;
            out.push(ThreadInfo {
                id,
                created,
                updated,
                title,
                checkpoints: count.max(0) as u64,
                project_contexts: Vec::new(),
                workspace,
            });
        }
        Self::hydrate_project_contexts(&self.conn, &mut out)
            .await
            .map_err(state_err)?;
        Ok(out)
    }

    async fn delete_thread(&self, thread_id: &str) -> Result<(), AgentError> {
        let mut ownership = self
            .conn
            .query(
                "SELECT 1 FROM threads WHERE id = ?1 AND workspace = ?2 LIMIT 1",
                libsql::params![thread_id, self.workspace.clone()],
            )
            .await
            .map_err(state_err)?;
        if ownership.next().await.map_err(state_err)?.is_none() {
            return Err(AgentError::State(format!(
                "thread not found in this workspace: {thread_id}"
            )));
        }
        let transaction = self.conn.transaction().await.map_err(state_err)?;
        transaction
            .execute(
                "DELETE FROM workspace_checkpoints WHERE thread_id = ?1",
                libsql::params![thread_id],
            )
            .await
            .map_err(state_err)?;
        transaction
            .execute(
                "DELETE FROM checkpoints WHERE thread_id = ?1",
                libsql::params![thread_id],
            )
            .await
            .map_err(state_err)?;
        transaction
            .execute(
                "DELETE FROM thread_project_contexts WHERE thread_id = ?1",
                libsql::params![thread_id],
            )
            .await
            .map_err(state_err)?;
        transaction
            .execute(
                "DELETE FROM threads WHERE id = ?1 AND workspace = ?2",
                libsql::params![thread_id, self.workspace.clone()],
            )
            .await
            .map_err(state_err)?;
        transaction.commit().await.map_err(state_err)?;
        if thread_id != "." && thread_id != ".." && !thread_id.contains(['/', '\\']) {
            let journal = crate::config::config_dir()
                .join("sessions-rs")
                .join(format!("{thread_id}.jsonl"));
            if let Err(error) = std::fs::remove_file(&journal) {
                if error.kind() != std::io::ErrorKind::NotFound {
                    return Err(AgentError::State(format!(
                        "deleted session database rows but could not remove {}: {error}",
                        journal.display()
                    )));
                }
            }
        }
        Ok(())
    }
}

/// Append-only JSONL journal under `~/.craftsman/sessions-rs/{thread_id}.jsonl`.
/// Infallible from the caller's perspective: I/O errors are reported once then
/// silently dropped so journaling never breaks a turn.
pub struct JsonlJournal {
    /// Warn only once if a journal write fails.
    warned: Once,
}

impl JsonlJournal {
    pub fn new() -> Self {
        Self {
            warned: Once::new(),
        }
    }
}

impl Default for JsonlJournal {
    fn default() -> Self {
        Self::new()
    }
}

impl TranscriptSink for JsonlJournal {
    fn append(&self, thread_id: &str, event: &TranscriptEvent<'_>) {
        let dir = crate::config::config_dir().join("sessions-rs");
        if let Err(e) = write_event(&dir, thread_id, event) {
            self.warned.call_once(|| {
                eprintln!("craftsman-rs: transcript journal write failed (continuing): {e}");
            });
        }
    }
}

fn write_event(
    dir: &std::path::Path,
    thread_id: &str,
    event: &TranscriptEvent<'_>,
) -> std::io::Result<()> {
    use std::io::Write;

    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }

    // Restamp with the app clock — the framework always passes ts: 0.
    let line = serde_json::json!({
        "ts": unix_secs(),
        "kind": event.kind,
        "data": event.data,
    });
    let mut body = serde_json::to_vec(&line)?;
    body.push(b'\n');

    let path = dir.join(format!("{thread_id}.jsonl"));
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    f.write_all(&body)
}

/// Test-only journal write to an explicit dir (avoids `config_dir()`/HOME).
#[cfg(test)]
fn journal_append_at(dir: &std::path::Path, thread_id: &str, event: &TranscriptEvent<'_>) {
    write_event(dir, thread_id, event).unwrap();
}

#[cfg(test)]
fn current_workspace() -> String {
    std::env::current_dir()
        .ok()
        .and_then(|path| path.canonicalize().ok().or(Some(path)))
        .map(|path| path.display().to_string())
        .unwrap_or_default()
}

fn is_general_chat_workspace(path: &str) -> bool {
    path.replace('\\', "/")
        .trim_end_matches('/')
        .ends_with("/.craftsman/general-chat")
}

#[cfg(test)]
#[path = "checkpoints/recovery_tests.rs"]
mod recovery_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::threads_db::{Probe, dump, open_plain, user_version};
    use craftsman_harness::interrupt::{HitlRequest, SuspensionFrame};
    use craftsman_harness::state::ToolCall;

    /// Open a checkpointer at a unique temp path (avoids the process-global
    /// HOME env var, so tests stay parallel-safe). Returns the guard too so the
    /// dir outlives the store.
    async fn open_tmp() -> (tempfile::TempDir, LibsqlCheckpointer) {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("threads.db");
        let cp = LibsqlCheckpointer::open_at(path).await.unwrap();
        (tmp, cp)
    }

    fn state_with_user(msg: &str) -> AgentState {
        let mut s = AgentState::default();
        s.messages.push(Msg::user(msg));
        s
    }

    fn suspension() -> Suspension {
        Suspension {
            frames: vec![SuspensionFrame {
                agent_name: "main".into(),
            }],
            request: HitlRequest {
                tool_call: ToolCall {
                    id: "c1".into(),
                    name: "execute".into(),
                    args: serde_json::json!({"command": "ls"}),
                },
                summary: "execute ls".into(),
            },
        }
    }

    #[tokio::test]
    async fn save_then_load_latest_round_trips() {
        let (_tmp, cp) = open_tmp().await;

        assert!(cp.load_latest("t1").await.unwrap().is_none());

        cp.save("t1", 0, &state_with_user("hello"), None)
            .await
            .unwrap();
        let mut s2 = state_with_user("hello");
        s2.messages.push(Msg::assistant("hi there"));
        cp.save("t1", 1, &s2, Some(&suspension())).await.unwrap();

        let (seq, state, susp) = cp.load_latest("t1").await.unwrap().unwrap();
        assert_eq!(seq, 1);
        assert_eq!(state.messages.len(), 2);
        assert_eq!(susp.unwrap().request.tool_call.name, "execute");
    }

    #[tokio::test]
    async fn thread_title_is_first_user_message_truncated() {
        let (_tmp, cp) = open_tmp().await;

        let long = "word ".repeat(40);
        cp.save("t1", 0, &state_with_user(&long), None)
            .await
            .unwrap();

        let threads = cp.list_threads().await.unwrap();
        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0].id, "t1");
        assert!(threads[0].title.chars().count() <= 60);
        assert!(threads[0].title.ends_with('…'));
        assert_eq!(threads[0].checkpoints, 1);
    }

    #[tokio::test]
    async fn title_and_created_fixed_on_first_insert() {
        let (_tmp, cp) = open_tmp().await;

        cp.save("t1", 0, &state_with_user("original title"), None)
            .await
            .unwrap();
        cp.save("t1", 1, &state_with_user("different later message"), None)
            .await
            .unwrap();

        let threads = cp.list_threads().await.unwrap();
        assert_eq!(threads[0].title, "original title");
        assert_eq!(threads[0].checkpoints, 2);
    }

    #[tokio::test]
    async fn save_and_rename_refresh_the_updated_timestamp() {
        let (_tmp, cp) = open_tmp().await;
        cp.save("t1", 0, &state_with_user("original"), None)
            .await
            .unwrap();
        cp.conn
            .execute(
                "UPDATE threads SET created = 10, updated = 10 WHERE id = 't1'",
                (),
            )
            .await
            .unwrap();
        cp.save("t1", 1, &state_with_user("later"), None)
            .await
            .unwrap();
        let after_save = cp.list_threads().await.unwrap().remove(0);
        assert_eq!(after_save.created, 10);
        assert!(after_save.updated > 10);
        cp.conn
            .execute("UPDATE threads SET updated = 20 WHERE id = 't1'", ())
            .await
            .unwrap();
        cp.rename_thread("t1", "renamed").await.unwrap();
        assert!(cp.list_threads().await.unwrap()[0].updated > 20);
    }

    #[tokio::test]
    async fn list_threads_newest_created_first() {
        let (_tmp, cp) = open_tmp().await;

        // Force distinct created stamps by inserting rows directly.
        cp.conn
            .execute(
                "INSERT INTO threads (id, created, updated, title, workspace) VALUES ('old', 100, 100, 'old', ?1)",
                libsql::params![cp.workspace.clone()],
            )
            .await
            .unwrap();
        cp.conn
            .execute(
                "INSERT INTO threads (id, created, updated, title, workspace) VALUES ('new', 200, 200, 'new', ?1)",
                libsql::params![cp.workspace.clone()],
            )
            .await
            .unwrap();

        let threads = cp.list_threads().await.unwrap();
        assert_eq!(threads[0].id, "new");
        assert_eq!(threads[1].id, "old");
    }

    #[tokio::test]
    async fn legacy_thread_schema_migrates_to_global_conversations_without_inventing_context() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("legacy.db");
        let db = libsql::Builder::new_local(path.to_string_lossy().to_string())
            .build()
            .await
            .unwrap();
        let conn = db.connect().unwrap();
        conn.execute(
            "CREATE TABLE threads (id TEXT PRIMARY KEY, created INTEGER NOT NULL, title TEXT NOT NULL)",
            (),
        ).await.unwrap();
        conn.execute(
            "CREATE TABLE checkpoints (thread_id TEXT NOT NULL, seq INTEGER NOT NULL, state_json TEXT NOT NULL, suspension_json TEXT, PRIMARY KEY (thread_id, seq))",
            (),
        ).await.unwrap();
        conn.execute(
            "INSERT INTO threads (id, created, title) VALUES ('legacy', 1, 'legacy')",
            (),
        )
        .await
        .unwrap();
        // 0.4.0 and earlier left no version behind.
        assert_eq!(user_version(&conn).await.unwrap(), 0);
        drop(conn);
        drop(db);

        let first =
            LibsqlCheckpointer::open_at_for_workspace(path.clone(), "/workspace/new".into())
                .await
                .unwrap();
        // Columns added to an old table, then stamped like any other database.
        assert_eq!(user_version(&first.conn).await.unwrap(), 1);
        let migrated = LibsqlCheckpointer::list_all_threads_at(path.clone())
            .await
            .unwrap();
        assert_eq!(migrated.len(), 1);
        assert_eq!(migrated[0].id, "legacy");
        assert_eq!(migrated[0].updated, migrated[0].created);
        assert!(migrated[0].project_contexts.is_empty());
        first
            .conn
            .execute(
                "UPDATE threads SET workspace = '/old/implementation/workspace' WHERE id = 'legacy'",
                (),
            )
            .await
            .unwrap();
        first
            .conn
            .execute(
                "INSERT INTO workspace_checkpoints
                 (id, thread_id, turn_seq, created, summary, partial)
                 VALUES ('legacy-files', 'legacy', 1, 1, 'unsafe legacy root', 0)",
                (),
            )
            .await
            .unwrap();

        let other =
            LibsqlCheckpointer::open_at_for_workspace(path.clone(), "/workspace/other".into())
                .await
                .unwrap();
        let rows = LibsqlCheckpointer::list_all_threads_at(path).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].project_contexts.is_empty());
        assert_eq!(rows[0].workspace, "");
        assert!(
            other
                .list_workspace_checkpoints("legacy")
                .await
                .unwrap()
                .is_empty()
        );
        assert!(other.load_latest("legacy").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn conversations_are_global_while_project_context_remains_thread_metadata() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("threads.db");
        let alpha =
            LibsqlCheckpointer::open_at_for_workspace(path.clone(), "/workspace/alpha".into())
                .await
                .unwrap();
        alpha
            .save("alpha-thread", 0, &state_with_user("alpha"), None)
            .await
            .unwrap();
        let beta = LibsqlCheckpointer::open_at_for_workspace(path, "/workspace/beta".into())
            .await
            .unwrap();
        beta.save("beta-thread", 0, &state_with_user("beta"), None)
            .await
            .unwrap();

        let scoped_ids = beta
            .list_threads()
            .await
            .unwrap()
            .into_iter()
            .map(|thread| thread.id)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(scoped_ids, ["beta-thread".to_string()].into());
        assert!(beta.load_latest("alpha-thread").await.unwrap().is_none());
        assert!(
            beta.rename_thread("alpha-thread", "wrong project")
                .await
                .is_err()
        );
        beta.rename_thread_global("alpha-thread", "renamed globally")
            .await
            .unwrap();
        assert_eq!(
            alpha
                .thread_info("alpha-thread")
                .await
                .unwrap()
                .unwrap()
                .title,
            "renamed globally"
        );
        assert_eq!(
            alpha
                .thread_info("alpha-thread")
                .await
                .unwrap()
                .unwrap()
                .project_contexts,
            vec!["/workspace/alpha"]
        );
    }

    #[tokio::test]
    async fn replacing_project_contexts_preserves_chat_and_drops_file_checkpoints() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("threads.db");
        let store = LibsqlCheckpointer::open_at_for_workspace(path, "/workspace/alpha".into())
            .await
            .unwrap();
        store
            .save("thread", 7, &state_with_user("keep this chat"), None)
            .await
            .unwrap();
        store
            .conn
            .execute(
                "INSERT INTO workspace_checkpoints
                 (id, thread_id, turn_seq, created, summary, partial)
                 VALUES ('workspace-snapshot', 'thread', 7, 1, 'old project', 0)",
                (),
            )
            .await
            .unwrap();

        let contexts = store
            .set_thread_project_contexts(
                "thread",
                &["/workspace/beta".into(), "/workspace/shared".into()],
                "/general",
            )
            .await
            .unwrap();
        assert_eq!(contexts, vec!["/workspace/beta", "/workspace/shared"]);
        let state = store.latest_state_global("thread").await.unwrap().unwrap();
        match state.messages.last().unwrap() {
            Msg::User { content } => assert_eq!(content, "keep this chat"),
            _ => panic!("expected preserved user message"),
        }
        assert!(
            store
                .list_workspace_checkpoints("thread")
                .await
                .unwrap()
                .is_empty()
        );
        let info = store.thread_info("thread").await.unwrap().unwrap();
        assert_eq!(info.workspace, "/workspace/beta");
        assert_eq!(info.project_contexts, contexts);
    }

    #[tokio::test]
    async fn unified_catalog_keeps_ordered_project_contexts() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("threads.db");
        let alpha =
            LibsqlCheckpointer::open_at_for_workspace(path.clone(), "/workspace/alpha".into())
                .await
                .unwrap();
        alpha
            .save("alpha-thread", 1, &state_with_user("Alpha task"), None)
            .await
            .unwrap();
        alpha
            .set_thread_project_contexts(
                "alpha-thread",
                &["/workspace/alpha".into(), "/workspace/shared".into()],
                "/general",
            )
            .await
            .unwrap();
        let general_workspace = dirs::home_dir()
            .unwrap()
            .join(".craftsman")
            .join("general-chat")
            .display()
            .to_string();
        let general = LibsqlCheckpointer::open_at_for_workspace(path.clone(), general_workspace)
            .await
            .unwrap();
        general
            .save("general-thread", 1, &state_with_user("General chat"), None)
            .await
            .unwrap();

        let catalog = LibsqlCheckpointer::list_all_threads_at(path).await.unwrap();
        let alpha = catalog
            .iter()
            .find(|thread| thread.id == "alpha-thread")
            .unwrap();
        assert_eq!(
            alpha.project_contexts,
            vec!["/workspace/alpha", "/workspace/shared"]
        );
        let general = catalog
            .iter()
            .find(|thread| thread.id == "general-thread")
            .unwrap();
        assert!(general.project_contexts.is_empty());
    }

    #[tokio::test]
    async fn prune_keeps_only_newest_50() {
        let (_tmp, cp) = open_tmp().await;

        for seq in 0..60u64 {
            cp.save("t1", seq, &state_with_user("m"), None)
                .await
                .unwrap();
        }
        let threads = cp.list_threads().await.unwrap();
        assert_eq!(threads[0].checkpoints, 50);

        // Latest survives; the oldest pruned seq no longer loads as latest.
        let (seq, _, _) = cp.load_latest("t1").await.unwrap().unwrap();
        assert_eq!(seq, 59);
    }

    #[tokio::test]
    async fn delete_thread_removes_thread_and_checkpoints() {
        let (_tmp, cp) = open_tmp().await;

        cp.save("t1", 0, &state_with_user("a"), None).await.unwrap();
        cp.save("t2", 0, &state_with_user("b"), None).await.unwrap();
        cp.delete_thread("t1").await.unwrap();

        assert!(cp.load_latest("t1").await.unwrap().is_none());
        let threads = cp.list_threads().await.unwrap();
        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0].id, "t2");
    }

    #[tokio::test]
    async fn rename_thread_trims_and_updates_title() {
        let (_tmp, cp) = open_tmp().await;

        cp.save("t1", 0, &state_with_user("original"), None)
            .await
            .unwrap();
        cp.rename_thread("t1", "  renamed thread  ").await.unwrap();

        let threads = cp.list_threads().await.unwrap();
        assert_eq!(threads[0].title, "renamed thread");
    }

    #[tokio::test]
    async fn rename_thread_rejects_blank_title_and_missing_thread() {
        let (_tmp, cp) = open_tmp().await;
        cp.save("t1", 0, &state_with_user("original"), None)
            .await
            .unwrap();

        let blank = cp.rename_thread("t1", "  \t\n ").await.unwrap_err();
        assert!(blank.to_string().contains("must not be empty"));
        assert_eq!(cp.list_threads().await.unwrap()[0].title, "original");

        let missing = cp.rename_thread("missing", "title").await.unwrap_err();
        assert!(missing.to_string().contains("thread not found"));
    }

    #[tokio::test]
    async fn latest_state_returns_latest_without_resume_metadata() {
        let (_tmp, cp) = open_tmp().await;

        assert!(cp.latest_state("missing").await.unwrap().is_none());

        cp.save("t1", 0, &state_with_user("first"), None)
            .await
            .unwrap();
        let mut latest = state_with_user("first");
        latest.messages.push(Msg::assistant("latest"));
        cp.save("t1", 1, &latest, Some(&suspension()))
            .await
            .unwrap();

        let exported = cp.latest_state("t1").await.unwrap().unwrap();
        assert_eq!(exported.messages.len(), 2);
        assert!(matches!(
            &exported.messages[1],
            Msg::Assistant { content, .. } if content == "latest"
        ));
    }

    #[tokio::test]
    async fn persists_across_reopen() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("threads.db");
        {
            let cp = LibsqlCheckpointer::open_at(path.clone()).await.unwrap();
            cp.save("t1", 0, &state_with_user("persisted"), None)
                .await
                .unwrap();
        }
        let cp2 = LibsqlCheckpointer::open_at(path).await.unwrap();
        let (_, state, _) = cp2.load_latest("t1").await.unwrap().unwrap();
        match &state.messages[0] {
            Msg::User { content } => assert_eq!(content, "persisted"),
            _ => panic!("expected user message"),
        }
    }

    /// Write a populated database, then reset its version to 0 so it looks as
    /// 0.4.0 left it. Returns a dump of everything it holds.
    async fn populate_unstamped(path: &std::path::Path) -> Vec<String> {
        let cp = LibsqlCheckpointer::open_at(path.to_path_buf())
            .await
            .unwrap();
        cp.save("t1", 0, &state_with_user("first"), None)
            .await
            .unwrap();
        let mut later = state_with_user("first");
        later.messages.push(Msg::assistant("reply"));
        cp.save("t1", 1, &later, Some(&suspension())).await.unwrap();
        cp.save("t2", 0, &state_with_user("second"), None)
            .await
            .unwrap();
        cp.save_workspace_checkpoint(&NewWorkspaceCheckpoint {
            id: "wc-1".into(),
            thread_id: "t1".into(),
            turn_seq: 1,
            created: 5,
            summary: "edit".into(),
            partial: false,
            files: vec![WorkspaceCheckpointFile {
                path: "a.txt".into(),
                kind: WorkspaceChangeKind::Modified,
                before_hash: Some("h0".into()),
                after_hash: Some("h1".into()),
                before_mode: Some(0o644),
                after_mode: Some(0o644),
                before_content: Some(b"old".to_vec()),
                after_content: None,
                restorable: true,
            }],
        })
        .await
        .unwrap();
        cp.conn
            .execute("PRAGMA user_version = 0", ())
            .await
            .unwrap();
        // As 0.4.0 left it: committed data, no version.
        assert_eq!(user_version(&cp.conn).await.unwrap(), 0);
        dump(&cp.conn).await
    }

    #[tokio::test]
    async fn fresh_database_is_stamped_with_version_one() {
        let (tmp, cp) = open_tmp().await;
        assert_eq!(user_version(&cp.conn).await.unwrap(), 1);
        // Committed, so a second connection sees it too.
        let outside = Probe::open(&tmp.path().join("threads.db")).await;
        assert_eq!(outside.user_version().await, 1);
    }

    #[tokio::test]
    async fn unstamped_database_is_stamped_on_open_without_touching_its_data() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("threads.db");
        let before = populate_unstamped(&path).await;

        let reopened = LibsqlCheckpointer::open_at(path.clone()).await.unwrap();
        assert_eq!(user_version(&reopened.conn).await.unwrap(), 1);
        // Same tables, columns, indexes and rows: only the version moved.
        assert_eq!(dump(&reopened.conn).await, before);
        let (seq, state, suspension) = reopened.load_latest("t1").await.unwrap().unwrap();
        assert_eq!(seq, 1);
        assert_eq!(state.messages.len(), 2);
        assert!(suspension.is_some());
    }

    #[tokio::test]
    async fn catalog_listing_stamps_an_unstamped_database_too() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("threads.db");
        let before = populate_unstamped(&path).await;
        // The listing opens its own connection and drops it, so the file is read
        // from outside, before and after.
        let outside = Probe::open(&path).await;
        assert_eq!(outside.user_version().await, 0);

        let threads = LibsqlCheckpointer::list_all_threads_at(path.clone())
            .await
            .unwrap();
        assert_eq!(threads.len(), 2);
        assert_eq!(outside.user_version().await, 1);
        assert_eq!(outside.dump().await, before);
    }

    #[tokio::test]
    async fn stamped_database_keeps_its_version_across_opens() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("threads.db");
        let mut cp = LibsqlCheckpointer::open_at(path.clone()).await.unwrap();
        // 1 is what the first open stamped; 2 stands for a database a newer
        // binary already raised.
        for existing in [1, 2] {
            cp.conn
                .execute(&format!("PRAGMA user_version = {existing}"), ())
                .await
                .unwrap();
            drop(cp);

            cp = LibsqlCheckpointer::open_at(path.clone()).await.unwrap();
            assert_eq!(user_version(&cp.conn).await.unwrap(), existing);
            LibsqlCheckpointer::list_all_threads_at(path.clone())
                .await
                .unwrap();
            // The listing's connection is another one: what it left is committed.
            assert_eq!(user_version(&cp.conn).await.unwrap(), existing);
        }
    }

    /// The stamp comes last, so an `apply_schema` that fails part-way leaves the
    /// version at 0 and a partial layout behind. The next call (the next launch,
    /// or a migrator, which treats 0 as unknown) completes the layout and only
    /// then stamps it.
    #[tokio::test]
    async fn a_failed_apply_schema_leaves_the_version_at_zero_until_a_retry_finishes() {
        let tmp = tempfile::tempdir().unwrap();
        let (_db, conn) = open_plain(&tmp.path().join("threads.db")).await;
        // An index named like the last table apply_schema creates: SQLite refuses
        // that CREATE TABLE (even with IF NOT EXISTS) after the tables before it
        // are in place.
        conn.execute("CREATE TABLE blocker (x)", ()).await.unwrap();
        conn.execute("CREATE INDEX thread_project_contexts ON blocker (x)", ())
            .await
            .unwrap();

        let error = LibsqlCheckpointer::apply_schema(&conn).await.unwrap_err();
        assert!(
            error.to_string().contains("thread_project_contexts"),
            "{error}"
        );
        assert_eq!(user_version(&conn).await.unwrap(), 0);
        let partial = dump(&conn).await;
        assert!(
            partial
                .iter()
                .any(|line| line.starts_with("table threads:"))
        );
        assert!(
            partial
                .iter()
                .any(|line| line.starts_with("table workspace_checkpoint_files:"))
        );
        assert!(
            !partial
                .iter()
                .any(|line| line.starts_with("table thread_project_contexts:"))
        );

        conn.execute("DROP INDEX thread_project_contexts", ())
            .await
            .unwrap();
        LibsqlCheckpointer::apply_schema(&conn).await.unwrap();
        assert_eq!(user_version(&conn).await.unwrap(), 1);
        assert!(
            dump(&conn)
                .await
                .iter()
                .any(|line| line.starts_with("table thread_project_contexts:"))
        );
    }

    #[test]
    fn new_thread_id_is_unique_and_formatted() {
        let a = LibsqlCheckpointer::new_thread_id();
        let b = LibsqlCheckpointer::new_thread_id();
        assert_ne!(a, b);
        // Format: <unix-secs>-<pid hex>-<4 hex counter>. The PID keeps two
        // processes started the same second from minting colliding ids.
        let parts: Vec<&str> = a.split('-').collect();
        assert_eq!(parts.len(), 3, "expected secs-pid-counter, got: {a}");
        assert!(parts[0].parse::<i64>().is_ok());
        assert_eq!(
            u64::from_str_radix(parts[1], 16).unwrap(),
            std::process::id() as u64
        );
        assert_eq!(parts[2].len(), 4);
        assert!(u16::from_str_radix(parts[2], 16).is_ok());
    }

    #[test]
    fn journal_appends_restamped_lines() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("sessions-rs");

        journal_append_at(
            &dir,
            "t1",
            &TranscriptEvent {
                ts: 0,
                kind: "user",
                data: serde_json::json!({"text": "hi"}),
            },
        );
        journal_append_at(
            &dir,
            "t1",
            &TranscriptEvent {
                ts: 0,
                kind: "assistant",
                data: serde_json::json!({"text": "yo"}),
            },
        );

        let path = dir.join("t1.jsonl");
        let body = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines.len(), 2);
        let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(first["kind"], "user");
        assert_eq!(first["data"]["text"], "hi");
        // ts was restamped away from the framework's 0.
        assert!(first["ts"].as_i64().unwrap() > 0);
    }

    #[tokio::test]
    async fn local_thread_database_uses_wal_for_concurrent_desktop_access() {
        let (_tmp, cp) = open_tmp().await;
        let mut rows = cp.conn.query("PRAGMA journal_mode", ()).await.unwrap();
        let mode: String = rows.next().await.unwrap().unwrap().get(0).unwrap();
        assert_eq!(mode.to_ascii_lowercase(), "wal");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn concurrent_catalog_initialization_does_not_lock() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("threads.db");
        let seed =
            LibsqlCheckpointer::open_at_for_workspace(path.clone(), "/workspace/seed".into())
                .await
                .unwrap();
        seed.save("seed-thread", 0, &state_with_user("seed"), None)
            .await
            .unwrap();
        drop(seed);

        for round in 0..20 {
            let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(16));
            let mut tasks = Vec::new();
            for index in 0..16 {
                let barrier = barrier.clone();
                let path = path.clone();
                tasks.push(tokio::spawn(async move {
                    barrier.wait().await;
                    if index % 2 == 0 {
                        LibsqlCheckpointer::open_at_for_workspace(
                            path,
                            format!("/workspace/{round}-{index}"),
                        )
                        .await
                        .map(|_| ())
                    } else {
                        LibsqlCheckpointer::list_all_threads_page_at(path, 0, 12)
                            .await
                            .map(|_| ())
                    }
                }));
            }
            for task in tasks {
                task.await.unwrap().unwrap();
            }
        }
    }

    #[tokio::test]
    async fn conversation_pages_are_bounded_and_report_the_next_page() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("threads.db");
        let cp =
            LibsqlCheckpointer::open_at_for_workspace(path.clone(), "/workspace/page-test".into())
                .await
                .unwrap();
        for index in 0..25 {
            cp.conn
                .execute(
                    "INSERT INTO threads (id, created, updated, title, workspace)
                     VALUES (?1, ?2, ?2, ?1, ?3)",
                    libsql::params![
                        format!("thread-{index:02}"),
                        index as i64,
                        "/workspace/page-test"
                    ],
                )
                .await
                .unwrap();
        }

        let (first, first_more) = cp.list_threads_page(0, 10, None).await.unwrap();
        let (second, second_more) = cp.list_threads_page(10, 10, None).await.unwrap();
        let (last, last_more) = cp.list_threads_page(20, 10, None).await.unwrap();
        assert_eq!(first.len(), 10);
        assert_eq!(first[0].id, "thread-24");
        assert!(first_more);
        assert_eq!(second.len(), 10);
        assert!(second_more);
        assert_eq!(last.len(), 5);
        assert!(!last_more);

        let (catalog, catalog_more) = LibsqlCheckpointer::list_all_threads_page_at(path, 10, 10)
            .await
            .unwrap();
        assert_eq!(catalog.len(), 10);
        assert!(catalog_more);
    }
}
