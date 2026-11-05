//! `StoreFs` + `CompositeBackend` — durable, cross-session file storage for the
//! agent's long-term memory.
//!
//! [`StoreFs`] is a [`Backend`] over a libSQL table (`store_files`) in the SAME
//! `~/.craftsman/threads.db` database the checkpointer uses, so memory survives
//! across sessions and threads. It opens its own connection to that local file;
//! there is no remote sync. Filesystem semantics (`cat -n`/overwrite-refusal/
//! unique-edit/glob/grep) are byte-identical to the framework's
//! [`craftsman_harness::backend::StateFs`]; execution is unsupported.
//!
//! [`CompositeBackend`] routes file operations by path prefix: paths under
//! `/memories/` go to a routed backend (the store), everything else to a default
//! backend (the real filesystem). Paths are passed through UNSTRIPPED — the store
//! keys include the `/memories/` prefix. Execution is never path-routable and
//! always delegates to the default backend.
//!
use async_trait::async_trait;
use craftsman_harness::backend::{cat_n, glob_match};
use craftsman_harness::{AgentError, Backend, ExecOutcome, FileInfo};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Path prefix routed to the store by [`CompositeBackend`] in the CLI.
pub const MEMORY_PREFIX: &str = "/memories/";

fn unix_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn state_err(e: impl std::fmt::Display) -> AgentError {
    AgentError::State(e.to_string())
}

/// libSQL-backed [`Backend`] for durable memory files, stored in the local
/// `~/.craftsman/threads.db`.
pub struct StoreFs {
    /// Held for the connection's lifetime; never read.
    _db: libsql::Database,
    conn: libsql::Connection,
}

impl StoreFs {
    /// Open (creating if needed) the store at `~/.craftsman/threads.db`, applying
    /// the schema.
    pub async fn open() -> Result<Self, String> {
        let path = crate::threads_db::path()
            .await
            .map_err(|error| error.to_string())?;
        Self::open_at(path).await
    }

    /// Path-explicit open — the testable core behind [`open`].
    pub(crate) async fn open_at(path: std::path::PathBuf) -> Result<Self, String> {
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
        conn.execute(
            "CREATE TABLE IF NOT EXISTS store_files (
                path TEXT PRIMARY KEY,
                content TEXT NOT NULL,
                modified INTEGER NOT NULL
            )",
            (),
        )
        .await
        .map_err(|e| e.to_string())?;
        drop(init_guard);

        Ok(Self { _db: db, conn })
    }

    /// Fetch a file's content, or `None` if absent.
    async fn get(&self, path: &str) -> Result<Option<String>, AgentError> {
        let mut rows = self
            .conn
            .query(
                "SELECT content FROM store_files WHERE path = ?1",
                libsql::params![path],
            )
            .await
            .map_err(state_err)?;
        match rows.next().await.map_err(state_err)? {
            Some(row) => Ok(Some(row.get::<String>(0).map_err(state_err)?)),
            None => Ok(None),
        }
    }

    /// All `(path, content)` rows, ordered by path (stable output for glob/grep/ls).
    async fn all_rows(&self) -> Result<Vec<(String, String)>, AgentError> {
        let mut rows = self
            .conn
            .query("SELECT path, content FROM store_files ORDER BY path", ())
            .await
            .map_err(state_err)?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().await.map_err(state_err)? {
            let p: String = row.get(0).map_err(state_err)?;
            let c: String = row.get(1).map_err(state_err)?;
            out.push((p, c));
        }
        Ok(out)
    }
}

#[async_trait]
impl Backend for StoreFs {
    async fn ls(&self, path: &str) -> Result<Vec<FileInfo>, AgentError> {
        let prefix = if path == "/" { "" } else { path };
        Ok(self
            .all_rows()
            .await?
            .into_iter()
            .filter(|(p, _)| p.starts_with(prefix))
            .map(|(p, c)| FileInfo {
                path: p,
                size: c.len() as u64,
            })
            .collect())
    }

    async fn read(&self, path: &str, offset: usize, limit: usize) -> Result<String, AgentError> {
        let content = self
            .get(path)
            .await?
            .ok_or_else(|| AgentError::State(format!("File '{path}' not found")))?;
        Ok(cat_n(&content, offset, limit))
    }

    async fn write(&self, path: &str, content: &str) -> Result<(), AgentError> {
        if self.get(path).await?.is_some() {
            return Err(AgentError::State(format!(
                "Cannot write to {path} because it already exists. Read and then make an edit, or write to a new path."
            )));
        }
        self.conn
            .execute(
                "INSERT INTO store_files (path, content, modified) VALUES (?1, ?2, ?3)",
                libsql::params![path, content, unix_secs()],
            )
            .await
            .map_err(state_err)?;
        Ok(())
    }

