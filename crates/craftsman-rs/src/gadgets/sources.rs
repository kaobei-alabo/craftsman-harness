//! Private source-bound headless approvals, NOT a GadgetRuntime/HostState.
//!
//! No ambient reopen, grant/cache/SDK write, compilation, instantiation or guest
//! export occurs here. An approved snapshot owns the exact manifest/component
//! bytes; a future host must consume that snapshot rather than reopen its label.
//! Source approval never grants a filesystem, HTTP or native tool operation.

use super::manifest::{Capabilities, MAX_COMPONENT_BYTES, MAX_MANIFEST_BYTES, Manifest, valid_id};
use super::{ErrorKind, GadgetError};
use crate::exec::confined_fs::ConfinedFs;
use crate::exec::construction::{Prepared, SourceRoot, SourceRoots};
use crate::exec::permissions::{InvocationPermissions, ToolDecision};
use cap_fs_ext::{DirExt, MetadataExt};
use cap_std::fs::{Dir, File, Metadata};
use cap_std::time::SystemTime;
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use thiserror::Error;

const MAX_EXPLICIT_IDS: usize = 256;
const MAX_GRANTS_BYTES: usize = 1024 * 1024;
const MAX_GRANTS: usize = 1024;

type Result<T> = std::result::Result<T, SourceError>;

/// Fixed-text diagnostics: parser/OS errors, paths and source contents never escape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub(crate) enum SourceError {
    #[error("gadget support was not opted into in the selected configuration")]
    Disabled,
    #[error("invalid, duplicate or excessive explicit gadget identifiers")]
    InvalidSelector,
    #[error("gadget execution is denied by the invocation policy")]
    Denied,
    #[error("project gadget sources require explicit workspace trust")]
    UntrustedProject,
    #[error("the selected gadget source has not been approved")]
    NeedsApproval,
    #[error("the selected gadget source or component does not exist")]
    NotFound,
    #[error("gadget source is not a valid no-follow regular single-link bundle")]
    InvalidSource,
    #[error("gadget source exceeds its resource bound")]
    Bounds,
    #[error("gadget source changed while preparing its approval snapshot")]
    Changed,
    #[error("selected source-bound user grants are unsafe, invalid or exceed their bounds")]
    Grants,
    #[error("gadget source construction worker failed")]
    Worker,
}

impl From<SourceError> for GadgetError {
    fn from(error: SourceError) -> Self {
        let kind = match error {
            SourceError::Disabled | SourceError::Denied => ErrorKind::DeniedCapability,
            SourceError::InvalidSelector => ErrorKind::InvalidArgs,
            SourceError::UntrustedProject | SourceError::NeedsApproval | SourceError::Grants => {
                ErrorKind::NeedsApproval
            }
            SourceError::NotFound => ErrorKind::NotFound,
            SourceError::InvalidSource | SourceError::Changed => ErrorKind::InvalidGadget,
            SourceError::Bounds => ErrorKind::OutputLimit,
            SourceError::Worker => ErrorKind::Io,
        };
        Self::new(kind, error.to_string())
    }
}

/// Explicit internal source selection, not a new CLI flag or catalog scanner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SourceKind {
    User,
    Project,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ApprovalOrigin {
    /// The first valid snapshot binds an explicitly named id to ONE source and
    /// capability ceiling for this invocation. Later loads cannot widen it.
    Invocation {
        manifest_hash: String,
        component_hash: String,
        capabilities: Capabilities,
        inherited_rebuild: bool,
    },
    /// Existing v1 grants pin the original component/capabilities, not a manifest
    /// hash. Record that distinction and the exact read-only grant-file snapshot.
    UserGrant {
        grant_file_hash: String,
        component_hash: String,
        capabilities: Capabilities,
        inherited_rebuild: bool,
    },
}

/// Internal provenance only. No Serialize/output/trace implementation: labels
/// and manifest data must pass seeded redaction before publication is wired.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ApprovalRecord {
    pub(crate) id: String,
    pub(crate) source: PathBuf,
    pub(crate) kind: SourceKind,
    pub(crate) manifest_hash: String,
    pub(crate) component_hash: String,
    pub(crate) capabilities: Capabilities,
    pub(crate) origin: ApprovalOrigin,
}

