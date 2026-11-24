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
    let bootstrap = preflight(startup, true)?;
    // The entry's own read-only eligibility, with the captured source's
    // strict settings, before any terminal effect.
    OwnedSecretTerminal::check_captured_bootstrap(&bootstrap)?;
    let Some(requirements) = bootstrap.startup_requirements()? else {
        return Err(PrivateTerminalError::Startup(StartupError::Invocation));
    };
    requirements.select_normal_routing()?;
    let mut terminal = match OwnedSecretTerminal::enter() {
        Ok(terminal) => terminal,
        Err(error) => return Err(PrivateTerminalError::Source(BootstrapError::Key(error))),
    };
    // Only this selected startup may change the person's settings from
    // inside its conversation and carry the conversation over.
    terminal.lend_router_settings();
    let code = terminal.run_captured_private(bootstrap, approval).await?;
    Ok(Some(code))
}

/// What the router decides read-only, before a terminal, a password or any
/// key work: the source turns voice off, the permission mode of this start
/// still asks before a file write, the four owners move into the
/// bootstrap for the current directory, and the key files exist. It unlocks
/// nothing, reaches no model and runs no guest. The detector's stored source
/// approval is not sampled here: the pinned call checks it before any guest
/// work, and `status` reports it. Only the real startup reserves its fresh
/// conversation (`reserve`); the status inspection reserves nothing.
fn preflight(
    startup: OwnedTerminalStartup,
    reserve: bool,
) -> std::result::Result<PrivateBootstrap, PrivateTerminalError> {
    require_voice_off(startup.configuration()?)?;
    require_asking_mode(&startup)?;
    let Ok(workspace) = std::env::current_dir() else {
        return Err(PrivateTerminalError::Source(BootstrapError::Workspace));
    };
    let bootstrap = match reserve {
        true => startup.into_private_bootstrap(&workspace)?,
        false => startup.into_inspection(&workspace)?,
    };
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

/// The permission mode this start would run under still asks before a file
/// write: the mode of the invocation's own option, or else the captured
/// file's, and no automatic approval from either. See
/// [`mode_asks_before_writes`]. Nothing is changed for the user.
fn require_asking_mode(startup: &OwnedTerminalStartup) -> Result<()> {
    let configuration = startup.configuration()?;
    let (mut mode, mut automatic) = (configuration.permission_mode, configuration.auto_approve);
    if let Some(options) = startup.requirements().terminal_options() {
        if let Some(chosen) = options.permission_mode_override() {
            mode = chosen;
        }
        automatic |= options.auto_approve_override();
    }
    if automatic || !mode_asks_before_writes(mode) {
        return Err(StartupError::ModeDecidesWritesAtStart);
    }
    Ok(())
}

/// A build without voice input refuses a source that does not turn voice
/// off: the setting it asks for could not be honoured, and it is never
/// turned off for the user. A build with voice input keeps the setting; the
/// private terminal owns the ordinary voice controller.
fn require_voice_off(configuration: &crate::config::Config) -> Result<()> {
    if cfg!(feature = "voice")
        || configuration
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
            "\nInteractive startup: read-only preflight passed (policy, voice setting, key files, stored detector approval). Not checked until `craftsman` starts: the vault unlock, the detector's own call check, the model connection, native-view support for the configured model wire, and support for every requested session feature. A passed preflight is not an unlock, an approval or readiness.",
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
    let bootstrap = preflight(finish_source(source)?.into_owned()?, false)?;
    OwnedSecretTerminal::check_captured_bootstrap(&bootstrap)?;
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
        // The file was replaced; what followed could not be verified.
        Err((_, crate::privacy_key::root::WRITTEN)) => {
            return Err(StartupError::WrittenThenChanged.into());
        }
        // Nothing was replaced, and the staging entry was left in place.
        // The rename was refused and the staging name was gone afterwards.
        Err((_, crate::privacy_key::root::UNCONFIRMED)) => {
            return Err(StartupError::RenameUnconfirmed.into());
        }
        Err((_, crate::privacy_key::root::STAGE_LEFT)) => {
            return Err(StartupError::StagedLeftover.into());
        }
        Err((crate::privacy_key::KeyError::Changed, _)) => {
            return Err(StartupError::Source.into());
        }
        Err((error, _)) => return Err(BootstrapError::Key(error).into()),
    }
    drop(source);
    // Read back: the actual user file now is the candidate and proposes it.
    // A file that reads differently was changed after this command's write.
    // Nothing is restored over it, and it is not reported as unchanged.
    let Ok(written) = actual_source() else {
        return Err(StartupError::WrittenThenChanged.into());
    };
    match written.recheck_original() {
        Ok((_, Some((bytes, _)))) if written.is_enabled() && bytes == candidate.as_slice() => {
            Ok(())
        }
        _ => Err(StartupError::WrittenThenChanged.into()),
    }
}

