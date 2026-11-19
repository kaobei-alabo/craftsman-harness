//! Read-only retained native facet. No Backend/Prepared policy, writer, delete,
//! directory accessor or Deref is exported. Existing confined IO is unchanged.

use super::{
    CaptureError, MAX_FILE_BYTES, MAX_PATH_BYTES, MAX_TARGETS, MAX_TOTAL_BYTES, Result, hex,
};
use crate::agent::FrontendCaptureContext;
use crate::hitl::ApprovalState;
use base64::{Engine, engine::general_purpose::STANDARD};
use cap_fs_ext::DirExt;
use cap_std::fs::{Dir, File, Metadata};
use craftsman_harness::TurnCancellation;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use unicode_normalization::UnicodeNormalization;

const MAX_DEPTH: usize = 16;
const MAX_ABSOLUTE_DEPTH: usize = 128;
const MAX_NATIVE_CALLS: usize = 16_384;
const MAX_READ_CALLS: usize = 128;

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Stage {
    Admitted,
    BeforeRead,
    BeforeEof,
    AfterRead,
}
#[cfg(test)]
pub(super) type Hook = std::sync::Arc<dyn Fn(Stage, &str) + Send + Sync>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Identity {
    volume: u64,
    file: u128,
}
impl Identity {
    fn json(self) -> Value {
        // Full 128-bit identity remains exact, including Windows ReFS fields.
        json!({"volume":self.volume,"file":format!("{:032x}", self.file)})
    }
}

struct Budget(usize);
impl Budget {
    fn charge(&mut self) -> Result<()> {
        self.0 = self.0.checked_sub(1).ok_or(CaptureError::Bounds)?;
        Ok(())
    }
}

pub(super) struct SelectedTargets(Vec<TargetName>);
struct TargetName {
    spelling: String,
    parts: Vec<String>,
    key: String,
}

pub(super) fn denial_key(name: &str) -> String {
    name.nfc()
        .flat_map(char::to_lowercase)
        .collect::<String>()
        .nfc()
        .collect()
}

pub(super) fn checked_name(raw: &str) -> Result<()> {
    if raw.is_empty() || raw.len() > 4096 || raw.starts_with('/') || raw.contains('\\') {
        return Err(CaptureError::Target);
    }
    let mut count = 0;
    for part in raw.split('/') {
        count += 1;
        if part == "."
            || part == ".."
            || part.is_empty()
            || part.len() > 255
            || part.contains(['<', '>', '"', '|', '*', '?'])
        {
            return Err(CaptureError::Target);
        }
        // Reuse the unchanged confined native basename/device grammar only.
        // Its invocation/root/Prepared policy is never borrowed.
        crate::exec::confined_fs::valid_name(part).map_err(|_| CaptureError::Target)?;
        let stem = part
            .split('.')
            .next()
            .unwrap_or("")
            .trim_end_matches(' ')
            .nfkc()
            .flat_map(char::to_lowercase)
            .collect::<String>();
        if matches!(stem.as_str(), "con" | "prn" | "aux" | "nul" | "clock$")
            || ((stem.starts_with("com") || stem.starts_with("lpt"))
                && stem.len() == 4
                && stem.as_bytes()[3].is_ascii_digit())
        {
            return Err(CaptureError::Target);
        }
        if part.chars().any(|c| {
            matches!(c,
            '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{2028}'..='\u{202e}'
            | '\u{2066}'..='\u{2069}' | '\u{feff}')
        }) {
            return Err(CaptureError::Target);
        }
    }
    if count > MAX_DEPTH {
        return Err(CaptureError::Target);
    }
    Ok(())
}

impl SelectedTargets {
    pub(super) fn admit(raw: &[&str]) -> Result<Self> {
        if raw.len() > MAX_TARGETS {
            return Err(CaptureError::Bounds);
        }
        let mut bytes = 0usize;
        let mut keys = std::collections::BTreeSet::new();
        // Whole name/type admission precedes copying owned selector strings.
        for name in raw {
            bytes = bytes.checked_add(name.len()).ok_or(CaptureError::Bounds)?;
            if bytes > MAX_PATH_BYTES {
                return Err(CaptureError::Bounds);
            }
            checked_name(name)?;
            if !keys.insert(denial_key(name)) {
                return Err(CaptureError::Target);
            }
        }
        Ok(Self(
            raw.iter()
                .map(|name| TargetName {
                    spelling: (*name).to_owned(),
                    parts: name.split('/').map(str::to_owned).collect(),
                    key: denial_key(name),
                })
                .collect(),
        ))
    }
}