    async fn edit(
        &self,
        path: &str,
        old: &str,
        new: &str,
        replace_all: bool,
    ) -> Result<(), AgentError> {
        let content = self
            .get(path)
            .await?
            .ok_or_else(|| AgentError::State(format!("File '{path}' not found")))?;
        let count = content.matches(old).count();
        // The not-found check applies to replace_all too: silently writing back
        // unchanged content would tell the model the edit landed when it didn't.
        if count == 0 {
            return Err(AgentError::State(format!(
                "Error: String not found in file: '{old}'"
            )));
        }
        let updated = if replace_all {
            content.replace(old, new)
        } else {
            match count {
                1 => content.replacen(old, new, 1),
                occurrences => {
                    return Err(AgentError::State(format!(
                        "Error: String '{old}' appears {occurrences} times in file. Use replace_all=True to replace all instances, or provide a more specific string with surrounding context."
                    )));
                }
            }
        };
        self.conn
            .execute(
                "UPDATE store_files SET content = ?2, modified = ?3 WHERE path = ?1",
                libsql::params![path, updated, unix_secs()],
            )
            .await
            .map_err(state_err)?;
        Ok(())
    }

    async fn glob(&self, pattern: &str) -> Result<Vec<String>, AgentError> {
        Ok(self
            .all_rows()
            .await?
            .into_iter()
            .map(|(p, _)| p)
            .filter(|p| glob_match(pattern, p))
            .collect())
    }

    async fn grep(&self, pattern: &str, path: Option<&str>) -> Result<String, AgentError> {
        let mut out = String::new();
        for (fpath, content) in self.all_rows().await? {
            if let Some(p) = path {
                if !fpath.starts_with(p) {
                    continue;
                }
            }
            for (i, line) in content.lines().enumerate() {
                if line.contains(pattern) {
                    out.push_str(&format!("{}:{}:{}\n", fpath, i + 1, line));
                }
            }
        }
        if out.is_empty() {
            Ok(format!("No matches found for '{pattern}'"))
        } else {
            Ok(out)
        }
    }
}

/// Routes file operations to a `routed` backend for paths under `prefix`, and to
/// `default` for everything else. Paths are passed through unstripped.
pub struct CompositeBackend {
    default: Arc<dyn Backend>,
    routed: Arc<dyn Backend>,
    /// Route prefix, e.g. `/memories/`.
    prefix: String,
}

impl CompositeBackend {
    pub fn new(
        default: Arc<dyn Backend>,
        prefix: impl Into<String>,
        routed: Arc<dyn Backend>,
    ) -> Self {
        Self {
            default,
            routed,
            prefix: prefix.into(),
        }
    }

    /// True when `path` targets the routed prefix (the bare prefix without a
    /// trailing slash, or anything beneath it).
    fn is_routed(&self, path: &str) -> bool {
        let bare = self.prefix.trim_end_matches('/');
        path == bare || path.starts_with(&self.prefix)
    }

    fn backend_for(&self, path: &str) -> &Arc<dyn Backend> {
        if self.is_routed(path) {
            &self.routed
        } else {
            &self.default
        }
    }
}

#[async_trait]
impl Backend for CompositeBackend {
    async fn ls(&self, path: &str) -> Result<Vec<FileInfo>, AgentError> {
        self.backend_for(path).ls(path).await
    }

    async fn read(&self, path: &str, offset: usize, limit: usize) -> Result<String, AgentError> {
        self.backend_for(path).read(path, offset, limit).await
    }

    async fn write(&self, path: &str, content: &str) -> Result<(), AgentError> {
        self.backend_for(path).write(path, content).await
    }

    async fn edit(
        &self,
        path: &str,
        old: &str,
        new: &str,
        replace_all: bool,
    ) -> Result<(), AgentError> {
        self.backend_for(path)
            .edit(path, old, new, replace_all)
            .await
    }

    async fn glob(&self, pattern: &str) -> Result<Vec<String>, AgentError> {
        // Route by the pattern's prefix: a `/memories/...` pattern hits the store
        // only; anything else hits the default backend only.
        self.backend_for(pattern).glob(pattern).await
    }

    async fn grep(&self, pattern: &str, path: Option<&str>) -> Result<String, AgentError> {
        // A `/memories/`-prefixed path scopes to the store; None or any other
        // path scopes to the default backend.
        match path {
            Some(p) if self.is_routed(p) => self.routed.grep(pattern, path).await,
            _ => self.default.grep(pattern, path).await,
        }
    }

    fn supports_execution(&self) -> bool {
        // Execution is never path-routable; it always uses the default backend.
        self.default.supports_execution()
    }

