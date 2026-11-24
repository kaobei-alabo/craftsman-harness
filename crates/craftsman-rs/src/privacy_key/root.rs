//! Captured actual config-root ownership. Never creates a directory or adopts files.
use super::{KeyError, Result, envelope::ENVELOPE_BYTES};
use std::{fmt, path::PathBuf};
use zeroize::Zeroizing;

pub const KEY_FILENAME: &str = "privacy-key.v1";
pub(crate) const CONFIG_LIMIT: usize = 1024 * 1024;

/// Only the actual user config source (or an owned test directory) can mint this.
pub struct ConfigRoot {
    path: PathBuf,
    #[cfg(unix)]
    ancestors: Vec<std::fs::File>,
    #[cfg(unix)]
    names: Vec<std::ffi::OsString>,
}
impl fmt::Debug for ConfigRoot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PrivacyConfigRoot([private])")
    }
}
impl ConfigRoot {
    /// Retains the same directory authority for independent policy/key owners;
    /// never obtains another root by reopening a caller's string.
    pub(crate) fn duplicate_owned(&self) -> Result<Self> {
        self.recheck()?;
        #[cfg(unix)]
        {
            let copy = Self {
                path: self.path.clone(),
                ancestors: self
                    .ancestors
                    .iter()
                    .map(|file| file.try_clone().map_err(|_| KeyError::Io))
                    .collect::<Result<Vec<_>>>()?,
                names: self.names.clone(),
            };
            copy.recheck()?;
            Ok(copy)
        }
        #[cfg(not(unix))]
        {
            Err(KeyError::SecureStorageUnsupported)
        }
    }

    pub(crate) fn with_captured_path<T>(
        &self,
        use_root: impl FnOnce(&std::path::Path) -> Result<T>,
    ) -> Result<T> {
        self.recheck()?;
        let result = use_root(&self.path)?;
        self.recheck()?;
        Ok(result)
    }

    /// Whether `directory` is this root's own final directory, by the device
    /// and inode of the two held descriptors and by nothing a caller says.
    /// It answers a refusal check for a proposed data location only: no
    /// descriptor, path or identity value leaves this owner, and an identity
    /// that cannot be read is an error, never a "no".
    #[cfg(unix)]
    pub(crate) fn is_final_directory(&self, directory: &std::fs::File) -> Result<bool> {
        let own = self.ancestors.last().ok_or(KeyError::Root)?;
        Ok(identity(&own.metadata().map_err(|_| KeyError::Io)?)
            == identity(&directory.metadata().map_err(|_| KeyError::Io)?))
    }

    pub(crate) fn verify_config_absent(&self) -> Result<()> {
        self.recheck()?;
        #[cfg(unix)]
        {
            if self.exists("config.json")? {
                Err(KeyError::Changed)
            } else {
                Ok(())
            }
        }
        #[cfg(not(unix))]
        {
            Err(KeyError::SecureStorageUnsupported)
        }
    }
    pub(super) fn file_presence(&self) -> Result<(bool, bool)> {
        self.recheck()?;
        #[cfg(unix)]
        {
            let key = self.exists(KEY_FILENAME)?;
            let vault = self.exists("privacy-vault.db")?;
            if key {
                self.open_existing(KEY_FILENAME, true)?;
            }
            if vault {
                self.open_existing("privacy-vault.db", true)?;
            }
            Ok((key, vault))
        }
        #[cfg(not(unix))]
        {
            Err(KeyError::SecureStorageUnsupported)
        }
    }
    pub fn capture_actual() -> Result<Self> {
        #[cfg(unix)]
        {
            Self::capture(crate::config::config_dir())
        }
        #[cfg(not(unix))]
        {
            Err(KeyError::SecureStorageUnsupported)
        }
    }

    #[cfg(all(test, unix))]
    pub(crate) fn capture_owned_startup_fixture(path: PathBuf) -> Result<Self> {
        Self::capture(path)
    }