struct AbsoluteDirectory {
    path: PathBuf,
    anchor: Dir,
    names: Vec<String>,
    ids: Vec<Identity>,
    directory: Dir,
}

impl AbsoluteDirectory {
    fn capture(path: &Path, budget: &mut Budget) -> Result<Self> {
        if !path.is_absolute() || path.as_os_str().len() > 16 * 1024 {
            return Err(CaptureError::Scope);
        }
        let mut anchor_path = PathBuf::new();
        let mut names = Vec::new();
        for component in path.components() {
            match component {
                Component::Prefix(prefix) if names.is_empty() => {
                    #[cfg(windows)]
                    if !matches!(
                        prefix.kind(),
                        std::path::Prefix::Disk(_) | std::path::Prefix::VerbatimDisk(_)
                    ) {
                        return Err(CaptureError::Scope);
                    }
                    anchor_path.push(prefix.as_os_str());
                }
                Component::RootDir if names.is_empty() => anchor_path.push(component.as_os_str()),
                Component::Normal(name) => {
                    let name = name.to_str().ok_or(CaptureError::Scope)?;
                    if name.len() > 255 {
                        return Err(CaptureError::Scope);
                    }
                    crate::exec::confined_fs::valid_name(name).map_err(|_| CaptureError::Scope)?;
                    names.push(name.to_owned());
                }
                _ => return Err(CaptureError::Scope),
            }
        }
        // A selected OS root is not a session folder.
        if names.is_empty() || names.len() > MAX_ABSOLUTE_DEPTH {
            return Err(CaptureError::Scope);
        }
        budget.charge()?;
        let anchor = Dir::open_ambient_dir(&anchor_path, cap_std::ambient_authority())
            .map_err(|_| CaptureError::Scope)?;
        let (directory, ids) = walk(&anchor, &names, budget)?;
        let captured = Self {
            path: path.to_path_buf(),
            anchor,
            names,
            ids,
            directory,
        };
        captured.validate(budget)?;
        Ok(captured)
    }

    fn validate(&self, budget: &mut Budget) -> Result<()> {
        let (_, ids) = walk(&self.anchor, &self.names, budget).map_err(stale)?;
        if ids != self.ids {
            return Err(CaptureError::Changed);
        }
        Ok(())
    }

    fn id(&self) -> Identity {
        *self
            .ids
            .last()
            .expect("a captured directory has a nonempty walk")
    }
}

fn walk(start: &Dir, names: &[String], budget: &mut Budget) -> Result<(Dir, Vec<Identity>)> {
    budget.charge()?;
    let mut directory = start.try_clone().map_err(|_| CaptureError::Io)?;
    let mut ids = Vec::with_capacity(names.len());
    for name in names {
        budget.charge()?;
        let before = directory
            .symlink_metadata(name)
            .map_err(|_| CaptureError::Parent)?;
        if before.file_type().is_symlink() || !before.is_dir() {
            return Err(CaptureError::Unsupported);
        }
        budget.charge()?;
        directory = directory
            .open_dir_nofollow(name)
            .map_err(|_| CaptureError::Parent)?;
        budget.charge()?;
        let metadata = directory.dir_metadata().map_err(|_| CaptureError::Io)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(CaptureError::Unsupported);
        }
        ids.push(directory_identity(&directory, &metadata)?);
    }
    Ok((directory, ids))
}

struct Parent {
    directory: Dir,
    names: Vec<String>,
    ids: Vec<Identity>,
    id: Identity,
}

/// The sole root is captured from actual normal construction, never suggestions.
pub(super) struct ReadRoot {
    root: AbsoluteDirectory,
    config: AbsoluteDirectory,
    config_key: Option<String>,
    runtime: Identity,
    runtime_path: PathBuf,
}