    async fn execute(&self, command: &str) -> Result<ExecOutcome, AgentError> {
        self.default.execute(command).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend_fs::RealFs;

    async fn store() -> (tempfile::TempDir, StoreFs) {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("threads.db");
        let fs = StoreFs::open_at(path).await.unwrap();
        (tmp, fs)
    }

    #[tokio::test]
    async fn storefs_read_write_edit_round_trip() {
        let (_tmp, fs) = store().await;
        fs.write("/memories/a.md", "one\ntwo").await.unwrap();
        assert_eq!(
            fs.read("/memories/a.md", 0, 100).await.unwrap(),
            "     1\tone\n     2\ttwo"
        );
        // Overwrite is refused.
        assert!(
            fs.write("/memories/a.md", "x")
                .await
                .unwrap_err()
                .to_string()
                .contains("already exists")
        );
        // Missing file read errors.
        assert!(
            fs.read("/memories/missing.md", 0, 100)
                .await
                .unwrap_err()
                .to_string()
                .contains("not found")
        );
        fs.edit("/memories/a.md", "two", "2", false).await.unwrap();
        assert!(
            fs.read("/memories/a.md", 0, 100)
                .await
                .unwrap()
                .contains("2")
        );
        assert!(!fs.supports_execution());
    }

    #[tokio::test]
    async fn storefs_edit_requires_unique_match() {
        let (_tmp, fs) = store().await;
        fs.write("/memories/a.md", "aa aa").await.unwrap();
        let err = fs
            .edit("/memories/a.md", "aa", "b", false)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("appears 2 times"));
        fs.edit("/memories/a.md", "aa", "b", true).await.unwrap();
        assert_eq!(fs.get("/memories/a.md").await.unwrap().unwrap(), "b b");
        // replace_all with a missing old_string must error, not silently no-op.
        let err = fs
            .edit("/memories/a.md", "missing", "x", true)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[tokio::test]
    async fn storefs_ls_glob_grep() {
        let (_tmp, fs) = store().await;
        fs.write("/memories/a.md", "hello\nworld").await.unwrap();
        fs.write("/memories/b.txt", "hello").await.unwrap();
        let all = fs.ls("/").await.unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(
            fs.glob("/memories/*.md").await.unwrap(),
            vec!["/memories/a.md".to_string()]
        );
        let hits = fs.grep("world", None).await.unwrap();
        assert!(hits.contains("/memories/a.md") && hits.contains("world"));
        assert!(
            fs.grep("nope", None)
                .await
                .unwrap()
                .contains("No matches found")
        );
    }

    #[tokio::test]
    async fn storefs_persists_across_reopen() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("threads.db");
        {
            let fs = StoreFs::open_at(path.clone()).await.unwrap();
            fs.write("/memories/note.md", "durable").await.unwrap();
        }
        let fs2 = StoreFs::open_at(path).await.unwrap();
        assert_eq!(
            fs2.get("/memories/note.md").await.unwrap().unwrap(),
            "durable"
        );
    }

    async fn composite() -> (tempfile::TempDir, tempfile::TempDir, CompositeBackend) {
        let disk = tempfile::tempdir().unwrap();
        let default = Arc::new(RealFs::new(disk.path().to_path_buf()));
        let (store_tmp, store) = store().await;
        let composite = CompositeBackend::new(default, MEMORY_PREFIX, Arc::new(store));
        (disk, store_tmp, composite)
    }

    #[tokio::test]
    async fn composite_routes_memories_to_store() {
        let (disk, _store_tmp, c) = composite().await;
        c.write("/memories/pref.md", "teal").await.unwrap();
        c.write("plain.txt", "on disk").await.unwrap();

        // Memory file is in the store, NOT on the real filesystem.
        assert!(
            c.read("/memories/pref.md", 0, 100)
                .await
                .unwrap()
                .contains("teal")
        );
        assert!(!disk.path().join("memories").exists());
        // Plain file landed on the real filesystem.
        assert!(disk.path().join("plain.txt").exists());
        assert!(
            c.read("plain.txt", 0, 100)
                .await
                .unwrap()
                .contains("on disk")
        );
    }

    #[tokio::test]
    async fn composite_ls_glob_grep_route_by_prefix() {
        let (_disk, _store_tmp, c) = composite().await;
        c.write("/memories/m.md", "secret needle").await.unwrap();
        c.write("code.rs", "plain needle").await.unwrap();

        // ls / glob scoped to the memory prefix hit only the store.
        let mem_ls = c.ls("/memories/").await.unwrap();
        assert_eq!(mem_ls.len(), 1);
        assert_eq!(mem_ls[0].path, "/memories/m.md");
        assert_eq!(
            c.glob("/memories/*.md").await.unwrap(),
            vec!["/memories/m.md".to_string()]
        );

        // grep scoped to the memory prefix hits only the store.
        let mem_grep = c.grep("needle", Some("/memories/")).await.unwrap();
        assert!(mem_grep.contains("/memories/m.md"));
        assert!(!mem_grep.contains("code.rs"));

        // Unprefixed glob/grep hit only the real filesystem (store excluded).
        assert_eq!(c.glob("*.rs").await.unwrap(), vec!["code.rs".to_string()]);
        let disk_grep = c.grep("needle", None).await.unwrap();
        assert!(disk_grep.contains("code.rs"));
        assert!(!disk_grep.contains("/memories/m.md"));
    }

    #[tokio::test]
    async fn composite_execution_uses_default() {
        let (_disk, _store_tmp, c) = composite().await;
        assert!(c.supports_execution());
        let out = c.execute("echo hi").await.unwrap();
        assert_eq!(out.stdout.trim(), "hi");
    }
}
