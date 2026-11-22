//! Explicit captured user data-file declarations. A filename, extension,
//! model label or merged project setting grants no restoration permission.
use super::{PolicyError, Result};
use crate::privacy_key::ConfigRoot;
use crate::privacy_restore::FileDataFormat;
use crate::privacy_session::{
    FileChange, FileDataRequest, FileEffectRequest, RetainedFileEffect, SupportedDataApplication,
    SupportedFileData,
};
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};

/// Additional app source-role exclusions. These are REFUSALS only; a path
/// that is absent from this list still needs the actual captured user owner,
/// compiled application/schema/ranges and retained final-effect grant.
pub(super) fn protected_role(destination: &Path) -> bool {
    for part in destination.components() {
        let Some(part) = part.as_os_str().to_str() else {
            return true;
        };
        let part = part.to_ascii_lowercase();
        if matches!(
            part.as_str(),
            ".git"
                | ".craftsman"
                | ".claude"
                | ".agents"
                | ".skills"
                | ".ssh"
                | ".aws"
                | ".config"
                | ".github"
                | "node_modules"
                | "instructions"
                | "instruction"
                | "rules"
                | "schemas"
                | "schema"
                | "src"
                | "scripts"
                | "sdk"
        ) {
            return true;
        }
    }
    let Some(name) = destination.file_name().and_then(|name| name.to_str()) else {
        return true;
    };
    let name = name.to_ascii_lowercase();
    if name.starts_with(".env")
        || name.contains("credential")
        || name.contains("schema")
        || name.contains("instruction")
        || matches!(
            name.as_str(),
            "agents.md"
                | "claude.md"
                | "config.json"
                | "package.json"
                | "manifest.json"
                | "tsconfig.json"
                | "privacy-key.v1"
                | "privacy-vault.db"
                | "privacy-vault.db-wal"
                | "privacy-vault.db-shm"
        )
    {
        return true;
    }
    destination.extension().is_some_and(|extension| {
        extension.to_str().is_none_or(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "md" | "mdx"
                    | "rst"
                    | "rs"
                    | "py"
                    | "js"
                    | "ts"
                    | "tsx"
                    | "jsx"
                    | "sh"
                    | "ps1"
                    | "bat"
                    | "cmd"
                    | "wasm"
                    | "wat"
                    | "c"
                    | "cc"
                    | "cpp"
                    | "h"
                    | "hpp"
                    | "go"
                    | "rb"
                    | "java"
                    | "html"
                    | "sql"
                    | "toml"
                    | "ini"
                    | "cfg"
                    | "yaml"
                    | "yml"
            )
        })
    })
}