impl ReadRoot {
    pub(super) fn capture_actual(
        actual: FrontendCaptureContext,
        stop: Option<&TurnCancellation>,
    ) -> Result<(
        Self,
        std::sync::Arc<std::sync::Mutex<ApprovalState>>,
        std::sync::Arc<()>,
    )> {
        let (root, config, approval, frontend) = actual.into_parts();
        let root = Self::capture_paths(&root, &config, stop)?;
        Ok((root, approval, frontend))
    }

    fn capture_paths(root: &Path, config: &Path, stop: Option<&TurnCancellation>) -> Result<Self> {
        if stop.is_some_and(TurnCancellation::is_cancelled) {
            return Err(CaptureError::Revoked);
        }
        let mut budget = Budget(MAX_NATIVE_CALLS);
        let root = AbsoluteDirectory::capture(root, &mut budget)?;
        // First subset requires the actual configuration folder to exist.
        // Missing/inaccessible configuration is not a synthetic deny identity.
        let config = AbsoluteDirectory::capture(config, &mut budget)?;
        if root.ids.contains(&config.id()) || root.id() == config.id() {
            return Err(CaptureError::Denied);
        }
        let config_key = relative_key(&root.path, &config.path);
        let runtime_path = std::env::current_exe().map_err(|_| CaptureError::Scope)?;
        let runtime_parent = runtime_path.parent().ok_or(CaptureError::Scope)?;
        let runtime_name = runtime_path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or(CaptureError::Scope)?;
        let parent = AbsoluteDirectory::capture(runtime_parent, &mut budget)?;
        let runtime_file = open_read(&parent.directory, runtime_name, &mut budget)?;
        let metadata = runtime_file.metadata().map_err(|_| CaptureError::Io)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(CaptureError::Scope);
        }
        let runtime = file_identity(&runtime_file, &metadata)?;
        root.validate(&mut budget)?;
        config.validate(&mut budget)?;
        if stop.is_some_and(TurnCancellation::is_cancelled) {
            return Err(CaptureError::Revoked);
        }
        Ok(Self {
            root,
            config,
            config_key,
            runtime,
            runtime_path,
        })
    }

    pub(super) fn binding(&self) -> Value {
        json!({
            "root":self.root.path.to_str(),
            "root_identity":self.root.id().json(),
            "root_ancestors":self.root.ids.iter().map(|id| id.json()).collect::<Vec<_>>(),
            "config_origin":self.config.path.to_str(),
            "config_identity":self.config.id().json(),
            "runtime":self.runtime_path.to_str(),
            "runtime_identity":self.runtime.json(),
            "policy":"normal-session-read-only-v1",
        })
    }

    pub(super) fn check_names(&self, targets: &SelectedTargets) -> Result<()> {
        for target in &targets.0 {
            if target.parts.iter().any(|part| sensitive(part))
                || self
                    .config_key
                    .as_ref()
                    .is_some_and(|prefix| within(&target.key, prefix))
                || relative_key(&self.root.path, &self.runtime_path)
                    .as_ref()
                    .is_some_and(|runtime| *runtime == target.key)
            {
                return Err(CaptureError::Denied);
            }
        }
        Ok(())
    }

    fn parent(&self, target: &TargetName, budget: &mut Budget) -> Result<Parent> {
        let names = &target.parts[..target.parts.len() - 1];
        let (directory, ids) = walk(&self.root.directory, names, budget)?;
        if ids.contains(&self.config.id()) {
            return Err(CaptureError::Denied);
        }
        budget.charge()?;
        let id = directory_identity(
            &directory,
            &directory.dir_metadata().map_err(|_| CaptureError::Io)?,
        )?;
        if id == self.config.id() {
            return Err(CaptureError::Denied);
        }
        Ok(Parent {
            directory,
            names: names.to_vec(),
            ids,
            id,
        })
    }

    fn validate_parent(&self, parent: &Parent, budget: &mut Budget) -> Result<()> {
        let (directory, ids) = walk(&self.root.directory, &parent.names, budget).map_err(stale)?;
        budget.charge()?;
        let id = directory_identity(
            &directory,
            &directory.dir_metadata().map_err(|_| CaptureError::Io)?,
        )?;
        if ids != parent.ids || id != parent.id {
            return Err(CaptureError::Changed);
        }
        Ok(())
    }

    pub(super) fn capture(
        &self,
        targets: SelectedTargets,
        check: impl Fn() -> Result<()>,
        #[cfg(test)] hook: Option<&Hook>,
    ) -> Result<Vec<TargetSnapshot>> {
        self.check_names(&targets)?;
        let mut budget = Budget(MAX_NATIVE_CALLS);
        check()?;
        self.root.validate(&mut budget)?;
        self.config.validate(&mut budget)?;
        let mut admitted = Vec::with_capacity(targets.0.len());
        let mut total = 0usize;
        let mut files = Vec::new();
        let mut slots = Vec::new();
        // Open/check the ENTIRE set before owning any original file bytes.
        for target in targets.0 {
            check()?;
            let parent = self.parent(&target, &mut budget)?;
            self.validate_parent(&parent, &mut budget)?;
            let name = target.parts.last().expect("admitted nonempty target");
            if slots
                .iter()
                .any(|(id, key)| *id == parent.id && *key == denial_key(name))
            {
                return Err(CaptureError::Target);
            }
            slots.push((parent.id, denial_key(name)));
            budget.charge()?;
            let path = parent.directory.symlink_metadata(name);
            let before = match path {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    self.validate_parent(&parent, &mut budget)?;
                    budget.charge()?;
                    match parent.directory.symlink_metadata(name) {
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                            Before::Absent
                        }
                        _ => return Err(CaptureError::Changed),
                    }
                }
                Err(_) => return Err(CaptureError::Io),
                Ok(path_before) => {
                    regular_path(&path_before)?;
                    let file = open_read(&parent.directory, name, &mut budget)?;
                    budget.charge()?;
                    let handle_before = HandleFacts::file(&file)?;
                    handle_before.regular()?;
                    compatible(&path_before, &handle_before.metadata)?;
                    if handle_before.id == self.runtime {
                        return Err(CaptureError::Denied);
                    }
                    if files.contains(&handle_before.id) {
                        return Err(CaptureError::Target);
                    }
                    files.push(handle_before.id);
                    let size = usize::try_from(handle_before.metadata.len())
                        .map_err(|_| CaptureError::Bounds)?;
                    total = total.checked_add(size).ok_or(CaptureError::Bounds)?;
                    if size > MAX_FILE_BYTES || total > MAX_TOTAL_BYTES {
                        return Err(CaptureError::Bounds);
                    }
                    Before::Present {
                        file,
                        path_before,
                        handle_before,
                    }
                }
            };
            admitted.push(Admitted {
                target,
                parent,
                before,
            });
        }
        #[cfg(test)]
        if let Some(hook) = hook {
            hook(Stage::Admitted, "");
        }
        check()?;
        let mut snapshots = Vec::with_capacity(admitted.len());
        for mut admitted in admitted {
            check()?;
            self.validate_parent(&admitted.parent, &mut budget)?;
            let name = admitted.target.parts.last().expect("admitted target");
            let (bytes, evidence) = match &mut admitted.before {
                Before::Absent => {
                    budget.charge()?;
                    match admitted.parent.directory.symlink_metadata(name) {
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                        _ => return Err(CaptureError::Changed),
                    }
                    (None, None)
                }
                Before::Present {
                    file,
                    path_before,
                    handle_before,
                } => {
                    #[cfg(test)]
                    if let Some(hook) = hook {
                        hook(Stage::BeforeRead, &admitted.target.spelling);
                    }
                    check()?;
                    let size = handle_before.metadata.len() as usize;
                    let mut bytes = Vec::with_capacity(size);
                    let mut scratch = [0u8; 8192];
                    let mut calls = MAX_READ_CALLS;
                    while bytes.len() < size {
                        check()?;
                        calls = calls.checked_sub(1).ok_or(CaptureError::Bounds)?;
                        budget.charge()?;
                        let remaining = (size - bytes.len()).min(scratch.len());
                        match file.read(&mut scratch[..remaining]) {
                            Ok(0) => return Err(CaptureError::Changed),
                            Ok(count) => bytes.extend_from_slice(&scratch[..count]),
                            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                            Err(_) => return Err(CaptureError::Io),
                        }
                    }
                    #[cfg(test)]
                    if let Some(hook) = hook {
                        hook(Stage::BeforeEof, &admitted.target.spelling);
                    }
                    loop {
                        check()?;
                        calls = calls.checked_sub(1).ok_or(CaptureError::Bounds)?;
                        budget.charge()?;
                        match file.read(&mut scratch[..1]) {
                            Ok(0) => break,
                            Ok(_) => return Err(CaptureError::Changed),
                            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                            Err(_) => return Err(CaptureError::Io),
                        }
                    }
                    #[cfg(test)]
                    if let Some(hook) = hook {
                        hook(Stage::AfterRead, &admitted.target.spelling);
                    }
                    check()?;
                    budget.charge()?;
                    let handle_after = HandleFacts::file(file)?;
                    handle_after.regular()?;
                    handle_before.stable(&handle_after)?;
                    budget.charge()?;
                    let path_after = admitted
                        .parent
                        .directory
                        .symlink_metadata(name)
                        .map_err(|_| CaptureError::Changed)?;
                    regular_path(&path_after)?;
                    stable_metadata(path_before, &path_after)?;
                    let named =
                        open_read(&admitted.parent.directory, name, &mut budget).map_err(stale)?;
                    budget.charge()?;
                    let named_after = HandleFacts::file(&named)?;
                    named_after.regular()?;
                    handle_after.stable(&named_after)?;
                    compatible(&path_after, &named_after.metadata)?;
                    (
                        Some(bytes),
                        Some(Evidence {
                            path_before: path_before.clone(),
                            path_after,
                            handle_before: handle_before.clone(),
                            handle_after,
                            named_after,
                            path_final: None,
                            named_final: None,
                        }),
                    )
                }
            };
            self.validate_parent(&admitted.parent, &mut budget)?;
            check()?;
            snapshots.push(TargetSnapshot {
                name: admitted.target.spelling,
                parent: admitted.parent,
                before: admitted.before,
                bytes,
                evidence,
            });
        }
        self.root.validate(&mut budget)?;
        self.config.validate(&mut budget)?;
        check()?;
        // Recheck every retained target after the entire batch, including
        // exact missing leaves. This is still historical, never atomic/CAS.
        for snapshot in &mut snapshots {
            self.validate_parent(&snapshot.parent, &mut budget)?;
            let name = snapshot.name.rsplit('/').next().expect("target leaf");
            if let Some(evidence) = &mut snapshot.evidence {
                budget.charge()?;
                let path_final = snapshot
                    .parent
                    .directory
                    .symlink_metadata(name)
                    .map_err(|_| CaptureError::Changed)?;
                stable_metadata(&evidence.path_after, &path_final)?;
                let named =
                    open_read(&snapshot.parent.directory, name, &mut budget).map_err(stale)?;
                let final_handle = HandleFacts::file(&named)?;
                evidence.named_after.stable(&final_handle)?;
                evidence.path_final = Some(path_final);
                evidence.named_final = Some(final_handle);
            } else {
                budget.charge()?;
                match snapshot.parent.directory.symlink_metadata(name) {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    _ => return Err(CaptureError::Changed),
                }
            }
            check()?;
        }
        self.root.validate(&mut budget)?;
        self.config.validate(&mut budget)?;
        Ok(snapshots)
    }
}