/// Private fields prevent declarations or caller-supplied hashes from becoming
/// approved code. Bytes are owned and immutable; never ambient-reopen `source`.
pub(crate) struct ApprovedSource {
    manifest: Manifest,
    manifest_bytes: Vec<u8>,
    component: Vec<u8>,
    approval: ApprovalRecord,
    filesystem: ConfinedFs,
    permissions: Arc<InvocationPermissions>,
}
impl ApprovedSource {
    pub(super) fn filesystem(&self) -> &ConfinedFs {
        &self.filesystem
    }
    pub(super) fn permissions(&self) -> &Arc<InvocationPermissions> {
        &self.permissions
    }
    pub(crate) fn manifest(&self) -> &Manifest {
        &self.manifest
    }
    pub(crate) fn manifest_bytes(&self) -> &[u8] {
        &self.manifest_bytes
    }
    pub(crate) fn component_bytes(&self) -> &[u8] {
        &self.component
    }
    pub(crate) fn approval(&self) -> &ApprovalRecord {
        &self.approval
    }
}

#[derive(Clone)]
pub(crate) struct SourceAuthority(Arc<Authority>);
struct Authority {
    roots: SourceRoots,
    config: PathBuf,
    workspace: PathBuf,
    trusted: bool,
    permissions: Arc<InvocationPermissions>,
    filesystem: ConfinedFs,
    explicit: BTreeSet<String>,
    // Immutable read-only input. External edits cannot add grants mid-invocation.
    grants: Result<GrantSnapshot>,
    // Monotonic first-use binding, NOT mutable legacy ApprovalState/broad always.
    bound: Mutex<BTreeMap<String, ApprovalRecord>>,
    #[cfg(test)]
    hook: Mutex<Option<Hook>>,
}
#[cfg(test)]
type Hook = Arc<dyn Fn(&str, SourceKind, &str) + Send + Sync>;

impl SourceAuthority {
    pub(crate) async fn from_prepared(prepared: &Prepared, explicit: Vec<String>) -> Result<Self> {
        if explicit.len() > MAX_EXPLICIT_IDS
            || explicit.iter().any(|id| !valid_id(id))
            || explicit.iter().collect::<BTreeSet<_>>().len() != explicit.len()
        {
            return Err(SourceError::InvalidSelector);
        }
        if !crate::extensions::terminal_gadgets_enabled(prepared.config()) {
            return Err(SourceError::Disabled);
        }
        if prepared
            .permissions()
            .tool_decision("gadget_call", &json!({}))
            == ToolDecision::Deny
        {
            return Err(SourceError::Denied);
        }
        let roots = prepared.source_roots().clone();
        let config = prepared.config_dir().to_path_buf();
        let workspace = prepared.workspace().to_path_buf();
        let trusted = prepared.discovery().trust_workspace;
        let permissions = prepared.permissions().clone();
        let filesystem = prepared.filesystem().clone();
        tokio::task::spawn_blocking(move || {
            // An explicit approval does not depend on a grant file being valid.
            // Retain a fixed failure for the fallback, never silently repair it.
            let grants = GrantSnapshot::load(roots.config.as_deref());
            Self(Arc::new(Authority {
                roots,
                config,
                workspace,
                trusted,
                permissions,
                filesystem,
                explicit: explicit.into_iter().collect(),
                grants,
                bound: Mutex::new(BTreeMap::new()),
                #[cfg(test)]
                hook: Mutex::new(None),
            }))
        })
        .await
        .map_err(|_| SourceError::Worker)
    }

    /// Selection/discovery remains the future constructor's responsibility.
    /// In particular, an invalid selected project override must NOT fall back
    /// to a user bundle. Project trust is checked before touching its sources.
    pub(crate) async fn authorize(&self, kind: SourceKind, id: &str) -> Result<ApprovedSource> {
        if !valid_id(id) {
            return Err(SourceError::InvalidSelector);
        }
        if kind == SourceKind::Project && !self.0.trusted {
            return Err(SourceError::UntrustedProject);
        }
        let authority = self.0.clone();
        let id = id.to_owned();
        tokio::task::spawn_blocking(move || authority.authorize(kind, &id))
            .await
            .map_err(|_| SourceError::Worker)?
    }
}

