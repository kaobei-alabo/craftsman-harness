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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::checkpoints::LibsqlCheckpointer;

const MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;
const MAX_SNAPSHOT_BYTES: u64 = 25 * 1024 * 1024;
const MAX_SCAN_ENTRIES: usize = 4096;
const MAX_SCAN_FILES: usize = 1024;
const MAX_SCOPE_DIRS: usize = 64;
const MAX_GIT_LIST_BYTES: u64 = 1024 * 1024;

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
    observed: BTreeSet<String>,
    directories: BTreeMap<String, Arc<BTreeSet<String>>>,
    unavailable: BTreeSet<String>,
    non_files: BTreeSet<String>,
    partial: bool,
}

impl WorkspaceSnapshot {
    fn covers(&self, path: &str) -> bool {
        self.files.contains_key(path)
            || self.observed.contains(path)
            || (!self.non_files.contains(path)
                && self.directories.iter().any(|(dir, unavailable)| {
                    within_directory(path, dir)
                        && !unavailable.iter().any(|dir| within_directory(path, dir))
                }))
    }

    fn retain_first_observations(&mut self, fresh: Self) {
        for path in &fresh.observed {
            if !self.covers(path) {
                if let Some(file) = fresh.files.get(path) {
                    self.files.insert(path.clone(), file.clone());
                }
                self.observed.insert(path.clone());
            }
        }
        for (directory, unavailable) in fresh.directories {
            self.directories.entry(directory).or_insert(unavailable);
        }
        self.unavailable.extend(fresh.unavailable);
        self.non_files.extend(fresh.non_files);
        self.partial |= fresh.partial;
    }

    fn retained_bytes(&self) -> u64 {
        self.files
            .values()
            .filter_map(|file| file.content.as_ref())
            .map(|bytes| bytes.len() as u64)
            .sum()
    }
}

#[derive(Debug, Clone, Default)]
struct SnapshotSelection {
    directories: BTreeSet<String>,
    files: BTreeSet<String>,
    partial: bool,
    excluded: BTreeSet<String>,
    protected: BTreeSet<String>,
}

#[derive(Debug, Default)]
struct TurnSnapshots {
    before: WorkspaceSnapshot,
    selection: SnapshotSelection,
    directory_hints: BTreeSet<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct ActiveTurn {
    snapshots: Arc<tokio::sync::Mutex<TurnSnapshots>>,
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
    warning_sink: Option<crate::agent::ActivitySink>,
    partial_notice_sent: AtomicBool,
    automatic_exclusions: BTreeSet<String>,
    protected: BTreeSet<String>,
}

impl WorkspaceCheckpointManager {
    pub fn new(
        root: PathBuf,
        store: Arc<LibsqlCheckpointer>,
        thread_id: impl Into<String>,
    ) -> Self {
        let root = root.canonicalize().unwrap_or(root);
        let database_path = store
            .database_path()
            .canonicalize()
            .unwrap_or_else(|_| store.database_path().to_path_buf());
        let mut protected = BTreeSet::new();
        for suffix in ["", "-wal", "-shm", "-journal"] {
            let mut path = database_path.as_os_str().to_os_string();
            path.push(suffix);
            if let Some(relative) = Path::new(&path)
                .to_str()
                .and_then(|path| checkpoint_relative(&root, path))
            {
                protected.insert(relative);
            }
        }
        let mut automatic_exclusions = protected.clone();
        let config = crate::config::config_dir();
        let config = config.canonicalize().unwrap_or(config);
        if let Some(relative) = config
            .to_str()
            .and_then(|path| checkpoint_relative(&root, path))
        {
            if relative.is_empty() {
                automatic_exclusions.extend(
                    ["config.json", "sessions-rs", "gadgets"]
                        .into_iter()
                        .map(str::to_owned),
                );
            } else {
                automatic_exclusions.insert(relative);
            }
        }
        Self {
            root,
            store,
            thread_id: Mutex::new(thread_id.into()),
            before: Mutex::new(None),
            warning_sink: None,
            partial_notice_sent: AtomicBool::new(false),
            automatic_exclusions,
            protected,
        }
    }

