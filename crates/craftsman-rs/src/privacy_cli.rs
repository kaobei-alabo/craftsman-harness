//! Private explicit operations only. No clap command/flag, enable writer or
//! frontend selects these; presence is not unlock, ownership approval or privacy.
use crate::privacy_key::{ConfigRoot, KeyManager, Passphrase, Result, UnlockedVault};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilePresence {
    Missing,
    Partial,
    FilesPresent,
}
pub struct PrivateKeyOperations {
    keys: KeyManager,
}
impl std::fmt::Debug for PrivateKeyOperations {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PrivateKeyOperations([private])")
    }
}
impl PrivateKeyOperations {
    pub fn capture_actual() -> Result<Self> {
        Ok(Self::from_captured_root(ConfigRoot::capture_actual()?))
    }
    /// The caller must already own the actual user root. No path, flag or
    /// merged configuration value can substitute for this retained authority.
    pub fn from_captured_root(root: ConfigRoot) -> Self {
        Self {
            keys: KeyManager::from_root(root),
        }
    }
    pub fn status(&self) -> Result<FilePresence> {
        Ok(match self.keys.files_present()? {
            (false, false) => FilePresence::Missing,
            (true, true) => FilePresence::FilesPresent,
            _ => FilePresence::Partial,
        })
    }
    pub async fn initialize(
        &self,
        first: Passphrase,
        confirmation: Passphrase,
    ) -> Result<UnlockedVault> {
        self.keys.initialize(first.confirm(confirmation)?).await
    }
    pub async fn unlock(&self, passphrase: Passphrase) -> Result<UnlockedVault> {
        self.keys.unlock(passphrase).await
    }
    pub async fn close_and_wait(&self) -> Result<()> {
        self.keys.close_and_wait().await
    }
}