/// The explicit settings writer. Its invocation is the user's authorization
/// of this one change: it sets one named setting of the actual user file
/// that enables privacy, without a conversation. Every other byte of the
/// file is kept: the value of an existing member is replaced in place, and a
/// missing member is inserted after the opening brace. The candidate is
/// validated as a whole, and the same staged, identity-bound replacement as
/// `enable` commits it. Nothing is turned off, no policy, provider or key
/// member is touched, and no conversation is started. A running private
/// conversation changes its settings through its own terminal, which
/// carries the conversation over; one that is running elsewhere ends when
/// its captured file changes.
pub(crate) async fn set(key: &str, value: &str) -> std::result::Result<(), PrivateTerminalError> {
    // The settings this command knows, each with its own check of the value.
    let valid = match key {
        "permission_mode" => match crate::config::PermissionMode::parse(value) {
            Some(mode) if mode_asks_before_writes(mode) => true,
            Some(_) => return Err(StartupError::ModeDecidesWrites.into()),
            None => false,
        },
        "agent" => crate::agents::validate_agent_name(value).is_ok(),
        "model" => !value.is_empty() && value.len() <= 512 && !value.chars().any(char::is_control),
        _ => return Err(StartupError::SettingUnsupported.into()),
    };
    if !valid {
        return Err(StartupError::SettingValue.into());
    }
    let original = enabled_original()?;
    let candidate = edited(&original, &[(vec![key.to_owned()], value.into())])?;
    commit(&original, &candidate)
}

/// Whether a file write still waits for the person under this permission
/// mode. The gate of a private conversation lets the mode's own automatic
/// decision stand for a reviewed write, so a mode that decides writes
/// itself would write restored private values that nobody was shown. The
/// settings writers of this module take only the modes that ask.
pub(crate) fn mode_asks_before_writes(mode: crate::config::PermissionMode) -> bool {
    use crate::config::PermissionMode;
    matches!(mode, PermissionMode::Plan | PermissionMode::Manual)
}

/// The bytes of the actual user file, which must enable privacy. Read-only.
pub(crate) fn enabled_original() -> std::result::Result<Vec<u8>, PrivateTerminalError> {
    let source = actual_source()?;
    if !source.is_enabled() {
        return Err(StartupError::SettingNeedsPolicy.into());
    }
    let Ok((_, original)) = source.recheck_original() else {
        return Err(StartupError::Source.into());
    };
    match original {
        Some((original, _)) => Ok(original.to_vec()),
        None => Err(StartupError::NoUserFile.into()),
    }
}