    pub(crate) fn with_warning_sink(mut self, sink: Option<crate::agent::ActivitySink>) -> Self {
        self.warning_sink = sink;
        self
    }

    pub(crate) async fn note_file(&self, path: &str) {
        let Some(relative) = checkpoint_relative(&self.root, path) else {
            return;
        };
        let parent = Path::new(&relative)
            .parent()
            .unwrap_or_else(|| Path::new(""));
        self.note_directory(parent.to_str().unwrap_or("")).await;
    }

    pub(crate) async fn note_directory(&self, path: &str) {
        let Some(relative) = checkpoint_relative(&self.root, path) else {
            return;
        };
        let active = self.before.lock().unwrap().clone();
        if let Some(active) = active {
            let mut snapshots = active.snapshots.lock().await;
            if snapshots.directory_hints.len() < MAX_SCOPE_DIRS {
                snapshots.directory_hints.insert(relative);
            } else {
                snapshots.selection.partial = true;
            }
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
        let mut checkpoint = self
            .store
            .load_workspace_checkpoint(&thread_id, checkpoint_id)
            .await?
            .ok_or_else(|| {
                AgentError::State(match checkpoint_id {
                    Some(id) => format!("workspace checkpoint not found: {id}"),
                    None => "no workspace checkpoints for this session".to_string(),
                })
            })?;
        for file in &mut checkpoint.files {
            if self.protected.contains(&file.path) {
                file.restorable = false;
            }
        }
        let root = self.root.clone();
        tokio::task::spawn_blocking(move || restore_checkpoint(&root, checkpoint, force))
            .await
            .map_err(|error| AgentError::State(format!("rewind task failed: {error}")))?
            .map_err(AgentError::State)
    }

    pub(crate) async fn begin_mutation(&self, summary: String) -> Option<ActiveTurn> {
        let active = self.mutation_ticket(summary).await;
        let mut snapshots = active.snapshots.lock().await;
        {
            let mut directories = if snapshots.directory_hints.is_empty() {
                BTreeSet::from([String::new()])
            } else {
                snapshots.directory_hints.clone()
            };
            if directories.len() > 1 {
                directories.remove("");
            }
            directories.retain(|directory| !snapshots.selection.directories.contains(directory));
            if snapshots
                .selection
                .directories
                .len()
                .saturating_add(directories.len())
                > MAX_SCOPE_DIRS
            {
                snapshots.selection.partial = true;
                self.warn("the checkpoint directory count limit was reached");
                return Some(active.clone());
            }
            if directories.is_empty() {
                return Some(active.clone());
            }
            let selection = SnapshotSelection {
                directories,
                ..SnapshotSelection::default()
            };
            if !self.enroll(&mut snapshots, selection).await {
                return None;
            }
        }
        drop(snapshots);
        Some(active)
    }

    pub(crate) async fn begin_file_mutation(
        &self,
        path: &str,
        summary: String,
    ) -> Option<ActiveTurn> {
        let Some(relative) = checkpoint_relative(&self.root, path) else {
            self.warn("the file path is outside the checkpoint scope");
            return None;
        };
        if self.protected.contains(&relative) {
            self.warn("the live application database is excluded from file undo");
            return None;
        }
        let active = self.mutation_ticket(summary).await;
        let mut snapshots = active.snapshots.lock().await;
        if !snapshots.selection.files.contains(&relative) {
            if snapshots.selection.files.len() >= MAX_SCAN_FILES {
                snapshots.selection.partial = true;
                self.warn("the file checkpoint count limit was reached");
                return None;
            }
            let selection = SnapshotSelection {
                files: BTreeSet::from([relative]),
                ..SnapshotSelection::default()
            };
            if !self.enroll(&mut snapshots, selection).await {
                return None;
            }
        }
        drop(snapshots);
        Some(active)
    }

    async fn mutation_ticket(&self, summary: String) -> ActiveTurn {
        if let Some(active) = self.before.lock().unwrap().clone() {
            return active;
        }
        let thread_id = self.thread_id.lock().unwrap().clone();
        let turn_seq = self
            .store
            .list_workspace_checkpoints(&thread_id)
            .await
            .ok()
            .and_then(|items| items.into_iter().map(|item| item.turn_seq).max())
            .unwrap_or(0)
            + 1;
        ActiveTurn {
            snapshots: Arc::new(tokio::sync::Mutex::new(TurnSnapshots::default())),
            id: new_checkpoint_id(),
            thread_id,
            turn_seq,
            summary,
        }
    }

    async fn enroll(&self, snapshots: &mut TurnSnapshots, selection: SnapshotSelection) -> bool {
        let root = self.root.clone();
        let mut requested = selection.clone();
        requested
            .excluded
            .extend(self.automatic_exclusions.iter().cloned());
        requested.protected.extend(self.protected.iter().cloned());
        let retained = snapshots.before.retained_bytes();
        let captured =
            tokio::task::spawn_blocking(move || capture_selection(&root, &requested, retained))
                .await;
        match captured {
            Ok(Ok(before)) => {
                if before.partial {
                    self.report_partial();
                }
                snapshots.before.retain_first_observations(before);
                snapshots
                    .selection
                    .directories
                    .extend(selection.directories);
                snapshots.selection.files.extend(selection.files);
                snapshots.selection.partial |= selection.partial;
                snapshots
                    .selection
                    .excluded
                    .extend(self.automatic_exclusions.iter().cloned());
                snapshots
                    .selection
                    .protected
                    .extend(self.protected.iter().cloned());
                true
            }
            Ok(Err(error)) => {
                self.warn(error);
                false
            }
            Err(error) => {
                self.warn(error);
                false
            }
        }
    }

    pub(crate) async fn finish_mutation(&self, ticket: Option<ActiveTurn>) {
        if let Some(active) = ticket {
            self.persist_active(&active).await;
        }
    }

    async fn persist_active(&self, active: &ActiveTurn) {
        let snapshots = active.snapshots.lock().await;
        if snapshots.selection.files.is_empty() && snapshots.selection.directories.is_empty() {
            return;
        }
        let root = self.root.clone();
        let selection = snapshots.selection.clone();
        let after = match tokio::task::spawn_blocking(move || {
            capture_selection(&root, &selection, 0)
        })
        .await
        {
            Ok(Ok(snapshot)) => snapshot,
            Ok(Err(error)) => {
                self.warn(error);
                return;
            }
            Err(error) => {
                self.warn(error);
                return;
            }
        };
        let mut files = diff_snapshots(&snapshots.before, &after);
        let type_change = snapshots
            .before
            .files
            .keys()
            .any(|path| after.non_files.contains(path))
            || after
                .files
                .keys()
                .any(|path| snapshots.before.non_files.contains(path));
        let observation_partial =
            snapshots.before.partial || after.partial || snapshots.selection.partial || type_change;
        if observation_partial {
            match self
                .store
                .load_workspace_checkpoint(&active.thread_id, Some(&active.id))
                .await
            {
                Ok(Some(previous)) => {
                    for file in previous.files {
                        if (!snapshots.before.covers(&file.path) || !after.covers(&file.path))
                            && !files.iter().any(|current| current.path == file.path)
                        {
                            files.push(file);
                        }
                    }
                }
                Ok(None) => {}
                Err(error) => {
                    self.warn(error);
                    return;
                }
            }
        }
        if after.partial {
            self.report_partial();
        }
        if files.is_empty() {
            // An incomplete observation cannot disprove recorded effects.
            if observation_partial {
                return;
            }
            if let Err(error) = self.store.delete_workspace_checkpoint(&active.id).await {
                self.warn(error);
            }
            return;
        }
        let partial = observation_partial
            || !snapshots.selection.directories.contains("")
            || files.iter().any(|file| !file.restorable);
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
            self.warn(error);
        }
    }