    #[cfg(all(test, unix))]
    pub(crate) fn capture_owned_test_root(path: &std::path::Path) -> Result<Self> {
        Self::capture(path.to_owned())
    }

    pub(super) fn vault_location(&self) -> Result<crate::privacy_vault::VaultLocation> {
        self.recheck()?;
        crate::privacy_vault::VaultLocation::under_host_config_root(&self.path)
            .map_err(|_| KeyError::Root)
    }

    pub(super) fn ensure_absent_pair(&self) -> Result<()> {
        self.recheck()?;
        #[cfg(unix)]
        {
            for name in [
                KEY_FILENAME,
                "privacy-vault.db",
                "privacy-vault.db-wal",
                "privacy-vault.db-shm",
                "privacy-vault.db-journal",
            ] {
                if self.exists(name)? {
                    return Err(KeyError::AlreadyInitialized);
                }
            }
            Ok(())
        }
        #[cfg(not(unix))]
        {
            Err(KeyError::SecureStorageUnsupported)
        }
    }

    pub(crate) fn read_user_config(&self) -> Result<Option<(Zeroizing<Vec<u8>>, FileLease)>> {
        #[cfg(unix)]
        {
            if !self.exists("config.json")? {
                return Ok(None);
            }
            let lease = self.open_existing("config.json", false)?;
            let bytes = lease.read_bounded(CONFIG_LIMIT, None)?;
            self.recheck()?;
            Ok(Some((bytes, lease)))
        }
        #[cfg(not(unix))]
        {
            Err(KeyError::SecureStorageUnsupported)
        }
    }

    /// The enable writer's one effect: replace the user file that `lease`
    /// holds with `candidate`, only while that lease still reads exactly
    /// `original`. The candidate goes to a new, exclusive, user-only file in
    /// the same held directory, is flushed, and is renamed over `config.json`.
    /// A leftover of that staging name is never adopted or overwritten. A
    /// failure before the rename leaves the user file as it was and removes
    /// only this operation's own staged file. The staleness check is a
    /// sample taken just before the rename, not a writer lease, and no
    /// power-loss durability is claimed.
    pub(crate) fn replace_user_config(
        &self,
        lease: &FileLease,
        original: &[u8],
        candidate: &[u8],
    ) -> Result<()> {
        #[cfg(unix)]
        {
            use std::{io::Write, os::fd::AsRawFd};
            const STAGED: &str = "config.json.privacy-enable";
            if candidate.len() > CONFIG_LIMIT {
                return Err(KeyError::Bounds);
            }
            self.recheck()?;
            let mut file = self
                .open_raw(STAGED, libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL, 0o600)
                .map_err(|_| KeyError::Io)?;
            let directory = self.ancestors.last().ok_or(KeyError::Root)?.as_raw_fd();
            let staged = std::ffi::CString::new(STAGED).map_err(|_| KeyError::Root)?;
            let written = match secure_file(&file.metadata().map_err(|_| KeyError::Io)?, true) {
                Err(error) => Err(error),
                Ok(()) if file.write_all(candidate).is_err() || file.sync_all().is_err() => {
                    Err(KeyError::Io)
                }
                // Staleness: the held user file still is the one that was read.
                Ok(()) => match lease.verify_configuration_bytes(original) {
                    Err(error) => Err(error),
                    Ok(()) => self.recheck(),
                },
            };
            let renamed = match written {
                Err(error) => Err(error),
                Ok(()) => {
                    let status = unsafe {
                        libc::renameat(
                            directory,
                            staged.as_ptr(),
                            directory,
                            c"config.json".as_ptr(),
                        )
                    };
                    if status == 0 {
                        Ok(())
                    } else {
                        Err(KeyError::Io)
                    }
                }
            };
            if renamed.is_err() {
                // Only the exclusive file this call created, by the held directory.
                unsafe { libc::unlinkat(directory, staged.as_ptr(), 0) };
                return renamed;
            }
            self.recheck()
        }
        #[cfg(not(unix))]
        {
            let _ = (lease, original, candidate);
            Err(KeyError::SecureStorageUnsupported)
        }
    }

