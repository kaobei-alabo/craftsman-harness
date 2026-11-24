//! The one normal caller of the startup capture, and the explicit commands
//! that report on and add a user policy. `main` calls the router only for the
//! eligible owned interactive invocation, before the legacy loader and every
//! terminal, store, provider or helper effect. The router hands the captured
//! owners to the existing private entry unchanged. Nothing here enables
//! privacy by itself; only the explicit `enable` command writes.
use super::{
    ActualTerminalControls, OwnedTerminalStartup, Result, SourceSelection, StartupError,
    TerminalStartupCapture, UserSourceOrigin, capture_user_source, finish_source, policy_error,
};
use crate::privacy_bootstrap::{BootstrapError, PrivateBootstrap};
use crate::privacy_cli::{FilePresence, PrivateKeyOperations};
use crate::privacy_key::ConfigRoot;
use crate::privacy_policy::{CapturedUserConfiguration, UserConfigurationCaptureError};
use crate::tui::privacy_unlock::{OwnedSecretTerminal, PrivateTerminalError};

/// `Ok(None)`: the capture's own answer that the actual user source proposes
/// no privacy; the caller goes on to its unchanged legacy loader.
/// `Ok(Some(code))`: the private terminal ran and ended. `Err`: a fixed
/// refusal. A refusal is final: nothing is loaded again, and a source that
/// could not be captured never continues as privacy-off.
pub(crate) async fn normal_route(
    cli: &crate::Cli,
) -> std::result::Result<Option<i32>, PrivateTerminalError> {
    let captured = match TerminalStartupCapture::capture_for_parsed_cli(cli)? {
        TerminalStartupCapture::Captured(captured) => captured,
        TerminalStartupCapture::Legacy(_) => return Ok(None),
    };
    let startup = captured.into_owned()?;
    let approval = startup.approval_start()?;
    let bootstrap = preflight(startup)?;
    let Some(requirements) = bootstrap.startup_requirements()? else {
        return Err(PrivateTerminalError::Startup(StartupError::Invocation));
    };
    requirements.select_normal_routing()?;
    let terminal = match OwnedSecretTerminal::enter() {
        Ok(terminal) => terminal,
        Err(error) => return Err(PrivateTerminalError::Source(BootstrapError::Key(error))),
    };
    let code = terminal.run_captured_private(bootstrap, approval).await?;
    Ok(Some(code))
}