    fn warn(&self, error: impl std::fmt::Display) {
        let message = format!("Workspace file checkpoint unavailable for this operation: {error}");
        self.emit_notice(&message);
    }

    fn report_partial(&self) {
        if !self.partial_notice_sent.swap(true, Ordering::Relaxed) {
            self.emit_notice("Workspace file checkpoints have partial coverage: unreadable paths or scan limits were encountered. Only captured changes can be rewound.");
        }
    }

    fn emit_notice(&self, message: &str) {
        if let Some(sink) = &self.warning_sink {
            sink(message);
        } else {
            eprintln!("craftsman: {message}");
        }
    }
}

#[async_trait]
impl AgentMiddleware for WorkspaceCheckpointManager {
    fn name(&self) -> &str {
        "workspace_checkpoint"
    }

    async fn turn_start(&self, state: &mut AgentState) -> Result<(), AgentError> {
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
            snapshots: Arc::new(tokio::sync::Mutex::new(TurnSnapshots::default())),
            id: new_checkpoint_id(),
            thread_id: self.thread_id.lock().unwrap().clone(),
            turn_seq,
            summary,
        });
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

fn within_directory(path: &str, directory: &str) -> bool {
    directory.is_empty() || Path::new(path).starts_with(directory)
}

fn checkpoint_relative(root: &Path, path: &str) -> Option<String> {
    let path = Path::new(path);
    let relative = if path.is_absolute() {
        path.strip_prefix(root).ok()?
    } else {
        path
    };
    let mut normalized = PathBuf::new();
    for component in relative.components() {
        match component {
            Component::Normal(name) => normalized.push(name),
            Component::CurDir => {}
            _ => return None,
        }
    }
    let value = normalize_relative_path(normalized.to_str()?);
    (value.len() <= 4096).then_some(value)
}

#[cfg(test)]
fn capture_snapshot(root: &Path) -> Result<WorkspaceSnapshot, String> {
    capture_selection(
        root,
        &SnapshotSelection {
            directories: BTreeSet::from([String::new()]),
            ..SnapshotSelection::default()
        },
        0,
    )
}

fn capture_selection(
    root: &Path,
    selection: &SnapshotSelection,
    retained: u64,
) -> Result<WorkspaceSnapshot, String> {
    let enumeration = enumerate_paths_excluding(root, &selection.directories, &selection.excluded)?;
    let mut snapshot = WorkspaceSnapshot {
        unavailable: enumeration.unavailable,
        non_files: enumeration.non_files,
        partial: enumeration.partial || selection.partial,
        ..WorkspaceSnapshot::default()
    };
    let mut budget = SnapshotBudget {
        retained,
        hashed: 0,
    };
    let paths = selection.files.iter().cloned().chain(
        enumeration
            .paths
            .into_iter()
            .filter(|path| !selection.files.contains(path)),
    );
    for (index, relative) in paths.enumerate() {
        if selection.protected.contains(&relative) {
            snapshot.unavailable.insert(relative);
            snapshot.partial = true;
            continue;
        }
        if index >= MAX_SCAN_FILES || budget.hashed >= MAX_SNAPSHOT_BYTES {
            snapshot.unavailable.insert(relative);
            snapshot.partial = true;
            continue;
        }
        let Some(_) = safe_join(root, &relative) else {
            continue;
        };
        match capture_snapshot_file(root, &relative, &mut budget) {
            Ok(file) => {
                snapshot.observed.insert(relative.clone());
                if let Some(file) = file {
                    snapshot.files.insert(relative, file);
                }
            }
            Err(SnapshotReadError::Budget | SnapshotReadError::Unsupported) => {
                snapshot.unavailable.insert(relative);
                snapshot.partial = true;
            }
            Err(SnapshotReadError::Io {
                operation: _,
                source,
            }) if source.kind() == std::io::ErrorKind::PermissionDenied => {
                snapshot.unavailable.insert(relative);
                snapshot.partial = true;
            }
            Err(SnapshotReadError::Io { operation, source }) => {
                return Err(format!("{operation} {relative}: {source}"));
            }
        }
    }
    let unavailable = Arc::new(snapshot.unavailable.clone());
    snapshot.directories = selection
        .directories
        .iter()
        .map(|dir| (dir.clone(), unavailable.clone()))
        .collect();
    Ok(snapshot)
}

#[derive(Default)]
struct EnumeratedPaths {
    paths: BTreeSet<String>,
    unavailable: BTreeSet<String>,
    non_files: BTreeSet<String>,
    partial: bool,
}

#[cfg(test)]
fn enumerate_paths(root: &Path, directories: &BTreeSet<String>) -> Result<EnumeratedPaths, String> {
    enumerate_paths_excluding(root, directories, &BTreeSet::new())
}

fn enumerate_paths_excluding(
    root: &Path,
    directories: &BTreeSet<String>,
    excluded: &BTreeSet<String>,
) -> Result<EnumeratedPaths, String> {
    let mut result = EnumeratedPaths::default();
    result.unavailable.extend(excluded.iter().cloned());
    if directories.is_empty() {
        return Ok(result);
    }
    let mut command = std::process::Command::new("git");
    command
        .arg("-C")
        .arg(root)
        .args(["ls-files", "-co", "--exclude-standard", "-z"]);
    if !directories.contains("") {
        command.arg("--");
        for directory in directories {
            command.arg(format!(":(literal){directory}"));
        }
    }
    if let Some((output, limited)) = bounded_git_listing(command)? {
        if limited {
            result.partial = true;
            result.unavailable.extend(directories.iter().cloned());
        }
        for (index, raw) in output
            .split_inclusive(|byte| *byte == 0)
            .filter(|raw| raw.len() > 1 && raw.last() == Some(&0))
            .map(|raw| &raw[..raw.len() - 1])
            .enumerate()
        {
            if index >= MAX_SCAN_ENTRIES {
                result.partial = true;
                result.unavailable.extend(directories.iter().cloned());
                break;
            }
            if let Ok(path) = std::str::from_utf8(raw) {
                let relative = normalize_relative_path(path);
                if !excluded
                    .iter()
                    .any(|prefix| within_directory(&relative, prefix))
                {
                    result.paths.insert(relative);
                }
            }
        }
        return Ok(result);
    }

    let mut stack = directories
        .iter()
        .map(|dir| root.join(dir))
        .collect::<Vec<_>>();
    let mut examined = 0usize;
    while let Some(directory) = stack.pop() {
        let relative_directory = normalize_relative_path(
            directory
                .strip_prefix(root)
                .unwrap_or(Path::new(""))
                .to_str()
                .unwrap_or(""),
        );
        if excluded
            .iter()
            .any(|prefix| within_directory(&relative_directory, prefix))
        {
            continue;
        }
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                result.unavailable.insert(relative_directory);
                result.partial = true;
                continue;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && directory != root => {
                continue;
            }
            Err(error) => return Err(format!("could not scan {}: {error}", directory.display())),
        };
        for entry in entries {
            if examined >= MAX_SCAN_ENTRIES {
                result.unavailable.extend(directories.iter().cloned());
                result.partial = true;
                return Ok(result);
            }
            examined += 1;
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    result.unavailable.insert(relative_directory.clone());
                    result.partial = true;
                    continue;
                }
            };
            let path = entry.path();
            let relative_path = path
                .strip_prefix(root)
                .ok()
                .and_then(Path::to_str)
                .map(normalize_relative_path);
            if relative_path
                .as_ref()
                .is_some_and(|path| excluded.iter().any(|prefix| within_directory(path, prefix)))
            {
                continue;
            }
            let metadata = match std::fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(_) => {
                    if let Ok(relative) = path.strip_prefix(root)
                        && let Some(relative) = relative.to_str()
                    {
                        result.unavailable.insert(normalize_relative_path(relative));
                    }
                    result.partial = true;
                    continue;
                }
            };
            if metadata.file_type().is_symlink() {
                if let Some(relative) = relative_path {
                    result.non_files.insert(relative);
                }
                result.partial = true;
                continue;
            }
            if metadata.is_dir() {
                if let Some(relative) = relative_path {
                    result.non_files.insert(relative);
                }
                if is_skipped_dir(path.file_name()) {
                    continue;
                }
                stack.push(path);
            } else if metadata.is_file()
                && let Ok(relative) = path.strip_prefix(root)
                && let Some(relative) = relative.to_str()
            {
                result.paths.insert(normalize_relative_path(relative));
            } else {
                if let Some(relative) = relative_path {
                    result.non_files.insert(relative);
                }
                result.partial = true;
            }
        }
    }
    Ok(result)
}