    pub(super) fn read_envelope(&self) -> Result<(Zeroizing<Vec<u8>>, FileLease)> {
        #[cfg(unix)]
        {
            let lease = self.open_existing(KEY_FILENAME, true)?;
            let bytes = lease.read_bounded(ENVELOPE_BYTES, Some(ENVELOPE_BYTES))?;
            self.recheck()?;
            Ok((bytes, lease))
        }
        #[cfg(not(unix))]
        {
            Err(KeyError::SecureStorageUnsupported)
        }
    }

    pub(super) fn create_envelope(&self, bytes: &[u8; ENVELOPE_BYTES]) -> Result<FileLease> {
        #[cfg(unix)]
        {
            use std::io::Write;
            self.recheck()?;
            let mut file = self
                .open_raw(
                    KEY_FILENAME,
                    libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
                    0o600,
                )
                .map_err(|error| {
                    if error.raw_os_error() == Some(libc::EEXIST) {
                        KeyError::AlreadyInitialized
                    } else {
                        KeyError::Io
                    }
                })?;
            secure_file(&file.metadata().map_err(|_| KeyError::Io)?, true)?;
            file.write_all(bytes).map_err(|_| KeyError::Io)?;
            file.sync_all().map_err(|_| KeyError::Io)?;
            self.recheck()?;
            // Retain a separately no-follow opened readable descriptor only if
            // its complete identity matches the file this operation created.
            let identity = identity(&file.metadata().map_err(|_| KeyError::Io)?);
            let lease = self.open_existing(KEY_FILENAME, true)?;
            if lease.identity != identity {
                return Err(KeyError::Changed);
            }
            let observed = lease.read_bounded(ENVELOPE_BYTES, Some(ENVELOPE_BYTES))?;
            if observed.as_slice() != bytes {
                return Err(KeyError::Changed);
            }
            Ok(lease)
        }
        #[cfg(not(unix))]
        {
            let _ = bytes;
            Err(KeyError::SecureStorageUnsupported)
        }
    }

