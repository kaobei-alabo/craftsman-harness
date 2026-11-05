//! Handle-relative filesystem authority for headless R1/R3.
//!
//! This backend never checks a path and then delegates an ambient absolute
//! reopen to RealFs. All tool IO uses the retained root capability, no-follow
//! component walks and blocking workers. Its immutable invocation policy also
//! applies when a delegated/custom tool calls the Backend directly.
//!
//! The private prepared runtime selects this backend; no CLI is exposed.
//! Native shell execution is not contained by this filesystem. Byte-oriented
//! guest adapters must reuse this same authority, not reopen ambient paths.

mod view;
pub(crate) use view::TextFs;

use super::permissions::{InvocationPermissions, ToolDecision};
use async_trait::async_trait;
use cap_fs_ext::{DirExt, MetadataExt};
use cap_std::fs::{Dir, File, Metadata};
use craftsman_harness::backend::{cat_n, glob_match};
use craftsman_harness::{AgentError, Backend, FileInfo};
use serde_json::json;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use thiserror::Error;
use unicode_normalization::UnicodeNormalization;

const MAX_PATH_CHARS: usize = 4096;
const MAX_DEPTH: usize = 128;
const MAX_FILE_BYTES: usize = 10 * 1024 * 1024;
pub(super) const MAX_INPUT_FILE_BYTES: usize = 1024 * 1024;
const MAX_OUTPUT_BYTES: usize = 100_000;
const MAX_WALK_ENTRIES: usize = 100_000;
const MAX_HOST_DIRECTORY_ENTRIES: usize = 1024;
const MAX_WALK_NAME_BYTES: usize = 8 * 1024 * 1024;
const MAX_SEARCH_BYTES: usize = 64 * 1024 * 1024;
const MAX_MATCH_WORK: usize = 16 * 1024 * 1024;
const TEMP_PREFIX: &str = ".craftsman-exec-tmp-";
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

type Result<T> = std::result::Result<T, FsError>;

/// Diagnostics never contain a caller path, contents, or arbitrary OS error text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub(crate) enum FsError {
    #[error("invalid workspace or configuration scope")]
    InvalidScope,
    #[error("file path is outside the canonical workspace")]
    OutsideRoot,
    #[error("invalid or ambiguous workspace path")]
    InvalidPath,
    #[error("this filesystem tool is explicitly denied")]
    ToolDenied,
    #[error("this filesystem tool requires an explicit invocation allow")]
    Unapproved,
    #[error("workspace path is denied by the invocation policy")]
    PathDenied,
    #[error("configuration, credentials or runtime artifacts are hard-denied")]
    HardDenied,
    #[error("protected writes require an exact file allow and tool permission")]
    ProtectedWrite,
    #[error("file selectors must name files, not directories or special files")]
    InvalidSelector,
    #[error("workspace entry does not exist")]
    NotFound,
    #[error("workspace path is not a directory")]
    NotDirectory,
    #[error("only regular workspace files may be accessed")]
    NotFile,
    #[error("symlinked workspace paths are not permitted")]
    Symlink,
    #[error("hard-linked files are ambiguous and not permitted")]
    HardLink,
    #[error("cannot create a file that already exists; read and edit it instead")]
    Exists,
    #[error("workspace file exceeds the 10 MiB operation bound")]
    FileTooLarge,
    #[error("input source exceeds its bounded file or aggregate allowance")]
    InputTooLarge,
    #[error("workspace file is not valid UTF-8 text")]
    InvalidUtf8,
    #[error("filesystem traversal or result exceeds its resource bound")]
    OutputLimit,
    #[error("read offset is past the end of the file")]
    Offset,
    #[error("old_string must not be empty")]
    EmptyEdit,
    #[error("old_string was not found in the file")]
    NoMatch,
    #[error("old_string is not unique; use replace_all or more context")]
    NonUnique,
    #[error("workspace path or file changed during the operation; read it again")]
    Changed,
    #[error("confined filesystem IO failed")]
    Io,
    #[error("confined filesystem worker failed")]
    Worker,
}

impl From<FsError> for AgentError {
    fn from(error: FsError) -> Self {
        AgentError::State(error.to_string())
    }
}

