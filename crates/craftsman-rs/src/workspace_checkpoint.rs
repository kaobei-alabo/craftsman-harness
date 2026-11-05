//! Per-turn workspace snapshots and conflict-safe file rewind.
//!
//! This is intentionally app-layer state: it snapshots the real workspace and
//! stores changed file bytes beside conversation checkpoints in libSQL. Rewind
//! never changes Git refs, the user's index, or the active branch.

use async_trait::async_trait;
use craftsman_harness::{AgentError, AgentMiddleware, AgentState, Msg};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::io::Read;
#[cfg(unix)]
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::checkpoints::LibsqlCheckpointer;

const MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;
const MAX_SNAPSHOT_BYTES: u64 = 25 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceCheckpointSummary {
    pub id: String,
    pub turn_seq: u64,
    pub created: i64,
    pub summary: String,
    pub file_count: usize,
    pub partial: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RewindReport {
    pub checkpoint_id: String,
    pub restored: Vec<String>,
    pub conflicts: Vec<String>,
    pub unrestorable: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WorkspaceChangeKind {
    Created,
    Modified,
    Deleted,
}

impl WorkspaceChangeKind {
    pub(crate) const fn as_str(&self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Modified => "modified",
            Self::Deleted => "deleted",
        }
    }

    pub(crate) fn parse(value: &str) -> Result<Self, AgentError> {
        match value {
            "created" => Ok(Self::Created),
            "modified" => Ok(Self::Modified),
            "deleted" => Ok(Self::Deleted),
            other => Err(AgentError::State(format!(
                "unknown workspace checkpoint change kind: {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct WorkspaceCheckpointFile {
    pub path: String,
    pub kind: WorkspaceChangeKind,
    pub before_hash: Option<String>,
    pub after_hash: Option<String>,
    pub before_mode: Option<u32>,
    pub after_mode: Option<u32>,
    pub before_content: Option<Vec<u8>>,
    pub after_content: Option<Vec<u8>>,
    pub restorable: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct NewWorkspaceCheckpoint {
    pub id: String,
    pub thread_id: String,
    pub turn_seq: u64,
    pub created: i64,
    pub summary: String,
    pub partial: bool,
    pub files: Vec<WorkspaceCheckpointFile>,
}

#[derive(Debug, Clone)]
pub(crate) struct StoredWorkspaceCheckpoint {
    pub summary: WorkspaceCheckpointSummary,
    pub files: Vec<WorkspaceCheckpointFile>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SnapshotFile {
    hash: String,
    mode: Option<u32>,
    content: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Default)]
struct WorkspaceSnapshot {
    files: BTreeMap<String, SnapshotFile>,
}

#[derive(Debug, Clone)]
pub(crate) struct ActiveTurn {
    before: WorkspaceSnapshot,
    id: String,
    thread_id: String,
    turn_seq: u64,
    summary: String,
}

/// Shared per-session workspace checkpoint service. The middleware and slash
/// commands hold the same Arc, so thread switches are reflected immediately.
pub struct WorkspaceCheckpointManager {
    root: PathBuf,
    store: Arc<LibsqlCheckpointer>,
    thread_id: Mutex<String>,
    before: Mutex<Option<ActiveTurn>>,
}

impl WorkspaceCheckpointManager {
    pub fn new(
        root: PathBuf,
        store: Arc<LibsqlCheckpointer>,
        thread_id: impl Into<String>,
    ) -> Self {
        let root = root.canonicalize().unwrap_or(root);
        Self {
            root,
            store,
            thread_id: Mutex::new(thread_id.into()),
            before: Mutex::new(None),
        }
    }

    pub fn set_thread_id(&self, thread_id: impl Into<String>) {
        *self.thread_id.lock().unwrap() = thread_id.into();
        *self.before.lock().unwrap() = None;
    }

    pub async fn list(&self) -> Result<Vec<WorkspaceCheckpointSummary>, AgentError> {
        let thread_id = self.thread_id.lock().unwrap().clone();
        self.store.list_workspace_checkpoints(&thread_id).await
    }

    pub async fn rewind(
        &self,
        checkpoint_id: Option<&str>,
        force: bool,
    ) -> Result<RewindReport, AgentError> {
        let thread_id = self.thread_id.lock().unwrap().clone();
        let checkpoint = self
            .store
            .load_workspace_checkpoint(&thread_id, checkpoint_id)
            .await?
            .ok_or_else(|| {
                AgentError::State(match checkpoint_id {
                    Some(id) => format!("workspace checkpoint not found: {id}"),
                    None => "no workspace checkpoints for this session".to_string(),
                })
            })?;
        let root = self.root.clone();
        tokio::task::spawn_blocking(move || restore_checkpoint(&root, checkpoint, force))
            .await
            .map_err(|error| AgentError::State(format!("rewind task failed: {error}")))?
            .map_err(AgentError::State)
    }

    pub(crate) async fn begin_mutation(&self, summary: String) -> Option<ActiveTurn> {
        if let Some(active) = self.before.lock().unwrap().clone() {
            // Return a durable ticket even for a background child. If the
            // parent turn finishes before the child tool does, the child can
            // still update the same checkpoint from this cloned baseline.
            return Some(active);
        }
        let root = self.root.clone();
        let before = match tokio::task::spawn_blocking(move || capture_snapshot(&root)).await {
            Ok(Ok(snapshot)) => snapshot,
            Ok(Err(error)) => {
                Self::warn(error);
                return None;
            }
            Err(error) => {
                Self::warn(error);
                return None;
            }
        };
        let thread_id = self.thread_id.lock().unwrap().clone();
        let turn_seq = self
            .store
            .list_workspace_checkpoints(&thread_id)
            .await
            .ok()
            .and_then(|items| items.into_iter().map(|item| item.turn_seq).max())
            .unwrap_or(0)
            + 1;
        Some(ActiveTurn {
            before,
            id: new_checkpoint_id(),
            thread_id,
            turn_seq,
            summary,
        })
    }

    pub(crate) async fn finish_mutation(&self, ticket: Option<ActiveTurn>) {
        if let Some(active) = ticket {
            self.persist_active(&active).await;
        }
    }

    async fn persist_active(&self, active: &ActiveTurn) {
        let root = self.root.clone();
        let after = match tokio::task::spawn_blocking(move || capture_snapshot(&root)).await {
            Ok(Ok(snapshot)) => snapshot,
            Ok(Err(error)) => {
                Self::warn(error);
                return;
            }
            Err(error) => {
                Self::warn(error);
                return;
            }
        };
        let files = diff_snapshots(&active.before, &after);
        if files.is_empty() {
            if let Err(error) = self.store.delete_workspace_checkpoint(&active.id).await {
                Self::warn(error);
            }
            return;
        }
        let partial = files.iter().any(|file| !file.restorable);
        let checkpoint = NewWorkspaceCheckpoint {
            id: active.id.clone(),
            thread_id: active.thread_id.clone(),
            turn_seq: active.turn_seq,
            created: unix_secs(),
            summary: active.summary.clone(),
            partial,
            files,
        };
        if let Err(error) = self.store.save_workspace_checkpoint(&checkpoint).await {
            Self::warn(error);
        }
    }

    fn warn(error: impl std::fmt::Display) {
        eprintln!("craftsman: workspace checkpoint unavailable for this turn: {error}");
    }
}

#[async_trait]
impl AgentMiddleware for WorkspaceCheckpointManager {
    fn name(&self) -> &str {
        "workspace_checkpoint"
    }

    async fn turn_start(&self, state: &mut AgentState) -> Result<(), AgentError> {
        let root = self.root.clone();
        match tokio::task::spawn_blocking(move || capture_snapshot(&root)).await {
            Ok(Ok(snapshot)) => {
                let summary = state
                    .messages
                    .iter()
                    .rev()
                    .find_map(|message| match message {
                        Msg::User { content } => Some(crate::util::one_line(content, 100)),
                        _ => None,
                    })
                    .unwrap_or_else(|| "Agent workspace changes".to_string());
                let turn_seq = state
                    .messages
                    .iter()
                    .filter(|message| matches!(message, Msg::User { .. }))
                    .count() as u64;
                *self.before.lock().unwrap() = Some(ActiveTurn {
                    before: snapshot,
                    id: new_checkpoint_id(),
                    thread_id: self.thread_id.lock().unwrap().clone(),
                    turn_seq,
                    summary,
                });
            }
            Ok(Err(error)) => {
                *self.before.lock().unwrap() = None;
                Self::warn(error);
            }
            Err(error) => {
                *self.before.lock().unwrap() = None;
                Self::warn(error);
            }
        }
        Ok(())
    }

    async fn turn_end(&self, _state: &mut AgentState) -> Result<(), AgentError> {
        let active = self.before.lock().unwrap().take();
        if let Some(active) = active {
            self.persist_active(&active).await;
        }
        Ok(())
    }
}

fn unix_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

fn new_checkpoint_id() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let counter = NEXT.fetch_add(1, Ordering::Relaxed) & 0xffff;
    format!(
        "wc-{:x}-{:x}-{counter:04x}",
        unix_secs(),
        std::process::id()
    )
}

fn capture_snapshot(root: &Path) -> Result<WorkspaceSnapshot, String> {
    let paths = enumerate_paths(root)?;
    let mut captured = 0u64;
    let mut files = BTreeMap::new();
    for relative in paths {
        let Some(_) = safe_join(root, &relative) else {
            continue;
        };
        let Some(file) = capture_snapshot_file(root, &relative, captured)? else {
            continue;
        };
        if let Some(bytes) = &file.content {
            captured = captured.saturating_add(bytes.len() as u64);
        }
        files.insert(relative, file);
    }
    Ok(WorkspaceSnapshot { files })
}

fn enumerate_paths(root: &Path) -> Result<Vec<String>, String> {
    let git = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["ls-files", "-co", "--exclude-standard", "-z"])
        .output();
    if let Ok(output) = git
        && output.status.success()
    {
        let mut paths = output
            .stdout
            .split(|byte| *byte == 0)
            .filter(|raw| !raw.is_empty())
            .filter_map(|raw| std::str::from_utf8(raw).ok())
            .map(normalize_relative_path)
            .collect::<Vec<_>>();
        paths.sort();
        paths.dedup();
        return Ok(paths);
    }

    let mut paths = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let entries = std::fs::read_dir(&directory)
            .map_err(|error| format!("could not scan {}: {error}", directory.display()))?;
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            let metadata = match std::fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(_) => continue,
            };
            if metadata.file_type().is_symlink() {
                continue;
            }
            if metadata.is_dir() {
                if is_skipped_dir(path.file_name()) {
                    continue;
                }
                stack.push(path);
            } else if metadata.is_file()
                && let Ok(relative) = path.strip_prefix(root)
                && let Some(relative) = relative.to_str()
            {
                paths.push(normalize_relative_path(relative));
            }
        }
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

#[cfg(windows)]
fn normalize_relative_path(path: &str) -> String {
    path.replace('\\', "/")
}

#[cfg(not(windows))]
fn normalize_relative_path(path: &str) -> String {
    path.to_string()
}

fn is_skipped_dir(name: Option<&OsStr>) -> bool {
    name.and_then(OsStr::to_str)
        .is_some_and(|name| matches!(name, ".git" | "target" | "node_modules"))
}

#[cfg(unix)]
fn unix_name(value: &OsStr) -> std::io::Result<std::ffi::CString> {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::CString::new(value.as_bytes())
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "path contains NUL"))
}

#[cfg(unix)]
fn open_relative_parent(
    root: &Path,
    relative: &str,
    create: bool,
) -> std::io::Result<(std::os::fd::OwnedFd, std::ffi::CString)> {
    use std::os::fd::{AsRawFd, FromRawFd};
    let components = Path::new(relative)
        .components()
        .map(|component| match component {
            Component::Normal(name) => Ok(name.to_os_string()),
            _ => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "unsafe relative path",
            )),
        })
        .collect::<std::io::Result<Vec<_>>>()?;
    let (final_name, parents) = components
        .split_last()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "empty path"))?;
    let root_name = unix_name(root.as_os_str())?;
    let raw = unsafe {
        libc::open(
            root_name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if raw < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut directory = unsafe { std::os::fd::OwnedFd::from_raw_fd(raw) };
    for component in parents {
        let name = unix_name(component)?;
        let mut child = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )
        };
        if child < 0
            && create
            && std::io::Error::last_os_error().kind() == std::io::ErrorKind::NotFound
        {
            let made = unsafe { libc::mkdirat(directory.as_raw_fd(), name.as_ptr(), 0o755) };
            if made < 0
                && std::io::Error::last_os_error().kind() != std::io::ErrorKind::AlreadyExists
            {
                return Err(std::io::Error::last_os_error());
            }
            child = unsafe {
                libc::openat(
                    directory.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                )
            };
        }
        if child < 0 {
            return Err(std::io::Error::last_os_error());
        }
        directory = unsafe { std::os::fd::OwnedFd::from_raw_fd(child) };
    }
    Ok((directory, unix_name(final_name)?))
}