    pub(crate) fn recheck(&self) -> Result<()> {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            for (index, name) in self.names.iter().enumerate() {
                let parent = &self.ancestors[index];
                let actual = open_directory_at(parent.as_raw_fd(), name)?;
                if identity(&actual.metadata().map_err(|_| KeyError::Io)?)
                    != identity(
                        &self.ancestors[index + 1]
                            .metadata()
                            .map_err(|_| KeyError::Io)?,
                    )
                {
                    return Err(KeyError::Changed);
                }
            }
            for (index, directory) in self.ancestors.iter().enumerate() {
                secure_directory(
                    &directory.metadata().map_err(|_| KeyError::Io)?,
                    index + 1 == self.ancestors.len(),
                )?;
            }
            Ok(())
        }
        #[cfg(not(unix))]
        {
            Err(KeyError::SecureStorageUnsupported)
        }
    }

    #[cfg(unix)]
    fn capture(path: PathBuf) -> Result<Self> {
        use std::{os::fd::AsRawFd, path::Component};
        if !path.is_absolute()
            || path.to_str().is_none()
            || path.as_os_str().len() > 16 * 1024
            || path.components().count() > 128
            || path
                .components()
                .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
        {
            return Err(KeyError::Root);
        }
        let mut ancestors = vec![open_directory_at(
            libc::AT_FDCWD,
            std::ffi::OsStr::new("/"),
        )?];
        let mut names = Vec::new();
        for part in path.components() {
            if let Component::Normal(name) = part {
                let directory =
                    open_directory_at(ancestors.last().ok_or(KeyError::Root)?.as_raw_fd(), name)?;
                names.push(name.to_owned());
                ancestors.push(directory);
            }
        }
        let root = Self {
            path,
            ancestors,
            names,
        };
        root.recheck()?;
        Ok(root)
    }

    #[cfg(unix)]
    fn open_raw(
        &self,
        name: &str,
        flags: i32,
        mode: libc::mode_t,
    ) -> std::io::Result<std::fs::File> {
        use std::os::fd::{AsRawFd, FromRawFd};
        let name = std::ffi::CString::new(name).map_err(|_| std::io::ErrorKind::InvalidInput)?;
        let directory = self
            .ancestors
            .last()
            .ok_or(std::io::ErrorKind::InvalidInput)?;
        let fd = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                flags | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
                mode as libc::c_uint,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(unsafe { std::fs::File::from_raw_fd(fd) })
    }

    #[cfg(unix)]
    fn exists(&self, name: &str) -> Result<bool> {
        use std::os::fd::AsRawFd;
        let name = std::ffi::CString::new(name).map_err(|_| KeyError::Root)?;
        let mut metadata: libc::stat = unsafe { std::mem::zeroed() };
        let directory = self.ancestors.last().ok_or(KeyError::Root)?;
        let status = unsafe {
            libc::fstatat(
                directory.as_raw_fd(),
                name.as_ptr(),
                &mut metadata,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if status == 0 {
            return Ok(true);
        }
        if std::io::Error::last_os_error().raw_os_error() == Some(libc::ENOENT) {
            Ok(false)
        } else {
            Err(KeyError::Io)
        }
    }

    #[cfg(unix)]
    fn open_existing(&self, name: &str, private: bool) -> Result<FileLease> {
        self.recheck()?;
        let file = self
            .open_raw(name, libc::O_RDONLY, 0)
            .map_err(|_| KeyError::Missing)?;
        let metadata = file.metadata().map_err(|_| KeyError::Io)?;
        secure_file(&metadata, private)?;
        let lease = FileLease {
            identity: identity(&metadata),
            metadata,
            file,
            private,
            parent: self
                .ancestors
                .last()
                .ok_or(KeyError::Root)?
                .try_clone()
                .map_err(|_| KeyError::Io)?,
            name: std::ffi::CString::new(name).map_err(|_| KeyError::Root)?,
        };
        lease.recheck()?;
        Ok(lease)
    }
}

pub(crate) struct FileLease {
    #[cfg(unix)]
    file: std::fs::File,
    #[cfg(unix)]
    metadata: std::fs::Metadata,
    #[cfg(unix)]
    identity: (u64, u64),
    #[cfg(unix)]
    private: bool,
    #[cfg(unix)]
    parent: std::fs::File,
    #[cfg(unix)]
    name: std::ffi::CString,
}
impl fmt::Debug for FileLease {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PrivacyFileLease([private])")
    }
}
impl FileLease {
    /// Duplicate this already-held source without opening a caller pathname.
    pub(crate) fn duplicate_owned(&self) -> Result<Self> {
        self.recheck()?;
        #[cfg(unix)]
        {
            let copy = Self {
                file: self.file.try_clone().map_err(|_| KeyError::Io)?,
                metadata: self.metadata.clone(),
                identity: self.identity,
                private: self.private,
                parent: self.parent.try_clone().map_err(|_| KeyError::Io)?,
                name: self.name.clone(),
            };
            copy.recheck()?;
            Ok(copy)
        }
        #[cfg(not(unix))]
        {
            Err(KeyError::SecureStorageUnsupported)
        }
    }

    /// Refusal-only re-verification of this descriptor's bounded original
    /// config bytes. Neither bytes nor a source owner can be supplied by it.
    pub(crate) fn verify_configuration_bytes(&self, expected: &[u8]) -> Result<()> {
        #[cfg(unix)]
        {
            let actual = self.read_bounded(CONFIG_LIMIT, Some(expected.len()))?;
            if actual.as_slice() != expected {
                return Err(KeyError::Changed);
            }
            Ok(())
        }
        #[cfg(not(unix))]
        {
            let _ = expected;
            Err(KeyError::SecureStorageUnsupported)
        }
    }