/// `original` with each of `edits` applied: the value at a path of member
/// names is set to a JSON value. An existing value is replaced where it
/// stands; a missing member, with any missing objects above it, is inserted
/// after the opening brace of the nearest object that exists. Every other
/// byte is kept.
///
/// The whole candidate is validated before it is returned, and nothing is
/// written here: it differs from the original in exactly those places; the
/// policy member is untouched and is still the valid enabled one; and it
/// parses as the configuration a startup would take, under this build's
/// voice rule. A caller that knows what the edits mean checks that too.
pub(crate) fn edited(
    original: &[u8],
    edits: &[(Vec<String>, serde_json::Value)],
) -> std::result::Result<Vec<u8>, PrivateTerminalError> {
    use crate::privacy_policy::{PolicyError, reject_unavailable_user_source};
    let invalid = || PrivateTerminalError::Startup(StartupError::Invalid);
    let Ok(before) = serde_json::from_slice::<serde_json::Value>(original) else {
        return Err(invalid());
    };
    let mut expected = before.clone();
    let mut candidate = original.to_vec();
    for (path, value) in edits {
        // The same edit on the parsed value says what the bytes must mean.
        let mut at = &mut expected;
        for (depth, key) in path.iter().enumerate() {
            let Some(members) = at.as_object_mut() else {
                return Err(invalid());
            };
            if depth + 1 == path.len() {
                members.insert(key.clone(), value.clone());
                break;
            }
            at = members
                .entry(key.clone())
                .or_insert_with(|| serde_json::json!({}));
        }
        candidate = match with_value_at(&candidate, path, &value.to_string()) {
            Some(candidate) => candidate,
            None => return Err(invalid()),
        };
    }
    let Ok(after) = serde_json::from_slice::<serde_json::Value>(&candidate) else {
        return Err(invalid());
    };
    if edits.iter().any(|(path, _)| path.is_empty())
        || after != expected
        || after.get("privacy") != before.get("privacy")
        || reject_unavailable_user_source(&candidate) != Err(PolicyError::NotYetAvailable)
    {
        return Err(invalid());
    }
    match serde_json::from_slice::<crate::config::Config>(&candidate) {
        Ok(configuration) => require_voice_off(&configuration)?,
        Err(_) => return Err(invalid()),
    }
    Ok(candidate)
}

/// Replaces the actual user file, which must still be `original` and enable
/// privacy, with `candidate`, through the retained source's staged,
/// identity-bound replacement, and reads it back. The refusals say which
/// phase was reached: nothing replaced, a staging entry left, or written
/// and then not verifiable.
pub(crate) fn commit(
    original: &[u8],
    candidate: &[u8],
) -> std::result::Result<(), PrivateTerminalError> {
    let source = actual_source()?;
    if !source.is_enabled() {
        return Err(StartupError::SettingNeedsPolicy.into());
    }
    let Ok((root, held)) = source.recheck_original() else {
        return Err(StartupError::Source.into());
    };
    let Some((held, lease)) = held else {
        return Err(StartupError::NoUserFile.into());
    };
    if held != original {
        return Err(StartupError::Source.into());
    }
    match root.replace_user_config(lease, held, candidate) {
        Ok(()) => {}
        Err((_, crate::privacy_key::root::WRITTEN)) => {
            return Err(StartupError::WrittenThenChanged.into());
        }
        // The rename was refused and the staging name was gone afterwards.
        Err((_, crate::privacy_key::root::UNCONFIRMED)) => {
            return Err(StartupError::RenameUnconfirmed.into());
        }
        Err((_, crate::privacy_key::root::STAGE_LEFT)) => {
            return Err(StartupError::StagedLeftover.into());
        }
        Err((crate::privacy_key::KeyError::Changed, _)) => {
            return Err(StartupError::Source.into());
        }
        Err((error, _)) => return Err(BootstrapError::Key(error).into()),
    }
    drop(source);
    let Ok(written) = actual_source() else {
        return Err(StartupError::WrittenThenChanged.into());
    };
    match written.recheck_original() {
        Ok((_, Some((bytes, _)))) if written.is_enabled() && bytes == candidate => Ok(()),
        _ => Err(StartupError::WrittenThenChanged.into()),
    }
}

/// The startup of this same invocation, captured anew from the actual user
/// source for a conversation that is carried over: the router's own steps
/// up to the terminal, which is already entered. No conversation is
/// reserved; the carried one brings its successor's binding.
pub(crate) fn recapture()
-> std::result::Result<(PrivateBootstrap, super::CapturedApprovalStart), PrivateTerminalError> {
    let captured = match TerminalStartupCapture::capture_actual()? {
        TerminalStartupCapture::Captured(captured) => captured,
        TerminalStartupCapture::Legacy(_) => {
            return Err(PrivateTerminalError::CarryOver(
                "The user configuration no longer proposes privacy.".into(),
            ));
        }
    };
    let startup = captured.into_owned()?;
    let approval = startup.approval_start()?;
    let bootstrap = preflight(startup, false)?;
    OwnedSecretTerminal::check_captured_bootstrap(&bootstrap)?;
    let Some(requirements) = bootstrap.startup_requirements()? else {
        return Err(PrivateTerminalError::Startup(StartupError::Invocation));
    };
    requirements.select_normal_routing()?;
    Ok((bootstrap, approval))
}

