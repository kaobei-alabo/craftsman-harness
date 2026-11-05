//! Explicit local persistence planning. The ephemeral branch returns before
//! metadata/DB access; selection does not invoke catalog migration or callbacks.

use super::{PreparationError, Result, SessionSelection};
use crate::checkpoints::LibsqlCheckpointer;
use craftsman_harness::{AgentState, Msg};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const MAX_STATE: usize = 16 * 1024 * 1024;

pub(crate) enum Persistence {
    Ephemeral,
    Durable {
        mode: SessionSelection,
        thread_id: String,
        store: Arc<LibsqlCheckpointer>,
        journal_dir: PathBuf,
    },
}
impl Persistence {
    pub(crate) fn mode(&self) -> &SessionSelection {
        match self {
            Self::Ephemeral => &SessionSelection::Ephemeral,
            Self::Durable { mode, .. } => mode,
        }
    }
    pub(crate) fn thread_id(&self) -> Option<&str> {
        match self {
            Self::Ephemeral => None,
            Self::Durable { thread_id, .. } => Some(thread_id),
        }
    }
    pub(crate) fn store(&self) -> Option<&Arc<LibsqlCheckpointer>> {
        match self {
            Self::Ephemeral => None,
            Self::Durable { store, .. } => Some(store),
        }
    }
    pub(crate) fn journal_dir(&self) -> Option<&Path> {
        match self {
            Self::Ephemeral => None,
            Self::Durable { journal_dir, .. } => Some(journal_dir),
        }
    }
}

pub(super) async fn prepare(
    config: &Path,
    workspace: &Path,
    selection: SessionSelection,
) -> Result<(Persistence, AgentState)> {
    if selection == SessionSelection::Ephemeral {
        return Ok((Persistence::Ephemeral, AgentState::default()));
    }
    let path = config.join("threads.db");
    let selection_for_read = selection.clone();
    let path_for_check = path.clone();
    tokio::task::spawn_blocking(move || validate_store_path(&path_for_check))
        .await
        .map_err(|_| PreparationError::Worker)??;
    let restored = match &selection_for_read {
        SessionSelection::Fresh => {
            validate_fresh_schema(&path).await?;
            None
        }
        SessionSelection::Resume(id) => {
            if !valid_id(id) {
                return Err(PreparationError::MissingSession);
            }
            Some(select(&path, workspace, Some(id)).await?)
        }
        SessionSelection::Continue => Some(select(&path, workspace, None).await?),
        SessionSelection::Ephemeral => unreachable!(),
    };
    let store = Arc::new(
        LibsqlCheckpointer::open_at_for_headless(
            path,
            workspace
                .to_str()
                .ok_or(PreparationError::Scope)?
                .to_owned(),
        )
        .await
        .map_err(|_| PreparationError::Storage)?,
    );
    let (thread_id, state) = match restored {
        Some((id, state)) => (id, clean_restored_state(state)),
        None => {
            let mut chosen = None;
            for _ in 0..16 {
                let id = random_id()?;
                if store
                    .create_headless_thread(&id)
                    .await
                    .map_err(|_| PreparationError::Storage)?
                {
                    chosen = Some(id);
                    break;
                }
            }
            (
                chosen.ok_or(PreparationError::Random)?,
                AgentState::default(),
            )
        }
    };
    Ok((
        Persistence::Durable {
            mode: selection,
            thread_id,
            store,
            journal_dir: config.join("sessions-rs"),
        },
        state,
    ))
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}
fn random_id() -> Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| PreparationError::Random)?;
    let mut id = String::from("thread-");
    for byte in bytes {
        use std::fmt::Write;
        write!(id, "{byte:02x}").expect("string formatting");
    }
    Ok(id)
}

fn validate_store_path(path: &Path) -> Result<()> {
    use cap_fs_ext::MetadataExt;
    // Handle metadata is required for Windows link counts; no path-only by-handle
    // expectation, symlink following, or special-file blocking is allowed here.
    let parent = path.parent().ok_or(PreparationError::Storage)?;
    let directory = match cap_std::fs::Dir::open_ambient_dir(parent, cap_std::ambient_authority()) {
        Ok(dir) => dir,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(PreparationError::Storage),
    };
    let file = match directory.open_with(
        "threads.db",
        &crate::platform_os::confined_file_options(false, false),
    ) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(PreparationError::Storage),
    };
    let meta = file.metadata().map_err(|_| PreparationError::Storage)?;
    if !meta.is_file() || meta.nlink() != 1 {
        return Err(PreparationError::Storage);
    }
    Ok(())
}