#[cfg(windows)]
fn normalize_relative_path(path: &str) -> String {
    path.replace('\\', "/")
}

#[cfg(not(windows))]
fn normalize_relative_path(path: &str) -> String {
    path.to_string()
}

fn bounded_git_listing(
    mut command: std::process::Command,
) -> Result<Option<(Vec<u8>, bool)>, String> {
    use std::process::Stdio;
    let Ok(mut child) = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return Ok(None);
    };
    let mut output = Vec::new();
    let read = child
        .stdout
        .take()
        .expect("piped git output")
        .take(MAX_GIT_LIST_BYTES + 1)
        .read_to_end(&mut output);
    let limited = output.len() as u64 > MAX_GIT_LIST_BYTES;
    if limited || read.is_err() {
        let _ = child.kill();
    }
    let status = child
        .wait()
        .map_err(|error| format!("could not finish checkpoint file listing: {error}"))?;
    read.map_err(|error| format!("could not read checkpoint file listing: {error}"))?;
    if limited || status.success() {
        Ok(Some((output, limited)))
    } else {
        Ok(None)
    }
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
            libc::O_RDONLY | libc::O_NONBLOCK | libc::O_CLOEXEC | libc::O_NOFOLLOW,
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

#[derive(Default)]
struct SnapshotBudget {
    retained: u64,
    hashed: u64,
}