#[cfg(unix)]
fn open_relative_read(root: &Path, relative: &str) -> std::io::Result<std::fs::File> {
    use std::os::fd::{AsRawFd, FromRawFd};
    let (parent, name) = open_relative_parent(root, relative, false)?;
    let raw = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if raw < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(unsafe { std::fs::File::from_raw_fd(raw) })
}

#[cfg(unix)]
fn unlink_relative(root: &Path, relative: &str) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;
    let (parent, name) = open_relative_parent(root, relative, false)?;
    let result = unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), 0) };
    if result < 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::NotFound {
            return Err(error);
        }
    }
    Ok(())
}

#[cfg(unix)]
fn write_relative(
    root: &Path,
    relative: &str,
    content: &[u8],
    mode: Option<u32>,
) -> std::io::Result<()> {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::fs::PermissionsExt;
    let (parent, name) = open_relative_parent(root, relative, true)?;
    let removed = unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), 0) };
    if removed < 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::NotFound {
            return Err(error);
        }
    }
    let raw = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            mode.unwrap_or(0o644) as libc::c_uint,
        )
    };
    if raw < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut file = unsafe { std::fs::File::from_raw_fd(raw) };
    file.write_all(content)?;
    if let Some(mode) = mode {
        file.set_permissions(std::fs::Permissions::from_mode(mode))?;
    }
    file.sync_all()?;
    Ok(())
}

