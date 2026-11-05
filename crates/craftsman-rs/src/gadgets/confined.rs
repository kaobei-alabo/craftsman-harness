//! Strict byte-oriented filesystem facet for private headless guest imports.
//!
//! This is NOT a guest/source grant or a headless GadgetRuntime constructor.
//! The private source-approved host selects it; model callables stay absent
//! until seeded redaction/provenance and registration boundaries are wired.
//! Existing terminal Workspace/HostState defaults remain unchanged.

use super::manifest::Capabilities;
use super::sources::ApprovedSource;
use super::{ErrorKind, GadgetError};
use crate::exec::confined_fs::{ConfinedFs, FsError};
use crate::exec::construction::Prepared;
use crate::exec::permissions::{InvocationPermissions, ToolDecision};
use serde_json::json;
use std::sync::Arc;

pub(crate) struct DirectoryEntry {
    pub(crate) name: String,
    pub(crate) is_dir: bool,
    pub(crate) size: u64,
}

/// Two independent restrictions: a validated declared workspace capability AND
/// the exact invocation policy already retained by Prepared's filesystem.
/// Declaration validation does not approve a component, even for describe.
pub(crate) struct ConfinedGuestFs {
    filesystem: ConfinedFs,
    permissions: Arc<InvocationPermissions>,
    read: bool,
    write: bool,
}

/// Describe always has empty filesystem/HTTP capabilities; call narrows to the
/// approved declaration. No caller-supplied declaration enters the composed host.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Phase {
    Describe,
    Call,
}

impl ConfinedGuestFs {
    pub(super) fn from_source(source: &ApprovedSource, phase: Phase) -> Self {
        let declaration = &source.manifest().capabilities;
        Self {
            filesystem: source.filesystem().clone(),
            permissions: source.permissions().clone(),
            read: phase == Phase::Call && declaration.read.contains("workspace"),
            write: phase == Phase::Call && declaration.write.contains("workspace"),
        }
    }

    pub(crate) fn from_prepared(
        prepared: &Prepared,
        declaration: Capabilities,
    ) -> Result<Self, GadgetError> {
        declaration.validate()?;
        Ok(Self {
            filesystem: prepared.filesystem().clone(),
            permissions: prepared.permissions().clone(),
            read: declaration.read.contains("workspace"),
            write: declaration.write.contains("workspace"),
        })
    }

    pub(crate) async fn read_file(&self, path: &str) -> Result<Vec<u8>, GadgetError> {
        self.require(self.read, "read_file")?;
        self.filesystem
            .read_raw(path)
            .await
            .map_err(filesystem_error)
    }

    pub(crate) async fn list_dir(&self, path: &str) -> Result<Vec<DirectoryEntry>, GadgetError> {
        self.require(self.read, "ls")?;
        self.filesystem
            .list_raw(path)
            .await
            .map_err(filesystem_error)
            .map(|entries| {
                entries
                    .into_iter()
                    .map(|entry| {
                        let is_dir = entry.path.ends_with('/');
                        DirectoryEntry {
                            name: entry.path.trim_end_matches('/').to_owned(),
                            is_dir,
                            size: entry.size,
                        }
                    })
                    .collect()
            })
    }

    pub(crate) async fn write_file(&self, path: &str, contents: &[u8]) -> Result<(), GadgetError> {
        self.require(self.write, "write_file")?;
        // Preserve the gadget import's established write bound, independently
        // of the confined backend's larger generic raw-file bound.
        if contents.len() > craftsman_gadget::MAX_JSON_BYTES {
            return Err(GadgetError::new(
                ErrorKind::OutputLimit,
                "gadget write exceeds 1 MiB",
            ));
        }
        self.filesystem
            .write_raw(path, contents)
            .await
            .map_err(filesystem_error)
    }

    fn require(&self, declared: bool, tool: &str) -> Result<(), GadgetError> {
        if !declared {
            return Err(GadgetError::new(
                ErrorKind::DeniedCapability,
                "workspace capability was not declared",
            ));
        }
        // Deny (including plan mutation) is final. Let the backend check hard
        // path denies BEFORE an unapproved write, so a config/credential path
        // cannot become an approval request or be opened during preflight.
        if self.permissions.tool_decision(tool, &json!({})) == ToolDecision::Deny {
            return Err(GadgetError::new(
                ErrorKind::DeniedCapability,
                "filesystem tool is denied by the invocation policy",
            ));
        }
        Ok(())
    }
}

fn filesystem_error(error: FsError) -> GadgetError {
    let kind = match error {
        FsError::Unapproved | FsError::ProtectedWrite => ErrorKind::NeedsApproval,
        FsError::NotFound => ErrorKind::NotFound,
        FsError::FileTooLarge | FsError::InputTooLarge | FsError::OutputLimit => {
            ErrorKind::OutputLimit
        }
        FsError::OutsideRoot
        | FsError::ToolDenied
        | FsError::PathDenied
        | FsError::HardDenied
        | FsError::NotFile
        | FsError::Symlink
        | FsError::HardLink
        | FsError::Exists => ErrorKind::DeniedCapability,
        FsError::InvalidScope
        | FsError::InvalidPath
        | FsError::InvalidSelector
        | FsError::NotDirectory
        | FsError::InvalidUtf8
        | FsError::Offset
        | FsError::EmptyEdit
        | FsError::NoMatch
        | FsError::NonUnique => ErrorKind::InvalidArgs,
        FsError::Changed | FsError::Io | FsError::Worker => ErrorKind::Io,
    };
    // FsError is typed and fixed-text; no OS error, path, byte content or config
    // value is echoed. Future producer integration must observe NeedsApproval
    // through the SAME shared invocation fail latch, never string-match errors.
    GadgetError::new(kind, error.to_string())
}

#[cfg(test)]
mod tests;