#[derive(Debug)]
enum SnapshotReadError {
    Io {
        operation: &'static str,
        source: std::io::Error,
    },
    Budget,
    Unsupported,
}

fn snapshot_io(operation: &'static str, source: std::io::Error) -> SnapshotReadError {
    SnapshotReadError::Io { operation, source }
}

fn snapshot_file_contents(
    file: &mut std::fs::File,
    metadata: &std::fs::Metadata,
    budget: &mut SnapshotBudget,
) -> Result<SnapshotFile, SnapshotReadError> {
    let remaining = MAX_SNAPSHOT_BYTES.saturating_sub(budget.hashed);
    if metadata.len() > remaining {
        return Err(SnapshotReadError::Budget);
    }
    let retain = metadata.len() <= MAX_FILE_BYTES
        && budget.retained.saturating_add(metadata.len()) <= MAX_SNAPSHOT_BYTES;
    let mut bytes = retain.then(Vec::new);
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut actual = 0u64;
    loop {
        let available = MAX_SNAPSHOT_BYTES.saturating_sub(budget.hashed);
        let amount = (available.saturating_add(1)).min(buffer.len() as u64) as usize;
        let read = file
            .read(&mut buffer[..amount])
            .map_err(|error| snapshot_io("could not read", error))?;
        if read == 0 {
            break;
        }
        budget.hashed = budget.hashed.saturating_add(read as u64);
        actual = actual.saturating_add(read as u64);
        if budget.hashed > MAX_SNAPSHOT_BYTES {
            return Err(SnapshotReadError::Budget);
        }
        if actual > MAX_FILE_BYTES || budget.retained.saturating_add(actual) > MAX_SNAPSHOT_BYTES {
            bytes = None;
        }
        hasher.update(&buffer[..read]);
        if let Some(bytes) = &mut bytes {
            bytes.extend_from_slice(&buffer[..read]);
        }
    }
    if let Some(bytes) = &bytes {
        budget.retained = budget.retained.saturating_add(bytes.len() as u64);
    }
    Ok(SnapshotFile {
        hash: format!("{:x}", hasher.finalize()),
        mode: file_mode(metadata),
        content: bytes,
    })
}