enum Before {
    Absent,
    Present {
        file: File,
        path_before: Metadata,
        handle_before: HandleFacts,
    },
}
struct Admitted {
    target: TargetName,
    parent: Parent,
    before: Before,
}
struct Evidence {
    path_before: Metadata,
    path_after: Metadata,
    handle_before: HandleFacts,
    handle_after: HandleFacts,
    named_after: HandleFacts,
    path_final: Option<Metadata>,
    named_final: Option<HandleFacts>,
}

pub(super) struct TargetSnapshot {
    name: String,
    parent: Parent,
    before: Before,
    bytes: Option<Vec<u8>>,
    evidence: Option<Evidence>,
}
impl TargetSnapshot {
    pub(super) fn name(&self) -> &str {
        &self.name
    }
    pub(super) fn bytes(&self) -> Option<&[u8]> {
        self.bytes.as_deref()
    }
    pub(super) fn binding(&self) -> Value {
        let before = match (&self.before, &self.bytes, &self.evidence) {
            (Before::Absent, None, None) => json!({"kind":"absent","exact_missing_leaf":true}),
            (Before::Present { file, .. }, Some(bytes), Some(evidence)) => {
                // Keep the retained read handle private and alive. No reopened
                // ambient path or guest before hash can replace this original.
                let _retained = file;
                json!({
                    "kind":"file","bytes_base64":STANDARD.encode(bytes),
                    "size":bytes.len(),"sha256":hex(&Sha256::digest(bytes)),
                    "path_before":metadata_json(&evidence.path_before),
                    "path_after":metadata_json(&evidence.path_after),
                    "handle_before":evidence.handle_before.json(),
                    "handle_after":evidence.handle_after.json(),
                    "named_after":evidence.named_after.json(),
                    "path_final":metadata_json(evidence.path_final.as_ref().expect("complete final path evidence")),
                    "named_final":evidence.named_final.as_ref().expect("complete final handle evidence").json(),
                    "whole_bytes_and_eof":true,
                })
            }
            _ => unreachable!("only completely admitted native originals are constructed"),
        };
        json!({
            "target":self.name,
            "parent_identity":self.parent.id.json(),
            "parent_ancestors":self.parent.ids.iter().map(|id| id.json()).collect::<Vec<_>>(),
            "before":before,
        })
    }
}