async fn read_connection(path: &Path) -> Result<(libsql::Database, libsql::Connection)> {
    if !path.is_file() {
        return Err(PreparationError::MissingSession);
    }
    let db = libsql::Builder::new_local(path)
        .flags(libsql::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .build()
        .await
        .map_err(|_| PreparationError::Storage)?;
    let conn = db.connect().map_err(|_| PreparationError::Storage)?;
    Ok((db, conn))
}

async fn columns(conn: &libsql::Connection, table: &str) -> Result<Vec<String>> {
    let sql = match table {
        "checkpoints" => "PRAGMA table_info(checkpoints)",
        "threads" => "PRAGMA table_info(threads)",
        "thread_project_contexts" => "PRAGMA table_info(thread_project_contexts)",
        _ => unreachable!(),
    };
    let mut rows = conn
        .query(sql, ())
        .await
        .map_err(|_| PreparationError::Schema)?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await.map_err(|_| PreparationError::Schema)? {
        out.push(row.get(1).map_err(|_| PreparationError::Schema)?);
    }
    Ok(out)
}

async fn validate_fresh_schema(path: &Path) -> Result<()> {
    if !path.exists()
        || path
            .metadata()
            .map_err(|_| PreparationError::Storage)?
            .len()
            == 0
    {
        return Ok(());
    }
    let (_db, conn) = read_connection(path).await?;
    let checkpoint_columns = columns(&conn, "checkpoints").await?;
    let thread_columns = columns(&conn, "threads").await?;
    if checkpoint_columns.is_empty() && thread_columns.is_empty() {
        let mut rows = conn
            .query(
                "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
                (),
            )
            .await
            .map_err(|_| PreparationError::Schema)?;
        if rows
            .next()
            .await
            .map_err(|_| PreparationError::Schema)?
            .is_some()
        {
            return Err(PreparationError::Schema);
        }
        return Ok(());
    }
    if !["thread_id", "seq", "state_json", "suspension_json"]
        .iter()
        .all(|s| checkpoint_columns.iter().any(|c| c == s))
        || !["id", "created", "title"]
            .iter()
            .all(|s| thread_columns.iter().any(|c| c == s))
    {
        return Err(PreparationError::Schema);
    }
    Ok(())
}

async fn select(path: &Path, workspace: &Path, id: Option<&str>) -> Result<(String, AgentState)> {
    let (_db, conn) = read_connection(path).await?;
    let threads = columns(&conn, "threads").await?;
    let contexts = columns(&conn, "thread_project_contexts").await?;
    let checkpoints = columns(&conn, "checkpoints").await?;
    if !["id", "updated", "workspace"]
        .iter()
        .all(|s| threads.iter().any(|c| c == s))
        || !["thread_id", "path", "position"]
            .iter()
            .all(|s| contexts.iter().any(|c| c == s))
        || !["thread_id", "seq", "state_json"]
            .iter()
            .all(|s| checkpoints.iter().any(|c| c == s))
    {
        return Err(PreparationError::Schema);
    }
    let workspace = workspace.to_str().ok_or(PreparationError::Scope)?;
    let base="SELECT t.id,t.workspace,p.path,
        (SELECT CASE WHEN length(CAST(c.state_json AS BLOB)) <= 16777216 THEN c.state_json ELSE NULL END FROM checkpoints c WHERE c.thread_id=t.id ORDER BY c.seq DESC LIMIT 1),
        (SELECT length(CAST(c.state_json AS BLOB)) FROM checkpoints c WHERE c.thread_id=t.id ORDER BY c.seq DESC LIMIT 1)
        FROM threads t LEFT JOIN thread_project_contexts p ON p.thread_id=t.id AND p.position=0";
    let mut rows = if let Some(id) = id {
        conn.query(&format!("{base} WHERE t.id=?1"), libsql::params![id])
            .await
    } else {
        conn.query(
            &format!(
                "{base} WHERE t.workspace=?1 AND p.path=?1 ORDER BY t.updated DESC,t.id ASC LIMIT 1"
            ),
            libsql::params![workspace],
        )
        .await
    }
    .map_err(|_| PreparationError::Schema)?;
    let Some(row) = rows.next().await.map_err(|_| PreparationError::Schema)? else {
        return Err(PreparationError::MissingSession);
    };
    let id: String = row.get(0).map_err(|_| PreparationError::Checkpoint)?;
    let stored: String = row
        .get(1)
        .map_err(|_| PreparationError::WorkspaceMismatch)?;
    let primary: Option<String> = row
        .get(2)
        .map_err(|_| PreparationError::WorkspaceMismatch)?;
    if stored != workspace || primary.as_deref() != Some(workspace) {
        return Err(PreparationError::WorkspaceMismatch);
    }
    if !valid_id(&id) {
        return Err(PreparationError::Checkpoint);
    }
    let size: Option<i64> = row.get(4).map_err(|_| PreparationError::Checkpoint)?;
    if size.is_some_and(|n| n < 0 || n as u64 > MAX_STATE as u64) {
        return Err(PreparationError::Checkpoint);
    }
    let state: Option<String> = row.get(3).map_err(|_| PreparationError::Checkpoint)?;
    let state = match state {
        Some(state) => serde_json::from_str(&state).map_err(|_| PreparationError::Checkpoint)?,
        None => AgentState::default(),
    };
    Ok((id, state))
}

fn clean_restored_state(mut state: AgentState) -> AgentState {
    state.custom.clear();
    state.system_extras.clear();
    state.files.clear();
    state.last_usage = None;
    state.last_usage_msg_count = 0;
    // Restore conversation data only, never run a parked approval. Supply missing
    // terminal results in the latest tool group so a new turn cannot auto-resume it.
    if let Some(index) = state
        .messages
        .iter()
        .rposition(|m| matches!(m, Msg::Assistant { .. }))
    {
        let pending = match &state.messages[index] {
            Msg::Assistant { tool_calls, .. } => tool_calls.clone(),
            _ => unreachable!(),
        };
        let answered = state.messages[index + 1..]
            .iter()
            .filter_map(|m| match m {
                Msg::Tool { tool_call_id, .. } => Some(tool_call_id.clone()),
                _ => None,
            })
            .collect::<std::collections::BTreeSet<_>>();
        let missing=pending.into_iter().filter(|c|!answered.contains(&c.id)).map(|c|Msg::Tool {tool_call_id:c.id,content:"Tool call was not executed after restoring this thread; current invocation permissions still apply.".into(),is_error:true}).collect::<Vec<_>>();
        let position = index
            + 1
            + state.messages[index + 1..]
                .iter()
                .take_while(|m| matches!(m, Msg::Tool { .. }))
                .count();
        state.messages.splice(position..position, missing);
    }
    state
}

// Grammar-only preflight before config/input IO. Store/scope selection stays
// authoritative in prepare(), with its existing identifier grammar unchanged.
pub(super) fn valid_argument_id(id: &str) -> bool {
    valid_id(id)
}