#[cfg(unix)]
fn capture_snapshot_file(
    root: &Path,
    relative: &str,
    budget: &mut SnapshotBudget,
) -> Result<Option<SnapshotFile>, SnapshotReadError> {
    use std::os::fd::{AsRawFd, FromRawFd};
    let (parent, name) = match open_relative_parent(root, relative, false) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(snapshot_io("could not safely open", error)),
    };
    let mut info = std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe {
        libc::fstatat(
            parent.as_raw_fd(),
            name.as_ptr(),
            info.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } < 0
    {
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::NotFound {
            return Ok(None);
        }
        return Err(snapshot_io("could not inspect", error));
    }
    let info = unsafe { info.assume_init() };
    if info.st_mode & libc::S_IFMT != libc::S_IFREG {
        return Err(SnapshotReadError::Unsupported);
    }
    let raw = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NONBLOCK | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if raw < 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::NotFound {
            return Ok(None);
        }
        return Err(snapshot_io("could not safely open", error));
    }
    let mut file = unsafe { std::fs::File::from_raw_fd(raw) };
    let metadata = file
        .metadata()
        .map_err(|error| snapshot_io("could not inspect", error))?;
    if !metadata.is_file() {
        return Err(SnapshotReadError::Unsupported);
    }
    snapshot_file_contents(&mut file, &metadata, budget).map(Some)
}