#[cfg(unix)]
fn capture_snapshot_file(
    root: &Path,
    relative: &str,
    captured: u64,
) -> Result<Option<SnapshotFile>, String> {
    let mut file = match open_relative_read(root, relative) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("could not safely open {relative}: {error}")),
    };
    let metadata = file
        .metadata()
        .map_err(|error| format!("could not inspect {relative}: {error}"))?;
    if !metadata.is_file() {
        return Ok(None);
    }
    let retain = metadata.len() <= MAX_FILE_BYTES
        && captured.saturating_add(metadata.len()) <= MAX_SNAPSHOT_BYTES;
    let (hash, content) = hash_reader(&mut file, retain)
        .map_err(|error| format!("could not read {relative}: {error}"))?;
    Ok(Some(SnapshotFile {
        hash,
        mode: file_mode(&metadata),
        content,
    }))
}

#[cfg(not(unix))]
fn capture_snapshot_file(
    root: &Path,
    relative: &str,
    captured: u64,
) -> Result<Option<SnapshotFile>, String> {
    let Some(path) = safe_join(root, relative) else {
        return Ok(None);
    };
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_file() => metadata,
        _ => return Ok(None),
    };
    let retain = metadata.len() <= MAX_FILE_BYTES
        && captured.saturating_add(metadata.len()) <= MAX_SNAPSHOT_BYTES;
    let mut file = std::fs::File::open(&path)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    let (hash, content) = hash_reader(&mut file, retain)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    Ok(Some(SnapshotFile {
        hash,
        mode: file_mode(&metadata),
        content,
    }))
}