impl Authority {
    fn authorize(&self, kind: SourceKind, id: &str) -> Result<ApprovedSource> {
        if self.permissions.tool_decision("gadget_call", &json!({})) == ToolDecision::Deny {
            return Err(SourceError::Denied);
        }
        let (root, base, parts) = match kind {
            SourceKind::User => (
                self.roots.config.as_deref().ok_or(SourceError::NotFound)?,
                &self.config,
                vec!["gadgets", id],
            ),
            SourceKind::Project => (
                self.roots.workspace.as_ref(),
                &self.workspace,
                vec![".craftsman", "gadgets", id],
            ),
        };
        let directory = SourceDirectory::open(root, &parts)?;
        self.stage("directory", kind, id);
        let manifest_file = SourceFile::read(
            &directory.directory,
            "gadget.toml",
            MAX_MANIFEST_BYTES,
            false,
        )?;
        let text =
            std::str::from_utf8(&manifest_file.bytes).map_err(|_| SourceError::InvalidSource)?;
        // Parse a single optional BOM, but hash ALL raw manifest bytes.
        let manifest = Manifest::parse(text.strip_prefix('\u{feff}').unwrap_or(text), id)
            .map_err(|_| SourceError::InvalidSource)?;
        self.stage("manifest", kind, id);
        let component = SourceFile::read(
            &directory.directory,
            "gadget.wasm",
            MAX_COMPONENT_BYTES,
            false,
        )?;
        self.stage("component", kind, id);
        let manifest_hash = digest(&manifest_file.bytes);
        let component_hash = digest(&component.bytes);
        let source = parts
            .iter()
            .fold(base.clone(), |path, name| path.join(name));

        // Verify both whole-byte snapshots and the entire retained parent chain
        // before committing the monotonic invocation binding. No OS writer CAS
        // is promised; executing these OWNED bytes never requires a later reopen.
        manifest_file.validate(&directory.directory, "gadget.toml")?;
        component.validate(&directory.directory, "gadget.wasm")?;
        directory.validate(root, &parts)?;
        let origin = self.origin(
            id,
            kind,
            &source,
            &manifest_hash,
            &component_hash,
            &manifest.capabilities,
        )?;
        Ok(ApprovedSource {
            approval: ApprovalRecord {
                id: id.into(),
                source,
                kind,
                manifest_hash,
                component_hash,
                capabilities: manifest.capabilities.clone(),
                origin,
            },
            manifest,
            manifest_bytes: manifest_file.bytes,
            component: component.bytes,
            filesystem: self.filesystem.clone(),
            permissions: self.permissions.clone(),
        })
    }

    fn origin(
        &self,
        id: &str,
        kind: SourceKind,
        source: &Path,
        manifest_hash: &str,
        component_hash: &str,
        capabilities: &Capabilities,
    ) -> Result<ApprovalOrigin> {
        if self.explicit.contains(id) {
            let mut bound = self.bound.lock().map_err(|_| SourceError::Worker)?;
            let first = bound.entry(id.into()).or_insert_with(|| ApprovalRecord {
                id: id.into(),
                source: source.into(),
                kind,
                manifest_hash: manifest_hash.into(),
                component_hash: component_hash.into(),
                capabilities: capabilities.clone(),
                origin: ApprovalOrigin::Invocation {
                    manifest_hash: manifest_hash.into(),
                    component_hash: component_hash.into(),
                    capabilities: capabilities.clone(),
                    inherited_rebuild: false,
                },
            });
            if first.source != source
                || first.kind != kind
                || !capabilities.is_subset_of(&first.capabilities)
            {
                return Err(SourceError::NeedsApproval);
            }
            return Ok(ApprovalOrigin::Invocation {
                manifest_hash: first.manifest_hash.clone(),
                component_hash: first.component_hash.clone(),
                capabilities: first.capabilities.clone(),
                inherited_rebuild: first.component_hash != component_hash
                    || first.manifest_hash != manifest_hash,
            });
        }
        let grants = self.grants.as_ref().map_err(|e| *e)?;
        let grant = grants
            .entries
            .iter()
            .filter(|g| {
                g.id == id && g.source == source && capabilities.is_subset_of(&g.capabilities)
            })
            // Stable provenance independent of entry order. Prefer an exact
            // component, then the component hash and canonical capabilities.
            .min_by_key(|g| {
                (
                    g.component_hash != component_hash,
                    &g.component_hash,
                    &g.capability_key,
                )
            })
            .ok_or(SourceError::NeedsApproval)?;
        Ok(ApprovalOrigin::UserGrant {
            grant_file_hash: grants.hash.clone(),
            component_hash: grant.component_hash.clone(),
            capabilities: grant.capabilities.clone(),
            inherited_rebuild: grant.component_hash != component_hash,
        })
    }

