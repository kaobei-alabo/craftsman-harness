//! Read-only destination capture before thread/store/provider effects.
use super::{CAP, Destination, Error, Format};
use crate::exec::construction::SourceRoots;
use cap_fs_ext::{DirExt, MetadataExt};
use cap_std::fs::{Dir, Metadata};
use std::path::{Component, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Identity(pub(super) u64, pub(super) u64);
pub(super) fn identity(m: &Metadata) -> Identity {
    Identity(m.dev(), m.ino())
}
pub(in crate::exec) struct Selected {
    anchor: PathBuf,
    names: Vec<String>,
    name: String,
    format: Format,
    maximum: usize,
}
pub(in crate::exec) struct Target {
    anchor: Dir,
    pub(super) directory: Dir,
    names: Vec<String>,
    identities: Vec<Identity>,
    pub(super) name: String,
    pub(super) format: Format,
    pub(super) maximum: usize,
}
impl Selected {
    pub(in crate::exec) fn select(d: Destination) -> Result<Self, Error> {
        if d.max_bytes == 0 || d.max_bytes > CAP {
            return Err(Error::Limit);
        }
        let raw = d.directory.to_str().ok_or(Error::Path)?;
        if !d.directory.is_absolute()
            || raw.len() > 16 * 1024
            || raw.chars().count() > 4096
            || d.file_name.len() > 255
            || d.file_name.contains(['/', '\\'])
            || !crate::exec::confined_fs::publication_names_safe(std::slice::from_ref(&d.file_name))
        {
            return Err(Error::Path);
        }
        // Components hide some interior dots; reject original spellings first.
        if raw.split(['/', '\\']).any(|p| p == "." || p == "..") {
            return Err(Error::Path);
        }
        let mut anchor = PathBuf::new();
        let mut names = Vec::new();
        for part in d.directory.components() {
            match part {
                Component::RootDir => anchor.push(part.as_os_str()),
                #[cfg(windows)]
                Component::Prefix(prefix) => {
                    if !matches!(
                        prefix.kind(),
                        std::path::Prefix::Disk(_) | std::path::Prefix::VerbatimDisk(_)
                    ) {
                        return Err(Error::Path);
                    }
                    anchor.push(prefix.as_os_str());
                }
                Component::Normal(name) => names.push(name.to_str().ok_or(Error::Path)?.to_owned()),
                _ => return Err(Error::Path),
            }
        }
        if !anchor.is_absolute()
            || names.len() > 128
            || !crate::exec::confined_fs::publication_names_safe(&names)
        {
            return Err(Error::Path);
        }
        Ok(Self {
            anchor,
            names,
            name: d.file_name,
            format: d.format,
            maximum: d.max_bytes,
        })
    }
    pub(in crate::exec) fn capture(self, roots: &SourceRoots) -> Result<Target, Error> {
        let mut forbidden = vec![identity(
            &roots.workspace.clone_directory()?.dir_metadata()?,
        )];
        if let Some(config) = &roots.config {
            forbidden.push(identity(&config.clone_directory()?.dir_metadata()?));
        }
        // Only the OS root/drive is opened ambiently. Every selected normal
        // component is no-follow under a retained handle; no later ambient path.
        let anchor = Dir::open_ambient_dir(&self.anchor, cap_std::ambient_authority())?;
        let mut directory = anchor.try_clone()?;
        let mut identities = Vec::with_capacity(self.names.len() + 1);
        let id = identity(&directory.dir_metadata()?);
        if forbidden.contains(&id) {
            return Err(Error::Scope);
        }
        identities.push(id);
        for name in &self.names {
            let meta = directory.symlink_metadata(name)?;
            if !meta.is_dir() || meta.file_type().is_symlink() {
                return Err(Error::Path);
            }
            directory = directory.open_dir_nofollow(name)?;
            let id = identity(&directory.dir_metadata()?);
            // Refuse physical workspace/config descendants even if an original
            // selected root was renamed after bootstrap. No path-only alias test.
            if forbidden.contains(&id) {
                return Err(Error::Scope);
            }
            identities.push(id);
        }
        let target = Target {
            anchor,
            directory,
            names: self.names,
            identities,
            name: self.name,
            format: self.format,
            maximum: self.maximum,
        };
        target.validate()?;
        target.absent()?;
        Ok(target)
    }
}
impl Target {
    pub(in crate::exec) fn parameters(&self) -> (Format, usize) {
        (self.format, self.maximum)
    }
    pub(super) fn validate(&self) -> Result<(), Error> {
        let mut current = self.anchor.try_clone()?;
        if identity(&current.dir_metadata()?) != self.identities[0] {
            return Err(Error::Changed);
        }
        for (name, expected) in self.names.iter().zip(&self.identities[1..]) {
            current = current
                .open_dir_nofollow(name)
                .map_err(|_| Error::Changed)?;
            if identity(&current.dir_metadata()?) != *expected {
                return Err(Error::Changed);
            }
        }
        if identity(&current.dir_metadata()?) != identity(&self.directory.dir_metadata()?) {
            return Err(Error::Changed);
        }
        Ok(())
    }
    pub(super) fn absent(&self) -> Result<(), Error> {
        match self.directory.symlink_metadata(&self.name) {
            Ok(_) => Err(Error::Exists),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

impl Target {
    /// The composed stdin wait can outlive initial target capture. This uses
    /// ONLY the retained target and never grants a new path or mutates IO.
    pub(in crate::exec) fn revalidate_before_session(&self) -> Result<(), Error> {
        self.validate()?;
        self.absent()
    }
}