#[cfg(not(unix))]
fn capture_snapshot_file(
    root: &Path,
    relative: &str,
    budget: &mut SnapshotBudget,
) -> Result<Option<SnapshotFile>, SnapshotReadError> {
    let Some(path) = safe_join(root, relative) else {
        return Ok(None);
    };
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_file() => metadata,
        Ok(_) => return Err(SnapshotReadError::Unsupported),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(snapshot_io("could not inspect", error)),
    };
    let mut file =
        std::fs::File::open(&path).map_err(|error| snapshot_io("could not read", error))?;
    snapshot_file_contents(&mut file, &metadata, budget).map(Some)
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
        if !before.covers(&path) || !after.covers(&path) {
            continue;
        }
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
#[path = "workspace_checkpoint/scoped_tests.rs"]
mod scoped_tests;

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
        manager
            .begin_mutation("fixture mutation".into())
            .await
            .unwrap();
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
        manager
            .begin_mutation("fixture mutation".into())
            .await
            .unwrap();
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
            ..WorkspaceSnapshot::default()
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
            ..WorkspaceSnapshot::default()
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
        manager
            .begin_mutation("fixture mutation".into())
            .await
            .unwrap();
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
        manager
            .begin_mutation("fixture mutation".into())
            .await
            .unwrap();
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
        manager
            .begin_mutation("fixture mutation".into())
            .await
            .unwrap();
        std::fs::write(manager.root.join("a.txt"), b"a").unwrap();
        manager.turn_end(&mut state).await.unwrap();
        assert_eq!(manager.list().await.unwrap().len(), 1);
        manager.set_thread_id("thread-2");
        assert!(manager.list().await.unwrap().is_empty());
    }
}
