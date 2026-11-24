//! Explicit local key operations and the `craftsman privacy` command that
//! selects them. Presence is not unlock, ownership approval or privacy, and
//! only the explicit `enable` command changes the user file.
use crate::privacy_key::{ConfigRoot, KeyError, KeyManager, Passphrase, Result, UnlockedVault};

/// `craftsman privacy <command>`. No command sends a model request. `status`
/// and `init` change no configuration; `enable` adds the policy it is asked
/// for and `set` changes the one setting it is asked to, and nothing else.
#[derive(clap::Subcommand)]
pub enum Command {
    /// Show the local privacy policy, mapping-vault key and startup preflight. Read-only.
    Status,
    /// Create the local mapping-vault key and vault. Create-only; asks for a new password twice.
    Init,
    /// Add a privacy policy to your user configuration. Only when it has none.
    Enable {
        /// The installed user gadget to pin as the detector; it must already be approved.
        #[arg(long, value_name = "ID")]
        detector: String,
        /// The detector function to call.
        #[arg(long, value_name = "NAME", default_value = "detect")]
        function: String,
    },
    /// Set one setting of a user configuration that enables privacy: permission_mode, model or agent.
    Set {
        /// The setting to change.
        key: String,
        /// Its new value.
        value: String,
    },
}

pub async fn run(
    command: &Command,
) -> std::result::Result<(), crate::tui::privacy_unlock::PrivateTerminalError> {
    match command {
        Command::Status => println!("{}", crate::privacy_startup::status().await),
        Command::Init => {
            init().await?;
            println!(
                "The mapping-vault key and vault were created. Privacy is not enabled by this command."
            );
        }
        Command::Set { key, value } => {
            crate::privacy_startup::set(key, value).await?;
            println!(
                "The setting was written to your user configuration. It applies to the next start."
            );
        }
        Command::Enable { detector, function } => {
            crate::privacy_startup::enable(detector, function).await?;
            println!(
                "A privacy policy was added to your user configuration. `craftsman privacy status` shows what a start still needs."
            );
        }
    }
    Ok(())
}

/// Create-only initialization through the owned password popup. It refuses
/// before any terminal or prompt when a key file exists; the create-only
/// worker checks absence again, and nothing is adopted or replaced. It
/// enables nothing and changes no configuration.
pub(crate) async fn init()
-> std::result::Result<(), crate::tui::privacy_unlock::PrivateTerminalError> {
    use crate::privacy_bootstrap::{BootstrapError, PrivateBootstrap};
    let Ok(workspace) = std::env::current_dir() else {
        return Err(BootstrapError::Workspace.into());
    };
    let bootstrap = PrivateBootstrap::capture_actual(&workspace)?;
    if bootstrap.status()? != FilePresence::Missing {
        return Err(BootstrapError::Key(KeyError::AlreadyInitialized).into());
    }
    let mut terminal = match crate::tui::privacy_unlock::OwnedSecretTerminal::enter() {
        Ok(terminal) => terminal,
        Err(error) => return Err(BootstrapError::Key(error).into()),
    };
    let created = bootstrap.initialize(&mut terminal).await;
    drop(terminal);
    created?;
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
