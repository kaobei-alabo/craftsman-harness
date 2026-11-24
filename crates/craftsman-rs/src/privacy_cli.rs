//! Explicit local key operations and the `craftsman privacy` command that
//! selects them. There is no enable writer; presence is not unlock, ownership
//! approval or privacy.
use crate::privacy_key::{ConfigRoot, KeyError, KeyManager, Passphrase, Result, UnlockedVault};

/// `craftsman privacy <command>`. Neither command enables privacy, changes the
/// user configuration or sends a model request.
#[derive(clap::Subcommand)]
pub enum Command {
    /// Show the local privacy policy, mapping-vault key and startup state. Read-only.
    Status,
    /// Create the local mapping-vault key and vault. Create-only; asks for a new password twice.
    Init,
}

pub async fn run(command: &Command) -> crate::privacy_bootstrap::Result<()> {
    use crate::privacy_bootstrap::{BootstrapError, PrivateBootstrap};
    match command {
        Command::Status => println!("{}", crate::privacy_startup::status()),
        Command::Init => {
            let Ok(workspace) = std::env::current_dir() else {
                return Err(BootstrapError::Workspace);
            };
            let bootstrap = PrivateBootstrap::capture_actual(&workspace)?;
            // Refuse before any terminal or password prompt. The create-only
            // worker checks absence again; nothing is adopted or replaced.
            if bootstrap.status()? != FilePresence::Missing {
                return Err(BootstrapError::Key(KeyError::AlreadyInitialized));
            }
            let mut terminal = crate::tui::privacy_unlock::OwnedSecretTerminal::enter()?;
            let created = bootstrap.initialize(&mut terminal).await;
            drop(terminal);
            created?;
            println!(
                "The mapping-vault key and vault were created. Privacy is not enabled by this command."
            );
        }
    }
    Ok(())
}

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