#[cfg(all(test, unix))]
mod native_role_tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};
    #[test]
    fn retained_parent_requires_actual_regular_single_link_nonexecuting_data() {
        let owner = tempfile::tempdir().unwrap();
        let workspace = owner.path().canonicalize().unwrap();
        let parent = workspace.join("data");
        std::fs::create_dir(&parent).unwrap();
        let destination = parent.join("record.txt");
        let mut rules = validate_rules(vec![RawFileRule {
            workspace: workspace.to_str().unwrap().into(),
            destination: destination.to_str().unwrap().into(),
            format: Format::PlainText,
            operations: vec![Operation::WriteFile, Operation::EditFile],
            application: Some(Application::ContactLinesV1),
        }])
        .unwrap();
        let rule = &mut rules[0];
        rule.capture().unwrap();
        assert!(rule.native_data_role(FileChange::Create));
        std::fs::write(&destination, "Contact alice@example.invalid\n").unwrap();
        std::fs::set_permissions(&destination, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(rule.native_data_role(FileChange::Update));
        assert!(!rule.native_data_role(FileChange::Create));
        std::fs::set_permissions(&destination, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(!rule.native_data_role(FileChange::Update));
        std::fs::set_permissions(&destination, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::hard_link(&destination, parent.join("second-link")).unwrap();
        assert!(!rule.native_data_role(FileChange::Update));
        std::fs::remove_file(parent.join("second-link")).unwrap();
        std::fs::rename(&destination, parent.join("retained-record")).unwrap();
        symlink(parent.join("retained-record"), &destination).unwrap();
        assert!(!rule.native_data_role(FileChange::Update));
        std::fs::rename(&parent, workspace.join("retained-parent")).unwrap();
        std::fs::create_dir(&parent).unwrap();
        assert!(rule.recheck().is_err());
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawFileRule {
    workspace: String,
    destination: String,
    format: Format,
    operations: Vec<Operation>,
    #[serde(default)]
    application: Option<Application>,
}
#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Application {
    ContactLinesV1,
    ContactJsonV1,
}
#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Format {
    PlainText,
    JsonDocument,
}
#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Operation {
    WriteFile,
    EditFile,
}

pub(super) struct CapturedFileRule {
    workspace: PathBuf,
    destination: PathBuf,
    format: FileDataFormat,
    application: Option<SupportedDataApplication>,
    #[cfg(unix)]
    ancestors: Vec<(PathBuf, std::fs::File)>,
    captured: bool,
}

fn absolute(value: &str) -> Result<PathBuf> {
    let path = Path::new(value);
    if value.is_empty()
        || value.len() > 4096
        || value.chars().any(char::is_control)
        || !path.is_absolute()
        || path.components().count() > 128
        || path
            .components()
            .any(|part| matches!(part, Component::CurDir | Component::ParentDir))
        || value
            .split(std::path::is_separator)
            .any(|part| part == "." || part == "..")
    {
        return Err(PolicyError::Invalid);
    }
    Ok(path.to_owned())
}

pub(super) fn validate_rules(raw: Vec<RawFileRule>) -> Result<Vec<CapturedFileRule>> {
    if raw.len() > 16 {
        return Err(PolicyError::Invalid);
    }
    let mut rules: Vec<CapturedFileRule> = Vec::new();
    for rule in raw {
        let workspace = absolute(&rule.workspace)?;
        let destination = absolute(&rule.destination)?;
        let format = match rule.format {
            Format::PlainText => FileDataFormat::PlainText,
            Format::JsonDocument => FileDataFormat::JsonDocument,
        };
        let application = match rule.application {
            Some(Application::ContactLinesV1) if matches!(format, FileDataFormat::PlainText) => {
                Some(SupportedDataApplication::ContactLinesV1)
            }
            Some(Application::ContactJsonV1) if matches!(format, FileDataFormat::JsonDocument) => {
                Some(SupportedDataApplication::ContactJsonV1)
            }
            Some(_) => return Err(PolicyError::Invalid),
            None => None,
        };
        // The current consumer supports these two content operations together.
        // A narrower/unknown operation set is refused instead of broadened.
        if rule.operations.len() != 2
            || !rule.operations.contains(&Operation::WriteFile)
            || !rule.operations.contains(&Operation::EditFile)
            || !destination.starts_with(&workspace)
            || destination == workspace
            || destination.file_name().is_none()
            || rules
                .iter()
                .any(|old| old.workspace == workspace && old.destination == destination)
        {
            return Err(PolicyError::Invalid);
        }
        rules.push(CapturedFileRule {
            workspace,
            destination,
            format,
            application,
            #[cfg(unix)]
            ancestors: Vec::new(),
            captured: false,
        });
    }
    Ok(rules)
}

impl CapturedFileRule {
    fn native_data_role(&self, change: FileChange) -> bool {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let Some((path, parent)) = self.ancestors.last() else {
                return false;
            };
            if Some(path.as_path()) != self.destination.parent() {
                return false;
            }
            let Some(name) = self.destination.file_name() else {
                return false;
            };
            let Ok(name) = std::ffi::CString::new(name.as_encoded_bytes()) else {
                return false;
            };
            let mut observed = std::mem::MaybeUninit::<libc::stat>::uninit();
            let result = unsafe {
                libc::fstatat(
                    parent.as_raw_fd(),
                    name.as_ptr(),
                    observed.as_mut_ptr(),
                    libc::AT_SYMLINK_NOFOLLOW,
                )
            };
            if result != 0 {
                return change == FileChange::Create
                    && std::io::Error::last_os_error().raw_os_error() == Some(libc::ENOENT);
            }
            // Data updates must have the real regular/single-link/current-user
            // non-executable role. The core still uses its verified held file
            // and rechecks this source/effect immediately before any write.
            let observed = unsafe { observed.assume_init() };
            change == FileChange::Update
                && observed.st_mode & libc::S_IFMT == libc::S_IFREG
                && observed.st_nlink == 1
                && observed.st_uid == unsafe { libc::geteuid() }
                && observed.st_mode & 0o111 == 0
        }
        #[cfg(not(unix))]
        {
            let _ = change;
            false
        }
    }
    fn matches_request(&self, workspace: &Path, request: &FileDataRequest) -> bool {
        workspace == self.workspace
            && request.destination() == self.destination
            && matches!(request.operation(), FileChange::Create | FileChange::Update)
            && std::mem::discriminant(&request.format()) == std::mem::discriminant(&self.format)
            && self.recheck().is_ok()
            && self.native_data_role(request.operation())
    }
    pub(super) fn supported(
        &self,
        workspace: &Path,
        request: &FileDataRequest,
    ) -> Option<SupportedFileData> {
        if !self.matches_request(workspace, request) {
            return None;
        }
        // A missing application is an inert format-only proposal. A present
        // enum selects the ACTUAL compiled object/range validator; it is never
        // treated as a purpose/schema string grant.
        let supported = request.validate_application(self.application?).ok()?;
        self.recheck().ok()?;
        if !self.native_data_role(request.operation()) {
            return None;
        }
        Some(supported)
    }
    pub(super) fn retained(
        &self,
        workspace: &Path,
        request: &FileEffectRequest,
    ) -> Option<RetainedFileEffect> {
        self.supported(workspace, request.data())?;
        #[cfg(unix)]
        {
            let (path, directory) = self.ancestors.last()?;
            if Some(path.as_path()) != self.destination.parent() {
                return None;
            }
            let retained = request.bind_retained_parent(directory).ok()?;
            self.recheck().ok()?;
            Some(retained)
        }
        #[cfg(not(unix))]
        {
            None
        }
    }
    #[cfg(unix)]
    fn open_root() -> Result<std::fs::File> {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open("/")
            .map_err(|_| PolicyError::Source)
    }
    #[cfg(unix)]
    fn open_child(parent: &std::fs::File, name: &std::ffi::OsStr) -> Result<std::fs::File> {
        use std::os::fd::{AsRawFd, FromRawFd};
        let name =
            std::ffi::CString::new(name.as_encoded_bytes()).map_err(|_| PolicyError::Invalid)?;
        let descriptor = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if descriptor < 0 {
            return Err(PolicyError::Source);
        }
        // SAFETY: openat returned a new owned descriptor; no other owner closes
        // it. This chain retains every no-follow ancestor independently.
        Ok(unsafe { std::fs::File::from_raw_fd(descriptor) })
    }
    pub(super) fn capture(&mut self) -> Result<()> {
        #[cfg(unix)]
        {
            if self.captured || !self.ancestors.is_empty() {
                return Err(PolicyError::Source);
            }
            let parent = self.destination.parent().ok_or(PolicyError::Invalid)?;
            self.ancestors
                .push((PathBuf::from("/"), Self::open_root()?));
            let mut path = PathBuf::from("/");
            for part in parent.components() {
                let Component::Normal(name) = part else {
                    continue;
                };
                let held = &self.ancestors.last().ok_or(PolicyError::Source)?.1;
                let directory = Self::open_child(held, name)?;
                path.push(part.as_os_str());
                self.ancestors.push((path.clone(), directory));
            }
            if !self
                .ancestors
                .iter()
                .any(|(path, _)| path == &self.workspace)
            {
                return Err(PolicyError::Source);
            }
            self.captured = true;
            self.recheck()
        }
        #[cfg(not(unix))]
        {
            Err(PolicyError::Source)
        }
    }
    pub(super) fn recheck(&self) -> Result<()> {
        if !self.captured {
            return Err(PolicyError::Source);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let mut named = Self::open_root()?;
            for (index, (path, held)) in self.ancestors.iter().enumerate() {
                if index != 0 {
                    named = Self::open_child(&named, path.file_name().ok_or(PolicyError::Source)?)?;
                }
                let before = held.metadata().map_err(|_| PolicyError::Source)?;
                let after = named.metadata().map_err(|_| PolicyError::Source)?;
                if !before.is_dir()
                    || !after.is_dir()
                    || before.dev() != after.dev()
                    || before.ino() != after.ino()
                {
                    return Err(PolicyError::Source);
                }
            }
            Ok(())
        }
        #[cfg(not(unix))]
        {
            Err(PolicyError::Source)
        }
    }
    /// Whether this is the rule that names `destination`.
    pub(super) fn names(&self, destination: &Path) -> bool {
        destination == self.destination
    }
    /// Whether one of the directories this rule retains on the way to its
    /// destination is the user's configured root itself. The device and inode
    /// of the held descriptors decide, so another spelling of that directory
    /// does not pass. A rule that retains nothing yet, or an identity that
    /// cannot be read, counts as inside: this answer only ever refuses.
    pub(super) fn reaches_root(&self, root: &ConfigRoot) -> bool {
        #[cfg(unix)]
        {
            !self.captured
                || self
                    .ancestors
                    .iter()
                    .any(|(_, held)| root.is_final_directory(held) != Ok(false))
        }
        #[cfg(not(unix))]
        {
            let _ = root;
            true
        }
    }
    /// Test-only. A format read from two paths grants nothing: production
    /// asks `supported` and `retained` for an actual request and nothing else.
    #[cfg(test)]
    pub(super) fn format_for(
        &self,
        workspace: &Path,
        destination: &Path,
    ) -> Option<FileDataFormat> {
        if workspace != self.workspace || destination != self.destination || self.recheck().is_err()
        {
            return None;
        }
        Some(self.format)
    }
}
