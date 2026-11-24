//! The one normal caller of the startup capture. `main` calls it right after
//! the parse, before the legacy loader and every terminal, store, provider or
//! helper effect. Only the owned interactive invocation can be selected; it
//! hands the captured owners to the existing private entry unchanged. With an
//! enabled policy every other invocation is refused here. It writes nothing
//! and enables nothing.
use super::{
    ActualTerminalControls, Result, SourceSelection, StartupError, TerminalStartupCapture,
    UserSourceOrigin, capture_user_source, finish_source,
};
use crate::privacy_bootstrap::BootstrapError;
use crate::privacy_cli::{FilePresence, PrivateKeyOperations};
use crate::privacy_policy::PolicyError;
use crate::tui::privacy_unlock::{OwnedSecretTerminal, PrivateTerminalError};

/// `Ok(None)`: the actual user source proposes no privacy, and the caller goes
/// on to its unchanged legacy loader. `Ok(Some(code))`: the private terminal
/// ran and ended. `Err`: a fixed refusal; nothing fell back to privacy-off.
pub(crate) async fn normal_route(
    cli: &crate::Cli,
) -> std::result::Result<Option<i32>, PrivateTerminalError> {
    let Some((bootstrap, approval)) = select(cli)? else {
        return Ok(None);
    };
    let terminal = match OwnedSecretTerminal::enter() {
        Ok(terminal) => terminal,
        Err(error) => return Err(PrivateTerminalError::Source(BootstrapError::Key(error))),
    };
    let code = terminal.run_captured_private(bootstrap, approval).await?;
    Ok(Some(code))
}

/// Everything before a terminal exists. The owners it returns are the ones
/// the capture issued; nothing is rebuilt from a Config or a caller value.
fn select(
    cli: &crate::Cli,
) -> std::result::Result<
    Option<(
        crate::privacy_bootstrap::PrivateBootstrap,
        super::CapturedApprovalStart,
    )>,
    PrivateTerminalError,
> {
    let captured = match TerminalStartupCapture::capture_for_parsed_cli(cli) {
        Ok(TerminalStartupCapture::Captured(captured)) => captured,
        Ok(TerminalStartupCapture::Legacy(_)) => return Ok(None),
        // The unchanged loader decides whether the user source proposes
        // privacy. It rejects an enabled policy, so a capture refusal can
        // never continue as privacy-off; every other case stays the loader's.
        Err(refusal) => {
            return match crate::config::Config::load() {
                Err(error)
                    if error.downcast_ref::<PolicyError>()
                        == Some(&PolicyError::NotYetAvailable) =>
                {
                    Err(PrivateTerminalError::Startup(refusal))
                }
                _ => Ok(None),
            };
        }
    };
    let startup = captured.into_owned()?;
    require_voice_off(startup.configuration()?)?;
    let approval = startup.approval_start()?;
    let Ok(workspace) = std::env::current_dir() else {
        return Err(PrivateTerminalError::Source(BootstrapError::Workspace));
    };
    let bootstrap = startup.into_private_bootstrap(&workspace)?;
    // The same held root answers for the key before any terminal or key work.
    require_key(bootstrap.status()?)?;
    let Some(requirements) = bootstrap.startup_requirements()? else {
        return Err(PrivateTerminalError::Startup(StartupError::Invocation));
    };
    requirements.select_normal_routing()?;
    Ok(Some((bootstrap, approval)))
}

impl From<StartupError> for PrivateTerminalError {
    fn from(refusal: StartupError) -> Self {
        Self::Startup(refusal)
    }
}
impl From<BootstrapError> for PrivateTerminalError {
    fn from(error: BootstrapError) -> Self {
        Self::Source(error)
    }
}

fn require_voice_off(configuration: &crate::config::Config) -> Result<()> {
    if configuration
        .voice_setting("enabled")
        .and_then(serde_json::Value::as_bool)
        == Some(false)
    {
        Ok(())
    } else {
        Err(StartupError::Voice)
    }
}

fn require_key(presence: FilePresence) -> Result<()> {
    match presence {
        FilePresence::FilesPresent => Ok(()),
        FilePresence::Missing => Err(StartupError::KeyMissing),
        FilePresence::Partial => Err(StartupError::KeyIncomplete),
    }
}

/// Read-only. Fixed text about the actual user source and key files, one
/// statement per line. It creates no terminal, thread, key, vault or grant and
/// runs no guest: the detector's stored source approval is not sampled here.
pub(crate) fn status() -> String {
    let key = match PrivateKeyOperations::capture_actual() {
        Ok(operations) => operations.status(),
        Err(error) => Err(error),
    };
    let mut text = String::from(match key {
        Ok(FilePresence::FilesPresent) => {
            "Mapping-vault key: files present. Presence is not an unlock."
        }
        Ok(FilePresence::Missing) => {
            "Mapping-vault key: not created. `craftsman privacy init` creates it."
        }
        Ok(FilePresence::Partial) => {
            "Mapping-vault key: incomplete files. Nothing is adopted or deleted."
        }
        Err(_) => "Mapping-vault key: the private user configuration root is unavailable.",
    });
    let refusal = match capture_user_source(&UserSourceOrigin::Actual) {
        Ok(SourceSelection::Legacy(_)) => {
            text.push_str("\nPrivacy policy: not enabled in the user configuration.");
            return text;
        }
        Ok(SourceSelection::Proposed(source)) => {
            text.push_str("\nPrivacy policy: enabled in the user configuration.");
            match interactive_readiness(source, key) {
                Ok(()) => {
                    text.push_str(
                        "\nInteractive startup: available. `craftsman` asks for the mapping-vault password; the detector's source approval is checked when it first runs.",
                    );
                    return text;
                }
                Err(refusal) => {
                    text.push_str("\nInteractive startup: refused. ");
                    refusal
                }
            }
        }
        Err(refusal) => {
            text.push_str("\nPrivacy policy: refused. ");
            refusal
        }
    };
    text.push_str(&refusal.to_string());
    text
}

/// Everything the router decides before a terminal exists, for an interactive
/// invocation of this source. The invocation's own terminal is not sampled.
fn interactive_readiness(
    source: crate::privacy_policy::CapturedUserConfiguration,
    key: crate::privacy_key::Result<FilePresence>,
) -> Result<()> {
    ActualTerminalControls {
        other_frontend: false,
        one_shot: false,
        resume: false,
        terminal: true,
    }
    .refuse_unsupported(&source)?;
    let startup = finish_source(source)?.into_owned()?;
    require_voice_off(startup.configuration()?)?;
    match key {
        Ok(presence) => require_key(presence),
        Err(_) => Err(StartupError::Source),
    }
}