fn sensitive(part: &str) -> bool {
    let part = denial_key(part);
    part.starts_with(".env")
        || part.starts_with(".craftsman-exec-tmp-")
        || matches!(
            part.as_str(),
            ".craftsman"
                | ".git"
                | ".ssh"
                | ".aws"
                | ".azure"
                | ".gcloud"
                | ".gnupg"
                | ".npmrc"
                | ".pypirc"
                | ".netrc"
                | ".git-credentials"
                | ".docker"
                | ".kube"
                | ".authinfo"
                | "credentials"
                | "credentials.json"
                | "secrets.json"
                | "id_rsa"
                | "id_ed25519"
                | "target"
                | "node_modules"
                | ".venv"
                | "__pycache__"
        )
}
fn within(path: &str, prefix: &str) -> bool {
    path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|tail| tail.starts_with('/'))
}
fn relative_key(root: &Path, path: &Path) -> Option<String> {
    let root = denial_key(root.to_str()?).replace('\\', "/");
    let path = denial_key(path.to_str()?).replace('\\', "/");
    path.strip_prefix(&root)
        .and_then(|tail| tail.strip_prefix('/'))
        .map(str::to_owned)
}
fn open_read(parent: &Dir, name: &str, budget: &mut Budget) -> Result<File> {
    budget.charge()?;
    parent
        .open_with(
            name,
            &crate::platform_os::confined_file_options(false, false),
        )
        .map_err(|_| CaptureError::Io)
}
fn stale(error: CaptureError) -> CaptureError {
    match error {
        CaptureError::Parent | CaptureError::Unsupported | CaptureError::Io => {
            CaptureError::Changed
        }
        other => other,
    }
}
fn regular_path(metadata: &Metadata) -> Result<()> {
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(CaptureError::Unsupported);
    }
    #[cfg(unix)]
    if cap_std::fs::MetadataExt::nlink(metadata) != 1 {
        return Err(CaptureError::Unsupported);
    }
    Ok(())
}