    pub(crate) fn recheck(&self) -> Result<()> {
        #[cfg(unix)]
        {
            use std::os::{
                fd::{AsRawFd, FromRawFd},
                unix::fs::MetadataExt,
            };
            let actual = self.file.metadata().map_err(|_| KeyError::Io)?;
            secure_file(&actual, self.private)?;
            let descriptor = unsafe {
                libc::openat(
                    self.parent.as_raw_fd(),
                    self.name.as_ptr(),
                    libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
                )
            };
            if descriptor < 0 {
                return Err(KeyError::Changed);
            }
            let named = unsafe { std::fs::File::from_raw_fd(descriptor) };
            let named = named.metadata().map_err(|_| KeyError::Io)?;
            secure_file(&named, self.private)?;
            if identity(&named) != identity(&actual) {
                return Err(KeyError::Changed);
            }
            if identity(&actual) != self.identity
                || actual.len() != self.metadata.len()
                || actual.mtime() != self.metadata.mtime()
                || actual.mtime_nsec() != self.metadata.mtime_nsec()
                || actual.ctime() != self.metadata.ctime()
                || actual.ctime_nsec() != self.metadata.ctime_nsec()
            {
                return Err(KeyError::Changed);
            }
            Ok(())
        }
        #[cfg(not(unix))]
        {
            Err(KeyError::SecureStorageUnsupported)
        }
    }

    #[cfg(unix)]
    fn read_bounded(&self, maximum: usize, exact: Option<usize>) -> Result<Zeroizing<Vec<u8>>> {
        use std::os::unix::fs::FileExt;
        self.recheck()?;
        let length = usize::try_from(self.metadata.len()).map_err(|_| KeyError::Bounds)?;
        if length > maximum || exact.is_some_and(|n| n != length) {
            return Err(KeyError::Invalid);
        }
        let mut bytes = Zeroizing::new(vec![0; length]);
        let mut read = 0;
        while read < length {
            let count = self
                .file
                .read_at(&mut bytes[read..], read as u64)
                .map_err(|_| KeyError::Io)?;
            if count == 0 {
                return Err(KeyError::Changed);
            }
            read += count;
        }
        let mut tail = [0];
        if self
            .file
            .read_at(&mut tail, length as u64)
            .map_err(|_| KeyError::Io)?
            != 0
        {
            return Err(KeyError::Changed);
        }
        self.recheck()?;
        Ok(bytes)
    }
}

#[cfg(unix)]
fn identity(meta: &std::fs::Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt;
    (meta.dev(), meta.ino())
}
#[cfg(unix)]
fn secure_file(meta: &std::fs::Metadata, private: bool) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    if !meta.is_file()
        || meta.nlink() != 1
        || meta.uid() != unsafe { libc::geteuid() }
        || (private && meta.mode() & 0o7777 != 0o600)
        || (!private && meta.mode() & 0o022 != 0)
    {
        return Err(KeyError::Root);
    }
    Ok(())
}
#[cfg(unix)]
fn secure_directory(meta: &std::fs::Metadata, final_root: bool) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    let user = unsafe { libc::geteuid() };
    let root_sticky = meta.uid() == 0 && meta.mode() & 0o1000 != 0;
    if !meta.is_dir()
        || (meta.uid() != 0 && meta.uid() != user)
        || (meta.mode() & 0o022 != 0 && !root_sticky)
        || (final_root && (meta.uid() != user || meta.mode() & 0o077 != 0))
    {
        return Err(KeyError::Root);
    }
    Ok(())
}
#[cfg(unix)]
fn open_directory_at(parent: i32, name: &std::ffi::OsStr) -> Result<std::fs::File> {
    use std::os::{fd::FromRawFd, unix::ffi::OsStrExt};
    let name = std::ffi::CString::new(name.as_bytes()).map_err(|_| KeyError::Root)?;
    let descriptor = unsafe {
        libc::openat(
            parent,
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if descriptor < 0 {
        return Err(KeyError::Root);
    }
    Ok(unsafe { std::fs::File::from_raw_fd(descriptor) })
}