/// `original` with the value at `path` set to the JSON text `encoded`. See
/// [`edited`]. `None` when the bytes are not an object along the path.
fn with_value_at(original: &[u8], path: &[String], encoded: &str) -> Option<Vec<u8>> {
    let name = |key: &str| serde_json::Value::from(key).to_string();
    let mut object = 0..original.len();
    for (depth, key) in path.iter().enumerate() {
        match member_value(&original[object.clone()], key)? {
            Some(span) => {
                let span = object.start + span.start..object.start + span.end;
                if depth + 1 == path.len() {
                    let mut candidate = Vec::with_capacity(original.len() + encoded.len());
                    candidate.extend_from_slice(&original[..span.start]);
                    candidate.extend_from_slice(encoded.as_bytes());
                    candidate.extend_from_slice(&original[span.end..]);
                    return Some(candidate);
                }
                object = span;
            }
            // Missing from here on: one new member that holds the rest of
            // the path.
            None => {
                let mut value = encoded.to_owned();
                for below in path[depth + 1..].iter().rev() {
                    value = format!("{{{}:{value}}}", name(below));
                }
                let member = format!("{}:{value}", name(key));
                let mut candidate = original[..object.start].to_vec();
                candidate.extend_from_slice(&inserted(&original[object.start..], &member)?);
                return Some(candidate);
            }
        }
    }
    None
}

/// Where the value of the member `key` stands in the bytes of one JSON
/// object. `Some(None)`: the object has no such member. `None`: the
/// bytes are not one object, or name the member more than once. Strings are
/// passed with their escapes and nested values with their brackets; nothing
/// is decoded but the member names. The caller validates the whole
/// candidate afterwards.
fn member_value(original: &[u8], key: &str) -> Option<Option<std::ops::Range<usize>>> {
    // The index after the string that starts at `at`.
    fn string_end(bytes: &[u8], at: usize) -> Option<usize> {
        let mut index = at + 1;
        loop {
            match bytes.get(index)? {
                b'"' => return Some(index + 1),
                b'\\' => index += 2,
                _ => index += 1,
            }
        }
    }
    fn skip_space(bytes: &[u8], mut at: usize) -> usize {
        while bytes.get(at).is_some_and(u8::is_ascii_whitespace) {
            at += 1;
        }
        at
    }
    let mut at = skip_space(original, 0);
    if original.get(at) != Some(&b'{') {
        return None;
    }
    at = skip_space(original, at + 1);
    let mut found = None;
    if original.get(at) == Some(&b'}') {
        return Some(None);
    }
    loop {
        if original.get(at) != Some(&b'"') {
            return None;
        }
        let name_end = string_end(original, at)?;
        let name: String = serde_json::from_slice(&original[at..name_end]).ok()?;
        at = skip_space(original, name_end);
        if original.get(at) != Some(&b':') {
            return None;
        }
        let start = skip_space(original, at + 1);
        // The end of this value: a string, a bracketed value, or a scalar.
        let mut end = start;
        let mut depth = 0usize;
        loop {
            match original.get(end)? {
                b'"' => end = string_end(original, end)?,
                b'{' | b'[' => {
                    depth += 1;
                    end += 1;
                }
                b'}' | b']' if depth > 0 => {
                    depth -= 1;
                    end += 1;
                }
                b',' | b'}' if depth == 0 => break,
                byte if depth == 0 && byte.is_ascii_whitespace() => break,
                _ => end += 1,
            }
            if depth == 0
                && original
                    .get(start)
                    .is_some_and(|first| matches!(first, b'"' | b'{' | b'['))
            {
                break;
            }
        }
        if end == start {
            return None;
        }
        if name == key {
            if found.is_some() {
                return None;
            }
            found = Some(start..end);
        }
        at = skip_space(original, end);
        match original.get(at)? {
            b',' => at = skip_space(original, at + 1),
            b'}' => return Some(found),
            _ => return None,
        }
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
    inserted(original, &format!("\"privacy\":{member}"))
}

/// `original` with the member text `member` inserted right after the opening
/// brace of its top-level object. Every original byte stays where it was.
fn inserted(original: &[u8], member: &str) -> Option<Vec<u8>> {
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