fn hash_reader(
    reader: &mut std::fs::File,
    retain: bool,
) -> std::io::Result<(String, Option<Vec<u8>>)> {
    if retain {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes)?;
        return Ok((hex_hash(&bytes), Some(bytes)));
    }
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok((format!("{:x}", hasher.finalize()), None))
}

fn hex_hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(unix)]
fn file_mode(metadata: &std::fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    Some(metadata.permissions().mode() & 0o777)
}

#[cfg(not(unix))]
fn file_mode(_metadata: &std::fs::Metadata) -> Option<u32> {
    None
}

fn diff_snapshots(
    before: &WorkspaceSnapshot,
    after: &WorkspaceSnapshot,
) -> Vec<WorkspaceCheckpointFile> {
    let paths = before
        .files
        .keys()
        .chain(after.files.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut changes = Vec::new();
    for path in paths {
        let old = before.files.get(&path);
        let new = after.files.get(&path);
        if old.map(|file| (&file.hash, file.mode)) == new.map(|file| (&file.hash, file.mode)) {
            continue;
        }
        let kind = match (old, new) {
            (None, Some(_)) => WorkspaceChangeKind::Created,
            (Some(_), None) => WorkspaceChangeKind::Deleted,
            (Some(_), Some(_)) => WorkspaceChangeKind::Modified,
            (None, None) => continue,
        };
        let restorable = matches!(kind, WorkspaceChangeKind::Created)
            || old.is_some_and(|file| file.content.is_some());
        changes.push(WorkspaceCheckpointFile {
            path,
            kind,
            before_hash: old.map(|file| file.hash.clone()),
            after_hash: new.map(|file| file.hash.clone()),
            before_mode: old.and_then(|file| file.mode),
            after_mode: new.and_then(|file| file.mode),
            before_content: old.and_then(|file| file.content.clone()),
            // Rewind only needs the pre-turn bytes plus the post-turn hash/mode.
            // Avoid doubling checkpoint DB size with unused after bytes.
            after_content: None,
            restorable,
        });
    }
    changes
}

fn restore_checkpoint(
    root: &Path,
    checkpoint: StoredWorkspaceCheckpoint,
    force: bool,
) -> Result<RewindReport, String> {
    let mut report = RewindReport {
        checkpoint_id: checkpoint.summary.id,
        ..RewindReport::default()
    };
    for file in checkpoint.files {
        if !file.restorable {
            report.unrestorable.push(file.path);
            continue;
        }
        let Some(path) = safe_join(root, &file.path) else {
            report.unrestorable.push(file.path);
            continue;
        };
        if restore_parent_is_safe(root, &path).is_err() {
            report.unrestorable.push(file.path);
            continue;
        }
        let current = fingerprint(root, &file.path, &path)?;
        let expected = file
            .after_hash
            .as_ref()
            .map(|hash| (hash.as_str(), file.after_mode));
        let matches_after = match (current.as_ref(), expected) {
            (None, None) => true,
            (Some(current), Some((hash, mode))) => current.hash == hash && current.mode == mode,
            _ => false,
        };
        if !matches_after && !force {
            report.conflicts.push(file.path);
            continue;
        }
        if file.before_hash.is_none() {
            if let Err(error) = restore_delete(root, &file.path, &path) {
                report.unrestorable.push(format!("{} ({error})", file.path));
                continue;
            }
        } else {
            let Some(content) = file.before_content.as_ref() else {
                report.unrestorable.push(file.path);
                continue;
            };
            if let Err(error) = restore_write(root, &file.path, &path, content, file.before_mode) {
                report.unrestorable.push(format!("{} ({error})", file.path));
                continue;
            }
        }
        report.restored.push(file.path);
    }
    Ok(report)
}

#[derive(Debug)]
struct Fingerprint {
    hash: String,
    mode: Option<u32>,
}

#[cfg(unix)]
fn fingerprint(root: &Path, relative: &str, _path: &Path) -> Result<Option<Fingerprint>, String> {
    let mut file = match open_relative_read(root, relative) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Ok(Some(Fingerprint {
                hash: format!("<unsafe:{error}>"),
                mode: None,
            }));
        }
    };
    let metadata = file
        .metadata()
        .map_err(|error| format!("could not inspect {relative}: {error}"))?;
    if !metadata.is_file() {
        return Ok(Some(Fingerprint {
            hash: "<non-file>".into(),
            mode: None,
        }));
    }
    let (hash, _) = hash_reader(&mut file, false)
        .map_err(|error| format!("could not read {relative}: {error}"))?;
    Ok(Some(Fingerprint {
        hash,
        mode: file_mode(&metadata),
    }))
}