    fn stage(&self, _stage: &str, _kind: SourceKind, _id: &str) {
        #[cfg(test)]
        if let Some(hook) = self.hook.lock().unwrap().clone() {
            hook(_stage, _kind, _id);
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GrantFile {
    version: u32,
    entries: Vec<Grant>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Grant {
    id: String,
    source: PathBuf,
    component_hash: String,
    capabilities: Capabilities,
    #[serde(skip)]
    capability_key: String,
}
struct GrantSnapshot {
    entries: Vec<Grant>,
    hash: String,
}
impl GrantSnapshot {
    fn load(root: Option<&SourceRoot>) -> Result<Self> {
        let empty = || Self {
            entries: Vec::new(),
            hash: String::new(),
        };
        let Some(root) = root else {
            return Ok(empty());
        };
        let directory = match SourceDirectory::open(root, &["gadgets"]) {
            Ok(directory) => directory,
            Err(SourceError::NotFound) => return Ok(empty()),
            Err(_) => return Err(SourceError::Grants),
        };
        let file =
            match SourceFile::read(&directory.directory, ".grants.json", MAX_GRANTS_BYTES, true) {
                Ok(file) => file,
                Err(SourceError::NotFound) => return Ok(empty()),
                Err(_) => return Err(SourceError::Grants),
            };
        let mut grants: GrantFile =
            serde_json::from_slice(&file.bytes).map_err(|_| SourceError::Grants)?;
        if grants.version != 1 || grants.entries.len() > MAX_GRANTS {
            return Err(SourceError::Grants);
        }
        for grant in &mut grants.entries {
            if !valid_id(&grant.id)
                || !grant.source.is_absolute()
                || grant
                    .source
                    .components()
                    .any(|c| matches!(c, Component::ParentDir))
                || grant
                    .source
                    .to_str()
                    .is_none_or(|s| s.chars().count() > 4096 || s.chars().any(char::is_control))
                || grant.component_hash.len() != 64
                || !grant.component_hash.bytes().all(|b| b.is_ascii_hexdigit())
            {
                return Err(SourceError::Grants);
            }
            grant
                .capabilities
                .validate()
                .map_err(|_| SourceError::Grants)?;
            grant.component_hash.make_ascii_lowercase();
            grant.capability_key =
                serde_json::to_string(&grant.capabilities).map_err(|_| SourceError::Grants)?;
        }
        file.validate(&directory.directory, ".grants.json")
            .map_err(|_| SourceError::Grants)?;
        directory
            .validate(root, &["gadgets"])
            .map_err(|_| SourceError::Grants)?;
        Ok(Self {
            entries: grants.entries,
            hash: digest(&file.bytes),
        })
    }
}

#[derive(Clone, PartialEq, Eq)]
struct Stamp {
    identity: (u64, u64),
    size: u64,
    modified: Option<SystemTime>,
}
impl Stamp {
    fn new(metadata: &Metadata) -> Self {
        // By-handle metadata populates Windows identity too. Not a DACL or
        // portable external-writer CAS; whole-byte validation supplements it.
        Self {
            identity: (metadata.dev(), metadata.ino()),
            size: metadata.len(),
            modified: metadata.modified().ok(),
        }
    }
}
struct SourceDirectory {
    directory: Dir,
    identities: Vec<(u64, u64)>,
}
impl SourceDirectory {
    fn open(root: &SourceRoot, parts: &[&str]) -> Result<Self> {
        let mut directory = root
            .clone_directory()
            .map_err(|_| SourceError::InvalidSource)?;
        let mut identities = Vec::new();
        for part in parts {
            let metadata = directory.symlink_metadata(part).map_err(source_io)?;
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return Err(SourceError::InvalidSource);
            }
            directory = directory.open_dir_nofollow(part).map_err(source_io)?;
            let metadata = directory.dir_metadata().map_err(source_io)?;
            identities.push((metadata.dev(), metadata.ino()));
        }
        Ok(Self {
            directory,
            identities,
        })
    }
    fn validate(&self, root: &SourceRoot, parts: &[&str]) -> Result<()> {
        let current = Self::open(root, parts).map_err(|_| SourceError::Changed)?;
        if current.identities != self.identities {
            return Err(SourceError::Changed);
        }
        Ok(())
    }
}
struct SourceFile {
    file: File,
    bytes: Vec<u8>,
    stamp: Stamp,
    maximum: usize,
    user_only: bool,
}
impl SourceFile {
    fn read(directory: &Dir, name: &str, maximum: usize, user_only: bool) -> Result<Self> {
        let metadata = directory.symlink_metadata(name).map_err(source_io)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(SourceError::InvalidSource);
        }
        let mut file = directory
            .open_with(
                name,
                &crate::platform_os::confined_file_options(false, false),
            )
            .map_err(source_io)?;
        let before = file.metadata().map_err(source_io)?;
        regular(&before, maximum, user_only)?;
        let stamp = Stamp::new(&before);
        let mut bytes = Vec::new();
        (&mut file)
            .take((maximum + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(source_io)?;
        if bytes.len() > maximum {
            return Err(SourceError::Bounds);
        }
        let after = file.metadata().map_err(source_io)?;
        regular(&after, maximum, user_only)?;
        if stamp != Stamp::new(&after) || bytes.len() as u64 != after.len() {
            return Err(SourceError::Changed);
        }
        let current = directory
            .open_with(
                name,
                &crate::platform_os::confined_file_options(false, false),
            )
            .map_err(|_| SourceError::Changed)?;
        let metadata = current.metadata().map_err(source_io)?;
        regular(&metadata, maximum, user_only)?;
        if stamp != Stamp::new(&metadata) {
            return Err(SourceError::Changed);
        }
        Ok(Self {
            file,
            bytes,
            stamp,
            maximum,
            user_only,
        })
    }
    fn validate(&self, directory: &Dir, name: &str) -> Result<()> {
        let current = Self::read(directory, name, self.maximum, self.user_only)
            .map_err(|_| SourceError::Changed)?;
        let metadata = self.file.metadata().map_err(source_io)?;
        regular(&metadata, self.maximum, self.user_only)?;
        if self.stamp != current.stamp
            || self.stamp != Stamp::new(&metadata)
            || self.bytes != current.bytes
        {
            return Err(SourceError::Changed);
        }
        Ok(())
    }
}
fn regular(metadata: &Metadata, maximum: usize, _user_only: bool) -> Result<()> {
    if !metadata.is_file() || metadata.nlink() != 1 {
        return Err(SourceError::InvalidSource);
    }
    if metadata.len() > maximum as u64 {
        return Err(SourceError::Bounds);
    }
    #[cfg(unix)]
    {
        use cap_std::fs::PermissionsExt;
        if _user_only && metadata.permissions().mode() & 0o077 != 0 {
            return Err(SourceError::Grants);
        }
    }
    Ok(())
}
fn source_io(error: std::io::Error) -> SourceError {
    if error.kind() == std::io::ErrorKind::NotFound {
        SourceError::NotFound
    } else {
        SourceError::InvalidSource
    }
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests;