#[derive(Clone)]
struct HandleFacts {
    metadata: Metadata,
    id: Identity,
    links: u64,
    legacy_dev: u64,
    legacy_ino: u64,
    #[cfg(windows)]
    change_time: i64,
}
impl HandleFacts {
    fn file(file: &File) -> Result<Self> {
        let metadata = file.metadata().map_err(|_| CaptureError::Io)?;
        let id = file_identity(file, &metadata)?;
        let links = cap_fs_ext::MetadataExt::nlink(&metadata);
        let legacy_dev = cap_fs_ext::MetadataExt::dev(&metadata);
        let legacy_ino = cap_fs_ext::MetadataExt::ino(&metadata);
        #[cfg(windows)]
        let change_time =
            windows_change_time(std::os::windows::io::AsRawHandle::as_raw_handle(file))?;
        Ok(Self {
            metadata,
            id,
            links,
            legacy_dev,
            legacy_ino,
            #[cfg(windows)]
            change_time,
        })
    }
    fn regular(&self) -> Result<()> {
        regular_path(&self.metadata)?;
        if self.links != 1 {
            return Err(CaptureError::Unsupported);
        }
        Ok(())
    }
    fn stable(&self, after: &Self) -> Result<()> {
        if self.id != after.id
            || self.links != after.links
            || self.legacy_dev != after.legacy_dev
            || self.legacy_ino != after.legacy_ino
        {
            return Err(CaptureError::Changed);
        }
        #[cfg(windows)]
        if self.change_time != after.change_time {
            return Err(CaptureError::Changed);
        }
        stable_metadata(&self.metadata, &after.metadata)
    }
    fn json(&self) -> Value {
        let mut value = metadata_json(&self.metadata);
        value["identity"] = self.id.json();
        value["nlink"] = json!(self.links);
        value["native_dev"] = json!(self.legacy_dev);
        value["native_ino"] = json!(self.legacy_ino);
        #[cfg(windows)]
        {
            value["change_time_100ns"] = json!(self.change_time);
        }
        value
    }
}