#[cfg(not(unix))]
fn fingerprint(_root: &Path, _relative: &str, path: &Path) -> Result<Option<Fingerprint>, String> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => metadata,
        Ok(_) => {
            return Ok(Some(Fingerprint {
                hash: "<non-file>".into(),
                mode: None,
            }));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("could not inspect {}: {error}", path.display())),
    };
    let mut file = std::fs::File::open(path)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    let (hash, _) = hash_reader(&mut file, false)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    Ok(Some(Fingerprint {
        hash,
        mode: file_mode(&metadata),
    }))
}

#[cfg(unix)]
fn restore_parent_is_safe(_root: &Path, _path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(not(unix))]
fn restore_parent_is_safe(root: &Path, path: &Path) -> Result<(), String> {
    ensure_safe_parent(root, path)
}

#[cfg(unix)]
fn restore_delete(root: &Path, relative: &str, _path: &Path) -> Result<(), String> {
    unlink_relative(root, relative).map_err(|error| error.to_string())
}

#[cfg(not(unix))]
fn restore_delete(_root: &Path, _relative: &str, path: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => Err("target is a directory".to_string()),
        Ok(_) => std::fs::remove_file(path).map_err(|error| error.to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(unix)]
fn restore_write(
    root: &Path,
    relative: &str,
    _path: &Path,
    content: &[u8],
    mode: Option<u32>,
) -> Result<(), String> {
    write_relative(root, relative, content, mode).map_err(|error| error.to_string())
}

#[cfg(not(unix))]
fn restore_write(
    _root: &Path,
    _relative: &str,
    path: &Path,
    content: &[u8],
    mode: Option<u32>,
) -> Result<(), String> {
    if let Ok(metadata) = std::fs::symlink_metadata(path)
        && metadata.file_type().is_symlink()
    {
        std::fs::remove_file(path).map_err(|error| error.to_string())?;
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    std::fs::write(path, content).map_err(|error| error.to_string())?;
    set_file_mode(path, mode)
}

fn safe_join(root: &Path, relative: &str) -> Option<PathBuf> {
    let relative = Path::new(relative);
    if relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return None;
    }
    Some(root.join(relative))
}

#[cfg(not(unix))]
fn ensure_safe_parent(root: &Path, path: &Path) -> Result<(), String> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| format!("restore path escaped workspace: {}", path.display()))?;
    let mut cursor = root.to_path_buf();
    if let Some(parent) = relative.parent() {
        for component in parent.components() {
            cursor.push(component.as_os_str());
            if let Ok(metadata) = std::fs::symlink_metadata(&cursor)
                && metadata.file_type().is_symlink()
            {
                return Err(format!(
                    "refusing to restore through symlinked directory {}",
                    cursor.display()
                ));
            }
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn set_file_mode(_path: &Path, _mode: Option<u32>) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn manager(dir: &tempfile::TempDir) -> WorkspaceCheckpointManager {
        let cp = Arc::new(
            LibsqlCheckpointer::open_at(dir.path().join("threads.db"))
                .await
                .unwrap(),
        );
        let root = dir.path().join("workspace");
        std::fs::create_dir_all(&root).unwrap();
        WorkspaceCheckpointManager::new(root, cp, "thread-1")
    }

    fn state(prompt: &str) -> AgentState {
        let mut state = AgentState::default();
        state.messages.push(Msg::user(prompt));
        state
    }

    #[tokio::test]
    async fn no_change_creates_no_checkpoint() {
        let dir = tempfile::tempdir().unwrap();
        let manager = manager(&dir).await;
        let mut state = state("no-op");
        manager.turn_start(&mut state).await.unwrap();
        manager.turn_end(&mut state).await.unwrap();
        assert!(manager.list().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn rewind_restores_created_modified_deleted_and_binary_files() {
        let dir = tempfile::tempdir().unwrap();
        let manager = manager(&dir).await;
        let root = manager.root.clone();
        std::fs::write(root.join("modified.bin"), [0, 1, 2]).unwrap();
        std::fs::write(root.join("deleted.txt"), b"before").unwrap();
        let mut state = state("change files");
        manager.turn_start(&mut state).await.unwrap();
        std::fs::write(root.join("modified.bin"), [9, 8, 7]).unwrap();
        std::fs::remove_file(root.join("deleted.txt")).unwrap();
        std::fs::write(root.join("created.txt"), b"new").unwrap();
        manager.turn_end(&mut state).await.unwrap();

        let list = manager.list().await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].file_count, 3);
        let report = manager.rewind(None, false).await.unwrap();
        assert_eq!(report.restored.len(), 3);
        assert_eq!(std::fs::read(root.join("modified.bin")).unwrap(), [0, 1, 2]);
        assert_eq!(std::fs::read(root.join("deleted.txt")).unwrap(), b"before");
        assert!(!root.join("created.txt").exists());
    }

    #[tokio::test]
    async fn rewind_skips_conflicts_unless_force_is_explicit() {
        let dir = tempfile::tempdir().unwrap();
        let manager = manager(&dir).await;
        let path = manager.root.join("file.txt");
        std::fs::write(&path, b"before").unwrap();
        let mut state = state("edit");
        manager.turn_start(&mut state).await.unwrap();
        std::fs::write(&path, b"agent").unwrap();
        manager.turn_end(&mut state).await.unwrap();
        std::fs::write(&path, b"user").unwrap();

        let report = manager.rewind(None, false).await.unwrap();
        assert_eq!(report.conflicts, vec!["file.txt"]);
        assert_eq!(std::fs::read(&path).unwrap(), b"user");

        let report = manager.rewind(None, true).await.unwrap();
        assert_eq!(report.restored, vec!["file.txt"]);
        assert_eq!(std::fs::read(&path).unwrap(), b"before");
    }

    #[test]
    fn oversized_modified_file_is_marked_unrestorable() {
        let before = WorkspaceSnapshot {
            files: BTreeMap::from([(
                "large.bin".to_string(),
                SnapshotFile {
                    hash: "before".to_string(),
                    mode: None,
                    content: None,
                },
            )]),
        };
        let after = WorkspaceSnapshot {
            files: BTreeMap::from([(
                "large.bin".to_string(),
                SnapshotFile {
                    hash: "after".to_string(),
                    mode: None,
                    content: None,
                },
            )]),
        };
        let changes = diff_snapshots(&before, &after);
        assert_eq!(changes.len(), 1);
        assert!(!changes[0].restorable);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn rewind_restores_unix_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let manager = manager(&dir).await;
        let path = manager.root.join("script.sh");
        std::fs::write(&path, b"before").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let mut state = state("make executable");
        manager.turn_start(&mut state).await.unwrap();
        std::fs::write(&path, b"after").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        manager.turn_end(&mut state).await.unwrap();
        manager.rewind(None, false).await.unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"before");
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o644
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn rewind_never_traverses_a_symlinked_parent() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let manager = manager(&dir).await;
        let mut state = state("create nested file");
        manager.turn_start(&mut state).await.unwrap();
        std::fs::create_dir_all(manager.root.join("nested")).unwrap();
        std::fs::write(manager.root.join("nested/file.txt"), b"agent").unwrap();
        manager.turn_end(&mut state).await.unwrap();

        std::fs::remove_dir_all(manager.root.join("nested")).unwrap();
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("file.txt"), b"agent").unwrap();
        symlink(&outside, manager.root.join("nested")).unwrap();

        let report = manager.rewind(None, true).await.unwrap();
        assert_eq!(report.unrestorable.len(), 1);
        assert!(report.unrestorable[0].starts_with("nested/file.txt"));
        assert_eq!(std::fs::read(outside.join("file.txt")).unwrap(), b"agent");
    }

    #[tokio::test]
    async fn workspace_checkpoints_prune_and_delete_with_thread() {
        use craftsman_harness::Checkpointer;
        let dir = tempfile::tempdir().unwrap();
        let manager = manager(&dir).await;
        manager
            .store
            .save("thread-1", 0, &state("checkpoint pruning"), None)
            .await
            .unwrap();
        for index in 0..21u64 {
            manager
                .store
                .save_workspace_checkpoint(&NewWorkspaceCheckpoint {
                    id: format!("wc-{index:03}"),
                    thread_id: "thread-1".to_string(),
                    turn_seq: index,
                    created: index as i64,
                    summary: format!("turn {index}"),
                    partial: false,
                    files: vec![WorkspaceCheckpointFile {
                        path: format!("{index}.txt"),
                        kind: WorkspaceChangeKind::Created,
                        before_hash: None,
                        after_hash: Some("hash".to_string()),
                        before_mode: None,
                        after_mode: None,
                        before_content: None,
                        after_content: Some(Vec::new()),
                        restorable: true,
                    }],
                })
                .await
                .unwrap();
        }
        let list = manager.list().await.unwrap();
        assert_eq!(list.len(), 20);
        assert!(!list.iter().any(|checkpoint| checkpoint.id == "wc-000"));
        manager.store.delete_thread("thread-1").await.unwrap();
        assert!(manager.list().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn background_mutation_ticket_survives_parent_turn_completion() {
        let dir = tempfile::tempdir().unwrap();
        let manager = manager(&dir).await;
        let mut parent = state("parent delegates in background");
        manager.turn_start(&mut parent).await.unwrap();
        let ticket = manager.begin_mutation("background edit".to_string()).await;
        manager.turn_end(&mut parent).await.unwrap();
        assert!(manager.list().await.unwrap().is_empty());

        std::fs::write(manager.root.join("late.txt"), b"late child").unwrap();
        manager.finish_mutation(ticket).await;
        let checkpoints = manager.list().await.unwrap();
        assert_eq!(checkpoints.len(), 1);
        assert_eq!(checkpoints[0].summary, "parent delegates in background");
        assert_eq!(checkpoints[0].file_count, 1);
    }

    #[tokio::test]
    async fn standalone_mutation_creates_a_checkpoint_for_delegated_work() {
        let dir = tempfile::tempdir().unwrap();
        let manager = manager(&dir).await;
        let baseline = manager.begin_mutation("delegated edit".to_string()).await;
        std::fs::write(manager.root.join("delegated.txt"), b"child").unwrap();
        manager.finish_mutation(baseline).await;
        let checkpoints = manager.list().await.unwrap();
        assert_eq!(checkpoints.len(), 1);
        assert_eq!(checkpoints[0].summary, "delegated edit");
        assert_eq!(checkpoints[0].file_count, 1);
    }

    #[tokio::test]
    async fn thread_switch_changes_listing_scope() {
        let dir = tempfile::tempdir().unwrap();
        let manager = manager(&dir).await;
        let mut state = state("first");
        manager.turn_start(&mut state).await.unwrap();
        std::fs::write(manager.root.join("a.txt"), b"a").unwrap();
        manager.turn_end(&mut state).await.unwrap();
        assert_eq!(manager.list().await.unwrap().len(), 1);
        manager.set_thread_id("thread-2");
        assert!(manager.list().await.unwrap().is_empty());
    }
}
