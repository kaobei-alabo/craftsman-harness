//! Explicit captured user data-file declarations. A filename, extension,
//! model label or merged project setting grants no restoration permission.
use super::{PolicyError, Result};
use crate::privacy_restore::FileDataFormat;
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawFileRule {
    workspace: String,
    destination: String,
    format: Format,
    operations: Vec<Operation>,
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
            format: match rule.format {
                Format::PlainText => FileDataFormat::PlainText,
                Format::JsonDocument => FileDataFormat::JsonDocument,
            },
            #[cfg(unix)]
            ancestors: Vec::new(),
            captured: false,
        });
    }
    Ok(rules)
}

impl CapturedFileRule {
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