fn compatible(path: &Metadata, handle: &Metadata) -> Result<()> {
    if path.is_file() != handle.is_file()
        || path.file_type().is_symlink()
        || path.len() != handle.len()
        || path.modified().ok() != handle.modified().ok()
        || path.permissions().readonly() != handle.permissions().readonly()
    {
        return Err(CaptureError::Changed);
    }
    #[cfg(unix)]
    {
        use cap_std::fs::MetadataExt;
        if path.dev() != handle.dev()
            || path.ino() != handle.ino()
            || path.nlink() != handle.nlink()
            || path.mode() != handle.mode()
            || path.uid() != handle.uid()
            || path.gid() != handle.gid()
            || path.rdev() != handle.rdev()
            || path.size() != handle.size()
            || path.mtime() != handle.mtime()
            || path.mtime_nsec() != handle.mtime_nsec()
            || path.ctime() != handle.ctime()
            || path.ctime_nsec() != handle.ctime_nsec()
            || path.blksize() != handle.blksize()
            || path.blocks() != handle.blocks()
        {
            return Err(CaptureError::Changed);
        }
    }
    #[cfg(windows)]
    {
        use cap_std::fs::MetadataExt;
        // Path creation time and native handle ChangeTime are distinct fields.
        if path.file_attributes() != handle.file_attributes()
            || path.creation_time() != handle.creation_time()
            || path.last_write_time() != handle.last_write_time()
            || path.file_size() != handle.file_size()
        {
            return Err(CaptureError::Changed);
        }
    }
    Ok(())
}
fn stable_metadata(before: &Metadata, after: &Metadata) -> Result<()> {
    compatible(before, after)?;
    if before.created().ok() != after.created().ok() {
        return Err(CaptureError::Changed);
    }
    // Atime is retained exactly in both records. Our own read may change it;
    // it is not a content/type/identity stability prerequisite.
    Ok(())
}
fn time_json(time: std::io::Result<cap_std::time::SystemTime>) -> Value {
    match time {
        Ok(time) => {
            let time = time.into_std();
            let (before_epoch, duration) = match time.duration_since(std::time::UNIX_EPOCH) {
                Ok(duration) => (false, duration),
                Err(error) => (true, error.duration()),
            };
            json!({"before_epoch":before_epoch,"seconds":duration.as_secs(),"nanoseconds":duration.subsec_nanos()})
        }
        Err(error) => json!({"unavailable":format!("{:?}",error.kind())}),
    }
}
fn metadata_json(metadata: &Metadata) -> Value {
    let mut value = json!({
        "regular":metadata.is_file(),"directory":metadata.is_dir(),
        "symlink":metadata.file_type().is_symlink(),
        "size":metadata.len(),"readonly":metadata.permissions().readonly(),
        "modified":time_json(metadata.modified()),
        "accessed":time_json(metadata.accessed()),
        "created":time_json(metadata.created()),
    });
    #[cfg(unix)]
    {
        use cap_std::fs::MetadataExt;
        value["unix"] = json!({
            "dev":metadata.dev(),"ino":metadata.ino(),"mode":metadata.mode(),
            "nlink":metadata.nlink(),"uid":metadata.uid(),"gid":metadata.gid(),
            "rdev":metadata.rdev(),"size":metadata.size(),
            "atime":metadata.atime(),"atime_nsec":metadata.atime_nsec(),
            "mtime":metadata.mtime(),"mtime_nsec":metadata.mtime_nsec(),
            "ctime":metadata.ctime(),"ctime_nsec":metadata.ctime_nsec(),
            "blksize":metadata.blksize(),"blocks":metadata.blocks(),
        });
    }
    #[cfg(windows)]
    {
        use cap_std::fs::MetadataExt;
        value["windows"] = json!({
            "file_attributes":metadata.file_attributes(),
            "creation_time_100ns":metadata.creation_time(),
            "access_time_100ns":metadata.last_access_time(),
            "write_time_100ns":metadata.last_write_time(),
            "file_size":metadata.file_size(),
        });
        // These fields are unavailable in the raw path-query shape. Actual
        // by-handle observations below fill them independently; they are never
        // fabricated zeroes or compared to CreationTime as if it were ChangeTime.
        value["native_dev"] = Value::Null;
        value["native_ino"] = Value::Null;
        value["nlink"] = Value::Null;
        value["change_time_100ns"] = Value::Null;
    }
    value
}

