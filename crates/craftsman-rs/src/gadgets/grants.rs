//! User-level grants, never a project-provided trust file. A grant is bound to
//! the source directory too, so a project cannot shadow an approved user
//! gadget with another component that happens to have the same id.

use super::manifest::Capabilities;
use super::{ErrorKind, GadgetError};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const MAX_GRANTS_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Grant {
    id: String,
    source: PathBuf,
    component_hash: String,
    capabilities: Capabilities,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Grants {
    version: u32,
    entries: Vec<Grant>,
}

impl Default for Grants {
    fn default() -> Self {
        Self {
            version: 1,
            entries: Vec::new(),
        }
    }
}

pub struct GrantStore {
    directory: PathBuf,
}

fn io_error(error: impl std::fmt::Display) -> GadgetError {
    GadgetError::new(
        ErrorKind::NeedsApproval,
        format!("cannot read/write gadget grants: {error}"),
    )
}

fn secure_open(path: &Path, create: bool) -> Result<File, GadgetError> {
    let mut options = OpenOptions::new();
    options.read(true).write(create).create(create);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    if fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(io_error("grant files may not be symlinks"));
    }
    let file = options.open(path).map_err(io_error)?;
    let metadata = file.metadata().map_err(io_error)?;
    if !metadata.is_file() {
        return Err(io_error("grant path must be a regular file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(io_error("grant files must be user-only (chmod 600)"));
        }
    }
    Ok(file)
}

impl GrantStore {
    pub fn new(config_dir: &Path) -> Self {
        Self {
            directory: config_dir.join("gadgets"),
        }
    }

    fn load(&self) -> Result<Grants, GadgetError> {
        let path = self.directory.join(".grants.json");
        if !path.try_exists().map_err(io_error)? {
            // A dangling symlink must not look like an absent grant file.
            if fs::symlink_metadata(&path).is_ok() {
                return Err(io_error("invalid grant path"));
            }
            return Ok(Grants::default());
        }
        let file = secure_open(&path, false)?;
        let mut bytes = Vec::new();
        file.take((MAX_GRANTS_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(io_error)?;
        if bytes.len() > MAX_GRANTS_BYTES {
            return Err(io_error("grant file exceeds 1 MiB"));
        }
        let grants: Grants = serde_json::from_slice(&bytes).map_err(io_error)?;
        if grants.version != 1 || grants.entries.len() > 1024 {
            return Err(io_error("unsupported or oversized grant file"));
        }
        for grant in &grants.entries {
            if grant.component_hash.len() != 64
                || !grant.component_hash.bytes().all(|b| b.is_ascii_hexdigit())
            {
                return Err(io_error("invalid component hash in grants"));
            }
            grant.capabilities.validate().map_err(io_error)?;
        }
        Ok(grants)
    }

    pub fn approved(
        &self,
        id: &str,
        source: &Path,
        hash: &str,
        capabilities: &Capabilities,
    ) -> Result<bool, GadgetError> {
        Ok(self.load()?.entries.iter().any(|g| {
            g.id == id
                && g.source == source
                && (g.component_hash == hash || capabilities.is_subset_of(&g.capabilities))
                && capabilities.is_subset_of(&g.capabilities)
        }))
    }

    pub fn approve(
        &self,
        id: &str,
        source: &Path,
        hash: &str,
        capabilities: &Capabilities,
    ) -> Result<(), GadgetError> {
        fs::create_dir_all(&self.directory).map_err(io_error)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.directory, fs::Permissions::from_mode(0o700))
                .map_err(io_error)?;
        }
        // Serialize read-modify-rename across processes, not only sessions in
        // this process. Closing the handle releases the OS file lock.
        let lock = secure_open(&self.directory.join(".grants.lock"), true)?;
        lock.lock().map_err(io_error)?;
        let mut grants = self.load()?;
        grants
            .entries
            .retain(|g| !(g.id == id && g.source == source && g.component_hash == hash));
        grants.entries.push(Grant {
            id: id.into(),
            source: source.into(),
            component_hash: hash.into(),
            capabilities: capabilities.clone(),
        });
        if grants.entries.len() > 1024 {
            return Err(io_error("too many gadget grants"));
        }
        let bytes = serde_json::to_vec_pretty(&grants).map_err(io_error)?;
        if bytes.len() > MAX_GRANTS_BYTES {
            return Err(io_error("grant file exceeds 1 MiB"));
        }
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let temporary = self.directory.join(format!(
            ".grants.tmp-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let result = (|| {
            let mut file = options.open(&temporary).map_err(io_error)?;
            file.write_all(&bytes).map_err(io_error)?;
            file.sync_all().map_err(io_error)?;
            drop(file);
            fs::rename(&temporary, self.directory.join(".grants.json")).map_err(io_error)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }
}