/// What the router decides read-only, before a terminal, a password or any
/// key work: the source turns voice off, the four owners move into the
/// bootstrap for the current directory, and the key files exist. It unlocks
/// nothing, reaches no model and runs no guest. The detector's stored source
/// approval is not sampled here: the pinned call checks it before any guest
/// work, and `status` reports it.
fn preflight(
    startup: OwnedTerminalStartup,
) -> std::result::Result<PrivateBootstrap, PrivateTerminalError> {
    require_voice_off(startup.configuration()?)?;
    let Ok(workspace) = std::env::current_dir() else {
        return Err(PrivateTerminalError::Source(BootstrapError::Workspace));
    };
    let bootstrap = startup.into_private_bootstrap(&workspace)?;
    match bootstrap.status()? {
        FilePresence::FilesPresent => Ok(bootstrap),
        FilePresence::Missing => Err(StartupError::KeyMissing.into()),
        FilePresence::Partial => Err(StartupError::KeyIncomplete.into()),
    }
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

/// Read-only. Fixed text about the actual user source and key files, one
/// statement per line. It creates no terminal, thread, key, vault or grant and
/// runs no guest. A passed preflight is not an unlocked or supported session:
/// the password and the model connection are checked only by a real start.
pub(crate) async fn status() -> String {
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
    let source = match capture_user_source(&UserSourceOrigin::Actual) {
        Ok(SourceSelection::Legacy(_)) => {
            text.push_str("\nPrivacy policy: not enabled in the user configuration.");
            return text;
        }
        Ok(SourceSelection::Proposed(source)) => source,
        Err(refusal) => {
            text.push_str("\nPrivacy policy: refused. ");
            text.push_str(&refusal.to_string());
            return text;
        }
    };
    text.push_str("\nPrivacy policy: enabled in the user configuration.");
    match interactive_preflight(source).await {
        Ok(()) => text.push_str(
            "\nInteractive startup: read-only preflight passed (policy, voice setting, key files, stored detector approval). Still outstanding until `craftsman` starts: the vault unlock, the detector's own call check, the model connection and the full private session.",
        ),
        Err(refusal) => {
            text.push_str("\nInteractive startup: refused. ");
            text.push_str(&refusal.to_string());
        }
    }
    text
}

/// The router's preflight for an interactive invocation of this source. The
/// invocation's own terminal is not sampled.
async fn interactive_preflight(
    source: CapturedUserConfiguration,
) -> std::result::Result<(), PrivateTerminalError> {
    ActualTerminalControls {
        other_frontend: false,
        one_shot: false,
        resume: false,
        terminal: true,
    }
    .refuse_unsupported(&source)?;
    let bootstrap = preflight(finish_source(source)?.into_owned()?)?;
    // What the first private input would otherwise find out: the pinned
    // detector is installed with a stored approval of exactly its bytes.
    match bootstrap.detector_source_approved().await {
        Ok(()) => Ok(()),
        Err(BootstrapError::Source) => Err(StartupError::Detector.into()),
        Err(error) => Err(error.into()),
    }
}

/// The explicit enable writer. Its invocation is the user's authorization of
/// this one change: it adds a privacy policy to the actual user file, and
/// only when that file has none. Every original byte is kept in place; only
/// the new `privacy` member is inserted. The detector is pinned to the bytes
/// installed now and must already have a stored source approval. Voice is
/// neither turned off nor on, no provider, model or key field is touched, and
/// no grant is written. Nothing is changed unless every check passes.
pub(crate) async fn enable(
    detector: &str,
    function: &str,
) -> std::result::Result<(), PrivateTerminalError> {
    let source = actual_source()?;
    if source.has_policy() {
        return Err(StartupError::PolicyPresent.into());
    }
    let Ok((root, original)) = source.recheck_original() else {
        return Err(StartupError::Source.into());
    };
    let Some((original, lease)) = original else {
        return Err(StartupError::NoUserFile.into());
    };
    // The simplest refusal first. Voice is never turned off for the user.
    match serde_json::from_slice::<crate::config::Config>(original) {
        Ok(configuration) => require_voice_off(&configuration)?,
        Err(_) => return Err(StartupError::Invalid.into()),
    }
    let member = policy_member(root, detector, function).await?;
    let Some(candidate) = candidate(original, &member) else {
        return Err(StartupError::Invalid.into());
    };
    validate(original, &candidate)?;
    match root.replace_user_config(lease, original, &candidate) {
        Ok(()) => {}
        Err(crate::privacy_key::KeyError::Changed) => return Err(StartupError::Source.into()),
        Err(error) => return Err(BootstrapError::Key(error).into()),
    }
    drop(source);
    // Read back: the actual user file now is the candidate and proposes it.
    let written = actual_source()?;
    match written.recheck_original() {
        Ok((_, Some((bytes, _)))) if written.is_enabled() && bytes == candidate.as_slice() => {
            Ok(())
        }
        _ => Err(StartupError::Source.into()),
    }
}

fn actual_source() -> Result<CapturedUserConfiguration> {
    let Ok(root) = ConfigRoot::capture_actual() else {
        return Err(StartupError::Source);
    };
    match CapturedUserConfiguration::capture(root) {
        Ok(source) => Ok(source),
        Err(UserConfigurationCaptureError::Refused(error)) => Err(policy_error(error)),
        Err(UserConfigurationCaptureError::Invalid) => Err(StartupError::Invalid),
        Err(UserConfigurationCaptureError::BeforeSource(_)) => Err(StartupError::Source),
    }
}

/// The new member's text: the detector as it is installed under the held
/// root, pinned to its present bytes, after its stored source approval was
/// found for exactly those bytes. Nothing is compiled, run or approved.
#[cfg(feature = "gadgets")]
async fn policy_member(root: &ConfigRoot, detector: &str, function: &str) -> Result<String> {
    use crate::privacy_hook::PinnedDetector;
    let Ok(workspace) = std::env::current_dir() else {
        return Err(StartupError::Source);
    };
    let Ok(path) = root.with_captured_path(|path| Ok(path.to_owned())) else {
        return Err(StartupError::Source);
    };
    let Ok(pinned) = PinnedDetector::installed(&workspace, &path, detector, function).await else {
        return Err(StartupError::DetectorNotApproved);
    };
    if pinned.source_approved().await.is_err() || root.recheck().is_err() {
        return Err(StartupError::DetectorNotApproved);
    }
    let (component, manifest) = pinned.pins();
    Ok(serde_json::json!({
        "enabled": true,
        "detector": {
            "gadget": detector,
            "function": function,
            "component_sha256": component,
            "manifest_sha256": manifest,
        },
        "identifiers": [],
    })
    .to_string())
}
#[cfg(not(feature = "gadgets"))]
async fn policy_member(_: &ConfigRoot, _: &str, _: &str) -> Result<String> {
    Err(StartupError::DetectorUnavailable)
}

/// `original` with one member inserted right after the opening brace of its
/// top-level object. Every original byte stays where it was.
fn candidate(original: &[u8], member: &str) -> Option<Vec<u8>> {
    let mut open = 0;
    while original.get(open)?.is_ascii_whitespace() {
        open += 1;
    }
    if original[open] != b'{' {
        return None;
    }
    let mut next = open + 1;
    while original.get(next)?.is_ascii_whitespace() {
        next += 1;
    }
    let mut candidate = Vec::with_capacity(original.len() + member.len() + 16);
    candidate.extend_from_slice(&original[..=open]);
    candidate.extend_from_slice(b"\"privacy\":");
    candidate.extend_from_slice(member.as_bytes());
    if original[next] != b'}' {
        candidate.push(b',');
    }
    candidate.extend_from_slice(&original[open + 1..]);
    Some(candidate)
}

/// The complete candidate, before anything is written: every original member
/// is still there with its value and only `privacy` is new; the loader's own
/// policy parser reads it as a valid enabled policy; and it parses as the
/// configuration a startup would take, with voice explicitly off.
fn validate(original: &[u8], candidate: &[u8]) -> Result<()> {
    use crate::privacy_policy::{PolicyError, reject_unavailable_user_source};
    let (Ok(before), Ok(mut after)) = (
        serde_json::from_slice::<serde_json::Value>(original),
        serde_json::from_slice::<serde_json::Value>(candidate),
    ) else {
        return Err(StartupError::Invalid);
    };
    let Some(members) = after.as_object_mut() else {
        return Err(StartupError::Invalid);
    };
    if members.remove("privacy").is_none() || after != before {
        return Err(StartupError::Invalid);
    }
    if reject_unavailable_user_source(candidate) != Err(PolicyError::NotYetAvailable) {
        return Err(StartupError::Invalid);
    }
    let Ok(configuration) = serde_json::from_slice::<crate::config::Config>(candidate) else {
        return Err(StartupError::Invalid);
    };
    require_voice_off(&configuration)
}