#[cfg(unix)]
fn file_identity(_file: &File, metadata: &Metadata) -> Result<Identity> {
    use cap_std::fs::MetadataExt;
    Ok(Identity {
        volume: metadata.dev(),
        file: metadata.ino() as u128,
    })
}
#[cfg(unix)]
fn directory_identity(_directory: &Dir, metadata: &Metadata) -> Result<Identity> {
    use cap_std::fs::MetadataExt;
    Ok(Identity {
        volume: metadata.dev(),
        file: metadata.ino() as u128,
    })
}
#[cfg(windows)]
fn file_identity(file: &File, _metadata: &Metadata) -> Result<Identity> {
    use std::os::windows::io::AsRawHandle;
    windows_identity(file.as_raw_handle())
}
#[cfg(windows)]
fn directory_identity(directory: &Dir, _metadata: &Metadata) -> Result<Identity> {
    use std::os::windows::io::AsRawHandle;
    windows_identity(directory.as_raw_handle())
}
#[cfg(windows)]
fn windows_identity(handle: std::os::windows::io::RawHandle) -> Result<Identity> {
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ID_INFO, FileIdInfo, GetFileInformationByHandleEx,
    };
    let mut info = std::mem::MaybeUninit::<FILE_ID_INFO>::uninit();
    // Query only the already-owned handle; never reopen or upgrade access.
    let success = unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileIdInfo,
            info.as_mut_ptr().cast(),
            std::mem::size_of::<FILE_ID_INFO>() as u32,
        )
    };
    if success == 0 {
        return Err(CaptureError::Unsupported);
    }
    let info = unsafe { info.assume_init() };
    Ok(Identity {
        volume: info.VolumeSerialNumber,
        file: u128::from_le_bytes(info.FileId.Identifier),
    })
}
#[cfg(windows)]
fn windows_change_time(handle: std::os::windows::io::RawHandle) -> Result<i64> {
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_BASIC_INFO, FileBasicInfo, GetFileInformationByHandleEx,
    };
    let mut info = std::mem::MaybeUninit::<FILE_BASIC_INFO>::uninit();
    let success = unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileBasicInfo,
            info.as_mut_ptr().cast(),
            std::mem::size_of::<FILE_BASIC_INFO>() as u32,
        )
    };
    if success == 0 {
        return Err(CaptureError::Unsupported);
    }
    Ok(unsafe { info.assume_init() }.ChangeTime)
}