fn io(error: std::io::Error) -> FsError {
    match error.kind() {
        std::io::ErrorKind::NotFound => FsError::NotFound,
        std::io::ErrorKind::AlreadyExists => FsError::Exists,
        _ => FsError::Io,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Identity(u64, u64);

fn identity(metadata: &Metadata) -> Identity {
    // Metadata is always obtained from an open file/directory handle here;
    // Windows' by-handle identity fields are consequently populated.
    Identity(metadata.dev(), metadata.ino())
}

#[derive(Clone)]
pub(crate) struct ConfinedFs {
    inner: Arc<Inner>,
}

struct Inner {
    root: PathBuf,
    directory: Dir,
    config_paths: Vec<String>,
    config_id: Option<Identity>,
    permissions: Arc<InvocationPermissions>,
    operations: Mutex<()>,
    #[cfg(test)]
    hook: Mutex<Option<Hook>>,
}

#[cfg(test)]
type Hook = Arc<dyn Fn(&str, &str) + Send + Sync>;

struct Parent {
    directory: Dir,
    parts: Vec<String>,
    ids: Vec<Identity>,
}

impl ConfinedFs {
    /// Trusted bootstrap only: both scopes must be absolute. The root must
    /// exist; config may not exist yet, but its nearest ancestor is resolved.
    /// The eventual frontend must construct this on a blocking worker too.
    pub(crate) fn new(
        root: &Path,
        config: &Path,
        permissions: Arc<InvocationPermissions>,
    ) -> Result<Self> {
        if !root.is_absolute() || !config.is_absolute() {
            return Err(FsError::InvalidScope);
        }
        let directory = Dir::open_ambient_dir(root, cap_std::ambient_authority()).map_err(io)?;
        let root_id = identity(&directory.dir_metadata().map_err(io)?);
        let canonical = root.canonicalize().map_err(io)?;
        let confirm =
            Dir::open_ambient_dir(&canonical, cap_std::ambient_authority()).map_err(io)?;
        if root_id != identity(&confirm.dir_metadata().map_err(io)?) || canonical.to_str().is_none()
        {
            return Err(FsError::Changed);
        }
        let config_canonical = resolve_config(config)?;
        // A workspace nested in the selected config directory has no permitted
        // file scope. Reject rather than silently expose part of that directory.
        if relative_scope(&config_canonical, &canonical).is_some() {
            return Err(FsError::InvalidScope);
        }
        let config_id = match Dir::open_ambient_dir(&config_canonical, cap_std::ambient_authority())
        {
            Ok(dir) => Some(identity(&dir.dir_metadata().map_err(io)?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => return Err(FsError::InvalidScope),
        };
        let mut config_paths = Vec::new();
        for path in [config, config_canonical.as_path()] {
            if let Some(relative) = relative_scope(&canonical, path) {
                if !relative.is_empty() && !config_paths.contains(&relative) {
                    config_paths.push(relative);
                }
            }
        }
        let inner = Inner {
            root: canonical,
            directory,
            config_paths,
            config_id,
            permissions,
            operations: Mutex::new(()),
            #[cfg(test)]
            hook: Mutex::new(None),
        };
        for (path, allow) in inner.permissions.file_selectors() {
            let parts = inner.relative(path, false)?;
            if allow && inner.hard_denied(&parts) {
                return Err(FsError::InvalidSelector);
            }
            inner.validate_selector(&parts)?;
        }
        Ok(Self {
            inner: Arc::new(inner),
        })
    }

    pub(crate) fn root(&self) -> &Path {
        &self.inner.root
    }

    /// Bootstrap discovery reuses the already-authorized workspace handle,
    /// never an ambient reopened pathname after config/session preparation.
    pub(super) fn discovery_directory(&self) -> std::io::Result<Dir> {
        self.inner.directory.try_clone()
    }

    /// Binary-safe host IO: no cat-n framing, UTF-8 replacement or ambient
    /// reopening. The same tool/path policy and 10 MiB raw-file bound apply.
    pub(crate) async fn read_raw(&self, path: &str) -> Result<Vec<u8>> {
        let path = checked_path(path)?;
        self.run_typed(move |fs| {
            let parts = fs.relative(&path, false)?;
            fs.authorize("read_file", &parts, false)?;
            fs.read_bytes(&parts)
        })
        .await
    }

    /// Pre-run input uses the SAME authority and policy with a stricter cap.
    /// Caller limits can only tighten the fixed 1 MiB input ceiling, never
    /// widen ordinary/guest reads or bypass no-follow/type/identity checks.
    pub(super) async fn read_input_raw(&self, path: &str, maximum: usize) -> Result<Vec<u8>> {
        if maximum > MAX_INPUT_FILE_BYTES {
            return Err(FsError::InputTooLarge);
        }
        let path = checked_path(path)?;
        self.run_typed(move |fs| {
            let parts = fs.relative(&path, false)?;
            fs.authorize("read_file", &parts, false)?;
            fs.read_bytes_with_limit(&parts, ReadLimit::Input(maximum))
        })
        .await
    }

    pub(crate) async fn write_raw(&self, path: &str, contents: &[u8]) -> Result<()> {
        let path = checked_path(path)?;
        if contents.len() > MAX_FILE_BYTES {
            return Err(FsError::FileTooLarge);
        }
        let contents = contents.to_vec();
        self.run_typed(move |fs| {
            let parts = fs.relative(&path, false)?;
            fs.authorize("write_file", &parts, true)?;
            fs.write_file(&parts, &contents)
        })
        .await
    }

    /// Guest-facing listing charges ALL root entries before filtering, matching
    /// the existing gadget host's 1024-entry bound; names remain deterministic.
    pub(crate) async fn list_raw(&self, path: &str) -> Result<Vec<FileInfo>> {
        let path = checked_path(path)?;
        self.run_typed(move |fs| {
            let parts = fs.relative(&path, true)?;
            fs.authorize("ls", &parts, false)?;
            fs.list_bounded(&parts, MAX_HOST_DIRECTORY_ENTRIES)
        })
        .await
    }

    async fn run<T, F>(&self, operation: F) -> std::result::Result<T, AgentError>
    where
        T: Send + 'static,
        F: FnOnce(&Inner) -> Result<T> + Send + 'static,
    {
        self.run_typed(operation).await.map_err(Into::into)
    }

    async fn run_typed<T, F>(&self, operation: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&Inner) -> Result<T> + Send + 'static,
    {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let _guard = inner.operations.lock().map_err(|_| FsError::Worker)?;
            operation(&inner)
        })
        .await
        .map_err(|_| FsError::Worker)?
    }
}

impl Inner {
    fn relative(&self, raw: &str, root_allowed: bool) -> Result<Vec<String>> {
        if raw.chars().count() > MAX_PATH_CHARS {
            return Err(FsError::InvalidPath);
        }
        let input = Path::new(raw);
        let path = if input.is_absolute() {
            input
                .strip_prefix(&self.root)
                .map_err(|_| FsError::OutsideRoot)?
        } else {
            input
        };
        let mut parts = Vec::new();
        for component in path.components() {
            match component {
                Component::Normal(name) => {
                    let name = name.to_str().ok_or(FsError::InvalidPath)?;
                    valid_name(name)?;
                    parts.push(name.to_owned());
                }
                Component::CurDir => {}
                _ => return Err(FsError::InvalidPath),
            }
        }
        if parts.len() > MAX_DEPTH || (!root_allowed && parts.is_empty()) {
            return Err(FsError::InvalidPath);
        }
        Ok(parts)
    }

    fn hard_denied(&self, parts: &[String]) -> bool {
        let relative = parts.join("/");
        parts.iter().any(|p| {
            let lower = p.to_ascii_lowercase();
            lower.starts_with(".env") || lower.starts_with(TEMP_PREFIX)
        }) || self
            .config_paths
            .iter()
            .any(|config| within_policy(&relative, config))
    }

    fn path_check(&self, parts: &[String], write: bool) -> Result<()> {
        if self.hard_denied(parts) {
            return Err(FsError::HardDenied);
        }
        let relative = parts.join("/");
        if self
            .permissions
            .file_selectors()
            .any(|(deny, allow)| !allow && within_policy(&relative, deny))
        {
            return Err(FsError::PathDenied);
        }
        if write {
            if parts
                .iter()
                .any(|p| p.eq_ignore_ascii_case(".git") || p.eq_ignore_ascii_case(".craftsman"))
            {
                return Err(FsError::HardDenied);
            }
            if protected_write(parts)
                && !self
                    .permissions
                    .protected_write_is_explicitly_allowed(&relative)
            {
                return Err(FsError::ProtectedWrite);
            }
        }
        Ok(())
    }

    fn authorize(&self, tool: &str, parts: &[String], write: bool) -> Result<()> {
        self.path_check(parts, write)?;
        match self.permissions.tool_decision(tool, &json!({})) {
            ToolDecision::Allow => Ok(()),
            ToolDecision::Deny => Err(FsError::ToolDenied),
            ToolDecision::Unapproved => Err(FsError::Unapproved),
        }
    }

    fn check_dir(&self, directory: &Dir) -> Result<Identity> {
        let id = identity(&directory.dir_metadata().map_err(io)?);
        if self.config_id == Some(id) {
            return Err(FsError::HardDenied);
        }
        Ok(id)
    }

    fn open_dir(&self, parts: &[String], create: bool, protect: bool) -> Result<Parent> {
        let mut directory = self.directory.try_clone().map_err(io)?;
        let mut ids = Vec::new();
        for name in parts {
            match directory.symlink_metadata(name) {
                Ok(meta) if meta.file_type().is_symlink() => return Err(FsError::Symlink),
                Ok(meta) if !meta.is_dir() => return Err(FsError::NotDirectory),
                Ok(_) => {}
                Err(e) if create && e.kind() == std::io::ErrorKind::NotFound => {
                    match crate::platform_os::confined_create_dir(&directory, name) {
                        Ok(()) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                        Err(e) => return Err(io(e)),
                    }
                }
                Err(e) => return Err(io(e)),
            }
            directory = directory.open_dir_nofollow(name).map_err(io)?;
            let id = if protect {
                self.check_dir(&directory)?
            } else {
                identity(&directory.dir_metadata().map_err(io)?)
            };
            ids.push(id);
        }
        Ok(Parent {
            directory,
            parts: parts.to_vec(),
            ids,
        })
    }

    fn validate_parent(&self, parent: &Parent) -> Result<()> {
        let current = self
            .open_dir(&parent.parts, false, true)
            .map_err(|_| FsError::Changed)?;
        if current.ids != parent.ids {
            return Err(FsError::Changed);
        }
        Ok(())
    }

    fn parent(&self, parts: &[String], create: bool) -> Result<Parent> {
        self.open_dir(&parts[..parts.len() - 1], create, true)
    }

    fn validate_selector(&self, parts: &[String]) -> Result<()> {
        let parent = match self.open_dir(&parts[..parts.len() - 1], false, false) {
            Ok(parent) => parent,
            Err(FsError::NotFound) => return Ok(()),
            Err(_) => return Err(FsError::InvalidSelector),
        };
        match parent.directory.symlink_metadata(parts.last().unwrap()) {
            Ok(meta) if meta.is_file() && !meta.file_type().is_symlink() => Ok(()),
            Ok(_) => Err(FsError::InvalidSelector),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(FsError::InvalidSelector),
        }
    }

    fn opened_file(&self, parent: &Parent, name: &str) -> Result<File> {
        let before = parent.directory.symlink_metadata(name).map_err(io)?;
        if before.file_type().is_symlink() {
            return Err(FsError::Symlink);
        }
        if !before.is_file() {
            return Err(FsError::NotFile);
        }
        let mut target = parent.parts.clone();
        target.push(name.to_owned());
        self.stage("open", &target);
        let file = parent
            .directory
            .open_with(
                name,
                &crate::platform_os::confined_file_options(false, false),
            )
            .map_err(io)?;
        let metadata = file.metadata().map_err(io)?;
        regular(&metadata)?;
        self.validate_parent(parent)?;
        Ok(file)
    }

    fn read_bytes(&self, parts: &[String]) -> Result<Vec<u8>> {
        self.read_bytes_with_limit(parts, ReadLimit::Tool)
    }

    fn read_bytes_with_limit(&self, parts: &[String], limit: ReadLimit) -> Result<Vec<u8>> {
        let parent = self.parent(parts, false)?;
        self.stage("parent", parts);
        self.validate_parent(&parent)?;
        let name = parts.last().unwrap();
        let mut file = self.opened_file(&parent, name)?;
        let before = file.metadata().map_err(io)?;
        self.stage("read", parts);
        self.validate_entry(&parent, name, identity(&before))?;
        if matches!(limit, ReadLimit::Input(_)) && before.len() > limit.maximum() as u64 {
            return Err(limit.overflow());
        }
        let bytes = bounded_read_with_limit(&mut file, limit)?;
        let after = file.metadata().map_err(io)?;
        if matches!(limit, ReadLimit::Input(_)) && bytes.len() as u64 != before.len() {
            return Err(FsError::Changed);
        }
        if before.len() != after.len() || before.modified().ok() != after.modified().ok() {
            return Err(FsError::Changed);
        }
        regular(&after)?;
        self.validate_entry(&parent, name, identity(&before))?;
        Ok(bytes)
    }

    fn validate_entry(&self, parent: &Parent, name: &str, expected: Identity) -> Result<()> {
        let file = self
            .opened_file(parent, name)
            .map_err(|_| FsError::Changed)?;
        if identity(&file.metadata().map_err(io)?) != expected {
            return Err(FsError::Changed);
        }
        Ok(())
    }

    fn write_file(&self, parts: &[String], content: &[u8]) -> Result<()> {
        if content.len() > MAX_FILE_BYTES {
            return Err(FsError::FileTooLarge);
        }
        let parent = self.parent(parts, true)?;
        self.stage("parent", parts);
        self.validate_parent(&parent)?;
        let name = parts.last().unwrap();
        match parent.directory.symlink_metadata(name) {
            Ok(_) => return Err(FsError::Exists),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(io(e)),
        }
        let mut temporary = Temporary::new(&parent.directory)?;
        temporary.file.write_all(content).map_err(io)?;
        temporary.file.sync_all().map_err(io)?;
        self.stage("publish", parts);
        self.validate_parent(&parent)?;
        temporary.validate()?;
        // A hard link is an atomic create-only publication, never replacement.
        // No fallback to an ambient write or overwrite on unsupported filesystems.
        parent
            .directory
            .hard_link(&temporary.name, &parent.directory, name)
            .map_err(io)?;
        if identity(
            &parent
                .directory
                .open_with(
                    name,
                    &crate::platform_os::confined_file_options(false, false),
                )
                .map_err(io)?
                .metadata()
                .map_err(io)?,
        ) != temporary.id
        {
            return Err(FsError::Changed);
        }
        Ok(())
    }

    fn edit_file(&self, parts: &[String], old: &str, new: &str, replace_all: bool) -> Result<()> {
        if old.is_empty() {
            return Err(FsError::EmptyEdit);
        }
        let parent = self.parent(parts, false)?;
        self.stage("parent", parts);
        self.validate_parent(&parent)?;
        let name = parts.last().unwrap();
        let mut file = self.opened_file(&parent, name)?;
        let metadata = file.metadata().map_err(io)?;
        let original = bounded_read(&mut file)?;
        let text = std::str::from_utf8(&original).map_err(|_| FsError::InvalidUtf8)?;
        let count = text.matches(old).count();
        if count == 0 {
            return Err(FsError::NoMatch);
        }
        if count > 1 && !replace_all {
            return Err(FsError::NonUnique);
        }
        let added = new.len().saturating_sub(old.len());
        let times = if replace_all { count } else { 1 };
        if original.len().saturating_add(added.saturating_mul(times)) > MAX_FILE_BYTES {
            return Err(FsError::FileTooLarge);
        }
        let updated = if replace_all {
            text.replace(old, new)
        } else {
            text.replacen(old, new, 1)
        };
        let mut temporary = Temporary::new(&parent.directory)?;
        temporary.file.write_all(updated.as_bytes()).map_err(io)?;
        temporary
            .file
            .set_permissions(crate::platform_os::confined_replace_permissions(
                metadata.permissions(),
            ))
            .map_err(io)?;
        temporary.file.sync_all().map_err(io)?;
        self.stage("publish", parts);
        self.validate_entry(&parent, name, identity(&metadata))?;
        let mut current = self.opened_file(&parent, name)?;
        if bounded_read(&mut current)? != original {
            return Err(FsError::Changed);
        }
        temporary.validate()?;
        self.validate_parent(&parent)?;
        // Destination rename does not follow a planted final symlink. Own edits
        // serialize; external content changes are checked, not a portable CAS.
        parent
            .directory
            .rename(&temporary.name, &parent.directory, name)
            .map_err(io)?;
        Ok(())
    }

    fn list(&self, parts: &[String]) -> Result<Vec<FileInfo>> {
        self.list_bounded(parts, MAX_WALK_ENTRIES)
    }

    fn list_bounded(&self, parts: &[String], max_entries: usize) -> Result<Vec<FileInfo>> {
        let parent = self.open_dir(parts, false, true)?;
        self.stage("parent", parts);
        self.validate_parent(&parent)?;
        let mut entries = Vec::new();
        let mut bytes = 0usize;
        let mut count = 0usize;
        for item in parent.directory.entries().map_err(io)? {
            count += 1;
            if count > max_entries {
                return Err(FsError::OutputLimit);
            }
            let item = item.map_err(io)?;
            let Some(name) = item.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if valid_name(&name).is_err() {
                continue;
            }
            let mut child = parts.to_vec();
            child.push(name.clone());
            if self.path_check(&child, false).is_err() {
                continue;
            }
            let kind = item.file_type().map_err(io)?;
            let entry = if kind.is_dir() {
                let dir = match self.open_dir(&child, false, true) {
                    Ok(dir) => dir,
                    Err(_) => continue,
                };
                self.validate_parent(&dir)?;
                FileInfo {
                    path: format!("{name}/"),
                    size: 0,
                }
            } else if kind.is_file() {
                let file = match self.opened_file(&parent, &name) {
                    Ok(file) => file,
                    Err(_) => continue,
                };
                FileInfo {
                    path: name,
                    size: file.metadata().map_err(io)?.len(),
                }
            } else {
                continue;
            };
            bytes = bytes.saturating_add(entry.path.len());
            if bytes > MAX_OUTPUT_BYTES {
                return Err(FsError::OutputLimit);
            }
            entries.push(entry);
        }
        self.validate_parent(&parent)?;
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(entries)
    }

    fn walk_files(&self, start: &[String]) -> Result<Vec<Vec<String>>> {
        let mut stack = vec![start.to_vec()];
        let mut result = Vec::new();
        let mut count = 0usize;
        let mut names = 0usize;
        while let Some(parts) = stack.pop() {
            let parent = match self.open_dir(&parts, false, true) {
                Ok(parent) => parent,
                Err(_) if parts != start => continue,
                Err(error) => return Err(error),
            };
            self.validate_parent(&parent)?;
            let mut children = Vec::new();
            for item in parent.directory.entries().map_err(io)? {
                count += 1;
                if count > MAX_WALK_ENTRIES {
                    return Err(FsError::OutputLimit);
                }
                let item = item.map_err(io)?;
                let Some(name) = item.file_name().to_str().map(str::to_owned) else {
                    continue;
                };
                if valid_name(&name).is_err() {
                    continue;
                }
                let mut child = parts.clone();
                child.push(name.clone());
                if child.len() > MAX_DEPTH || child.join("/").chars().count() > MAX_PATH_CHARS {
                    return Err(FsError::OutputLimit);
                }
                if self.path_check(&child, false).is_err() {
                    continue;
                }
                names = names.saturating_add(child.join("/").len());
                if names > MAX_WALK_NAME_BYTES {
                    return Err(FsError::OutputLimit);
                }
                let kind = item.file_type().map_err(io)?;
                if kind.is_dir() {
                    if matches!(
                        name.to_ascii_lowercase().as_str(),
                        ".git" | "node_modules" | "target"
                    ) {
                        continue;
                    }
                    children.push(child);
                } else if kind.is_file() && self.opened_file(&parent, &name).is_ok() {
                    result.push(child);
                }
            }
            self.validate_parent(&parent)?;
            children.sort();
            stack.extend(children.into_iter().rev());
        }
        result.sort();
        Ok(result)
    }

    fn stage(&self, _stage: &str, _parts: &[String]) {
        #[cfg(test)]
        {
            let hook = self.hook.lock().unwrap().clone();
            if let Some(hook) = hook {
                hook(_stage, &_parts.join("/"));
            }
        }
    }
}

struct Temporary {
    directory: Dir,
    name: String,
    file: File,
    id: Identity,
}

impl Temporary {
    fn new(parent: &Dir) -> Result<Self> {
        for _ in 0..16 {
            let name = format!(
                "{TEMP_PREFIX}{}-{}",
                std::process::id(),
                NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
            );
            match parent.open_with(
                &name,
                &crate::platform_os::confined_file_options(true, true),
            ) {
                Ok(file) => {
                    let id = identity(&file.metadata().map_err(io)?);
                    return Ok(Self {
                        directory: parent.try_clone().map_err(io)?,
                        name,
                        file,
                        id,
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(io(error)),
            }
        }
        Err(FsError::Io)
    }

    fn validate(&self) -> Result<()> {
        let file = self
            .directory
            .open_with(
                &self.name,
                &crate::platform_os::confined_file_options(false, false),
            )
            .map_err(|_| FsError::Changed)?;
        let metadata = file.metadata().map_err(io)?;
        regular(&metadata)?;
        if identity(&metadata) != self.id {
            return Err(FsError::Changed);
        }
        Ok(())
    }
}

impl Drop for Temporary {
    fn drop(&mut self) {
        // Clean only our reserved entry, never an arbitrary caller-named file.
        // A rename may already have published it, leaving nothing to remove.
        if let Ok(file) = self.directory.open_with(
            &self.name,
            &crate::platform_os::confined_file_options(false, false),
        ) {
            if let Ok(metadata) = file.metadata() {
                if metadata.is_file() && identity(&metadata) == self.id {
                    let _ = self.directory.remove_file(&self.name);
                }
            }
        }
    }
}

fn regular(metadata: &Metadata) -> Result<()> {
    if !metadata.is_file() {
        return Err(FsError::NotFile);
    }
    if metadata.nlink() != 1 {
        return Err(FsError::HardLink);
    }
    if metadata.len() > MAX_FILE_BYTES as u64 {
        return Err(FsError::FileTooLarge);
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum ReadLimit {
    Tool,
    Input(usize),
}
impl ReadLimit {
    fn maximum(self) -> usize {
        match self {
            Self::Tool => MAX_FILE_BYTES,
            Self::Input(bytes) => bytes,
        }
    }
    fn overflow(self) -> FsError {
        match self {
            Self::Tool => FsError::FileTooLarge,
            Self::Input(_) => FsError::InputTooLarge,
        }
    }
}
fn bounded_read(file: &mut File) -> Result<Vec<u8>> {
    bounded_read_with_limit(file, ReadLimit::Tool)
}
fn bounded_read_with_limit(file: &mut File, limit: ReadLimit) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    file.take((limit.maximum() + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(io)?;
    if bytes.len() > limit.maximum() {
        return Err(limit.overflow());
    }
    Ok(bytes)
}

/// Append at most the output bound plus one Unicode scalar, so the existing
/// clamp formatter supplies its exact truncation marker without large copies.
fn push_bounded(out: &mut String, text: &str) -> bool {
    let remaining = MAX_OUTPUT_BYTES.saturating_sub(out.len());
    if text.len() <= remaining {
        out.push_str(text);
        return false;
    }
    let mut end = remaining.min(text.len());
    while !text.is_char_boundary(end) {
        end += 1;
    }
    if end == remaining && end < text.len() {
        end += text[end..].chars().next().unwrap().len_utf8();
    }
    out.push_str(&text[..end]);
    true
}

fn render_page(text: &str, offset: usize, limit: usize) -> String {
    if text.is_empty() {
        return cat_n(text, offset, limit);
    }
    let mut out = String::new();
    for (index, line) in text.lines().skip(offset).take(limit).enumerate() {
        if index > 0 && push_bounded(&mut out, "\n") {
            return crate::tools::clamp(out);
        }
        if push_bounded(&mut out, &format!("{:>6}\t", offset + index + 1))
            || push_bounded(&mut out, line)
        {
            return crate::tools::clamp(out);
        }
    }
    out
}

fn checked_path(path: &str) -> Result<String> {
    if path.len() > MAX_PATH_CHARS * 4 || path.chars().count() > MAX_PATH_CHARS {
        return Err(FsError::InvalidPath);
    }
    Ok(path.to_owned())
}

pub(super) fn valid_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.contains([':', '\\'])
        || name.ends_with(['.', ' '])
        || name
            .chars()
            .any(|c| matches!(c, '\u{0}'..='\u{1f}' | '\u{7f}'..='\u{9f}'))
    {
        return Err(FsError::InvalidPath);
    }
    let lower = name.to_ascii_lowercase();
    let stem = lower.split('.').next().unwrap_or("");
    if matches!(stem, "con" | "prn" | "aux" | "nul" | "clock$")
        || (stem.len() == 4
            && (stem.starts_with("com") || stem.starts_with("lpt"))
            && matches!(stem.as_bytes()[3], b'1'..=b'9'))
    {
        return Err(FsError::InvalidPath);
    }
    Ok(())
}

/// Denies/config boundaries compare conservative NFC/lowercase keys. Actual
/// opens and exact protected-write allows keep the original path spelling;
/// normalization cannot grant a different file on a case-sensitive filesystem.
fn policy_key(value: &str) -> String {
    value
        .nfc()
        .flat_map(char::to_lowercase)
        .collect::<String>()
        .nfc()
        .collect()
}

fn within_policy(path: &str, prefix: &str) -> bool {
    let path = policy_key(path);
    let prefix = policy_key(prefix);
    path == prefix
        || path
            .strip_prefix(&prefix)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

fn protected_write(parts: &[String]) -> bool {
    parts.iter().any(|part| {
        matches!(
            part.to_ascii_lowercase().as_str(),
            ".github"
                | ".gitlab"
                | ".circleci"
                | ".superset"
                | ".devcontainer"
                | ".vscode"
                | ".gitlab-ci.yml"
                | "azure-pipelines.yml"
                | "jenkinsfile"
                | "build.rs"
                | "build.sh"
                | "build.ps1"
                | "install.sh"
                | "install.ps1"
                | "makefile"
                | "gnumakefile"
                | "cmakelists.txt"
                | "cargo.toml"
                | "package.json"
                | "pyproject.toml"
                | "setup.py"
                | "setup.cfg"
        )
    })
}

fn relative_scope(root: &Path, path: &Path) -> Option<String> {
    let roots = root.components().collect::<Vec<_>>();
    let paths = path.components().collect::<Vec<_>>();
    if roots.len() > paths.len() {
        return None;
    }
    if !roots.iter().zip(&paths).all(|(a, b)| {
        a.as_os_str()
            .to_str()
            .zip(b.as_os_str().to_str())
            .is_some_and(|(a, b)| policy_key(a) == policy_key(b))
    }) {
        return None;
    }
    paths[roots.len()..]
        .iter()
        .map(|p| p.as_os_str().to_str())
        .collect::<Option<Vec<_>>>()
        .map(|p| p.join("/"))
}

pub(super) fn resolve_config(config: &Path) -> Result<PathBuf> {
    // A nonexistent path containing '..' cannot be canonicalized coherently:
    // lexical normalization and traversal through a symlink have different
    // meanings. The trusted frontend must resolve it before constructing us.
    if config
        .components()
        .any(|part| matches!(part, Component::ParentDir))
    {
        return Err(FsError::InvalidScope);
    }
    let mut existing = config.to_path_buf();
    let mut tail = Vec::new();
    loop {
        match std::fs::symlink_metadata(&existing) {
            Ok(_) => {
                let mut resolved = existing.canonicalize().map_err(|_| FsError::InvalidScope)?;
                for name in tail.into_iter().rev() {
                    resolved.push(name);
                }
                return Ok(resolved);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                tail.push(
                    existing
                        .file_name()
                        .ok_or(FsError::InvalidScope)?
                        .to_os_string(),
                );
                if !existing.pop() {
                    return Err(FsError::InvalidScope);
                }
            }
            Err(_) => return Err(FsError::InvalidScope),
        }
    }
}

#[async_trait]
impl Backend for ConfinedFs {
    async fn read(
        &self,
        path: &str,
        offset: usize,
        limit: usize,
    ) -> std::result::Result<String, AgentError> {
        let path = checked_path(path).map_err(AgentError::from)?;
        self.run(move |fs| {
            let parts = fs.relative(&path, false)?;
            fs.authorize("read_file", &parts, false)?;
            let bytes = fs.read_bytes(&parts)?;
            let text = std::str::from_utf8(&bytes).map_err(|_| FsError::InvalidUtf8)?;
            if !text.is_empty() && offset >= text.lines().count() {
                return Err(FsError::Offset);
            }
            Ok(render_page(text, offset, limit))
        })
        .await
    }

    async fn write(&self, path: &str, content: &str) -> std::result::Result<(), AgentError> {
        let path = checked_path(path).map_err(AgentError::from)?;
        if content.len() > MAX_FILE_BYTES {
            return Err(FsError::FileTooLarge.into());
        }
        let content = content.to_owned();
        self.run(move |fs| {
            let parts = fs.relative(&path, false)?;
            fs.authorize("write_file", &parts, true)?;
            fs.write_file(&parts, content.as_bytes())
        })
        .await
    }

    async fn edit(
        &self,
        path: &str,
        old: &str,
        new: &str,
        replace_all: bool,
    ) -> std::result::Result<(), AgentError> {
        let path = checked_path(path).map_err(AgentError::from)?;
        if old.len() > MAX_FILE_BYTES || new.len() > MAX_FILE_BYTES {
            return Err(FsError::FileTooLarge.into());
        }
        let old = old.to_owned();
        let new = new.to_owned();
        self.run(move |fs| {
            let parts = fs.relative(&path, false)?;
            fs.authorize("edit_file", &parts, true)?;
            fs.edit_file(&parts, &old, &new, replace_all)
        })
        .await
    }

    async fn ls(&self, path: &str) -> std::result::Result<Vec<FileInfo>, AgentError> {
        let path = checked_path(path).map_err(AgentError::from)?;
        self.run(move |fs| {
            let parts = fs.relative(&path, true)?;
            fs.authorize("ls", &parts, false)?;
            fs.list(&parts)
        })
        .await
    }

    async fn glob(&self, pattern: &str) -> std::result::Result<Vec<String>, AgentError> {
        if pattern.len() > MAX_OUTPUT_BYTES {
            return Err(FsError::OutputLimit.into());
        }
        let pattern = pattern.to_owned();
        self.run(move |fs| {
            // Bound both the DP matcher and the returned names. Globs are always
            // root-relative; literal directories still go through the same walk.
            if pattern.len() > 256
                || pattern.starts_with('/')
                || pattern.contains(['\\', ':'])
                || pattern.split('/').any(|p| p == "..")
                || pattern.chars().any(char::is_control)
            {
                return Err(FsError::InvalidPath);
            }
            fs.authorize("glob", &[], false)?;
            let mut bytes = 0usize;
            let mut work = 0usize;
            let mut matches = Vec::new();
            for parts in fs.walk_files(&[])? {
                let path = parts.join("/");
                work = work
                    .saturating_add(pattern.chars().count().saturating_mul(path.chars().count()));
                if work > MAX_MATCH_WORK {
                    return Err(FsError::OutputLimit);
                }
                if glob_match(&pattern, &path) {
                    bytes = bytes.saturating_add(path.len());
                    if bytes > MAX_OUTPUT_BYTES {
                        return Err(FsError::OutputLimit);
                    }
                    matches.push(path);
                }
            }
            Ok(matches)
        })
        .await
    }

    async fn grep(
        &self,
        pattern: &str,
        path: Option<&str>,
    ) -> std::result::Result<String, AgentError> {
        if pattern.len() > MAX_OUTPUT_BYTES {
            return Err(FsError::OutputLimit.into());
        }
        let pattern = pattern.to_owned();
        let path = checked_path(path.unwrap_or("")).map_err(AgentError::from)?;
        self.run(move |fs| {
            if pattern.len() > MAX_OUTPUT_BYTES {
                return Err(FsError::OutputLimit);
            }
            let parts = fs.relative(&path, true)?;
            fs.authorize("grep", &parts, false)?;
            let files = if !parts.is_empty() {
                let parent = fs.parent(&parts, false)?;
                let metadata = parent
                    .directory
                    .symlink_metadata(parts.last().unwrap())
                    .map_err(io)?;
                if metadata.is_file() && !metadata.file_type().is_symlink() {
                    vec![parts.clone()]
                } else {
                    fs.walk_files(&parts)?
                }
            } else {
                fs.walk_files(&parts)?
            };
            let mut out = String::new();
            let mut searched = 0usize;
            let direct = files.len() == 1 && files[0] == parts && !parts.is_empty();
            for file in files {
                let bytes = match fs.read_bytes(&file) {
                    Ok(bytes) => bytes,
                    Err(error) if direct => return Err(error),
                    Err(_) => continue,
                };
                searched = searched.saturating_add(bytes.len());
                if searched > MAX_SEARCH_BYTES {
                    return Err(FsError::OutputLimit);
                }
                if bytes.iter().take(8192).any(|b| *b == 0) {
                    continue;
                }
                let Ok(text) = std::str::from_utf8(&bytes) else {
                    continue;
                };
                for (index, line) in text.lines().enumerate() {
                    if line.contains(&pattern) {
                        let path = file.join("/");
                        // Use the established bounded text formatter, including
                        // its explicit truncation marker, not a silent prefix.
                        let prefix = format!("{path}:{}:", index + 1);
                        if push_bounded(&mut out, &prefix)
                            || push_bounded(&mut out, line)
                            || push_bounded(&mut out, "\n")
                        {
                            return Ok(crate::tools::clamp(out));
                        }
                    }
                }
            }
            if out.is_empty() {
                Ok(crate::tools::clamp(format!(
                    "No matches found for '{pattern}'"
                )))
            } else {
                Ok(out)
            }
        })
        .await
    }
}

#[cfg(test)]
mod tests;

/// Pure stricter private publication deny check; no backend allow or IO facet.
/// Source spellings remain original; these normalized keys can ONLY deny.
pub(super) fn publication_names_safe(parts: &[String]) -> bool {
    let keys: Vec<String> = parts.iter().map(|p| policy_key(p)).collect();
    !protected_write(&keys)
        && keys.iter().all(|name| {
            valid_name(name).is_ok()
                && name != "."
                && name != ".."
                && name != ".git"
                && name != ".craftsman"
                && !name.starts_with(".env")
                && !name.starts_with(TEMP_PREFIX)
        })
}
