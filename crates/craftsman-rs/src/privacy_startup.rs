//! Actual-user startup capture for the owned interactive terminal. The `route`
//! child is the one normal caller. A captured selection is a proposal: by
//! itself it activates nothing.
use crate::config::{Config, PermissionMode};
use crate::hitl::ApprovalState;
use crate::privacy_key::{ConfigRoot, KeyError};
use crate::privacy_policy::{
    CapturedPolicy, CapturedUserConfiguration, PolicyError, RetainedUserConfigurationSource,
    UserConfigurationCaptureError,
};
use clap::Parser;
use std::{
    fmt,
    io::IsTerminal,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};
mod route;
pub(crate) use route::{
    commit, edited, enable, enabled_original, mode_asks_before_writes, normal_route, recapture,
    set, status,
};

/// The value of `$result`, or for any error a return of `$error`. A plain
/// match where an adapter call with a closure stood: the same work in the
/// same order, without a function of its own for each place.
macro_rules! or_return {
    ($result:expr, $error:expr) => {
        match $result {
            Ok(value) => value,
            Err(_) => return Err($error),
        }
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum StartupError {
    #[error("The private terminal startup invocation is unsupported.")]
    Invocation,
    #[error("The captured user configuration is invalid.")]
    Invalid,
    #[error("The actual user configuration source is unavailable or changed.")]
    Source,
    #[error("Enabled privacy requires the owned fresh interactive terminal.")]
    Frontend,
    #[error("Enabled privacy does not support one-shot message input.")]
    OneShot,
    #[error("Enabled privacy cannot resume or transfer an existing conversation.")]
    Resume,
    #[error("Enabled privacy requires actual terminal input and output.")]
    Terminal,
    #[error("Secure private key ownership is unsupported on this platform.")]
    SecureStorageUnsupported,
    #[error("The private terminal requires the source-built detector feature.")]
    DetectorUnavailable,
    #[error("The private terminal cannot omit unsupported declared identifiers.")]
    IdentifiersUnavailable,
    #[error("Normal terminal privacy activation is not yet available.")]
    NotYetAvailable,
    #[error("The captured terminal approval source is unavailable or foreign.")]
    ApprovalSource,
    #[error(
        "Enabled privacy requires voice.enabled:false in the user configuration; voice input has no private adapter."
    )]
    Voice,
    #[error(
        "No mapping-vault key exists yet. Run `craftsman privacy init` first; nothing was unlocked."
    )]
    KeyMissing,
    #[error(
        "The mapping-vault key files are incomplete; nothing was unlocked, adopted or deleted."
    )]
    KeyIncomplete,
    #[error(
        "The installed detector gadget does not match the policy's pins or has no stored source approval; nothing was unlocked."
    )]
    Detector,
    #[error(
        "The user configuration already has a privacy policy; this command only adds one. Nothing was changed."
    )]
    PolicyPresent,
    #[error(
        "No user configuration file exists; configure a model connection first. Nothing was changed."
    )]
    NoUserFile,
    #[error(
        "The detector gadget is not installed under the user configuration or has no stored source approval. Nothing was changed."
    )]
    DetectorNotApproved,
    #[error(
        "The policy was written, but the user configuration could not be verified afterwards or then read differently. `craftsman privacy status` shows its present state."
    )]
    WrittenThenChanged,
    #[error(
        "The replacement of the user configuration was refused, and its staging name `config.json.privacy-enable` was gone afterwards: the file may have been replaced. `craftsman privacy status` shows its present state."
    )]
    RenameUnconfirmed,
    #[error(
        "The setting was written, but the user configuration could not be verified afterwards or then read differently. `craftsman privacy status` shows its present state."
    )]
    SettingWrittenThenChanged,
    #[error(
        "The user configuration was not replaced. The staging name `config.json.privacy-enable` could not be confirmed as the file this command created; that entry was left in place."
    )]
    StagedLeftover,
    #[error("This command sets `permission_mode`, `model` or `agent`. Nothing was changed.")]
    SettingUnsupported,
    #[error("The value is not valid for that setting. Nothing was changed.")]
    SettingValue,
    #[error(
        "This command changes a user file that enables privacy; without a policy the ordinary terminal changes settings. Nothing was changed."
    )]
    SettingNeedsPolicy,
    #[error(
        "That permission mode decides file writes without asking. With privacy enabled, a write that restores private values would then not be shown first. Use `plan` or `manual`. Nothing was changed."
    )]
    ModeDecidesWrites,
    #[error(
        "The permission mode of this start decides file writes without asking. With privacy enabled, a write that restores private values would then not be shown first. Start with `plan` or `manual`; nothing was unlocked."
    )]
    ModeDecidesWritesAtStart,
}
pub type Result<T> = std::result::Result<T, StartupError>;

/// This enum is issued from the actual user source only. A later project merge
/// of a Legacy Config cannot change it into an owned private selection.
pub enum TerminalStartupCapture {
    Legacy(Option<Config>),
    Captured(CapturedTerminalStartup),
}
impl fmt::Debug for TerminalStartupCapture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TerminalStartupCapture([private])")
    }
}

/// The SAME user root/file source remains in the policy and source owners;
/// configuration was parsed from the bounded original bytes held by `source`.
/// None has a caller-data constructor, Clone, Serialize or mutable getter.
pub struct CapturedTerminalStartup {
    configuration: Config,
    policy: CapturedPolicy,
    source: RetainedUserConfigurationSource,
    // Only the actual parsed invocation fills this. Root-only test/legacy
    // captures cannot stand in for a normal terminal's option origin.
    options: Option<CapturedTerminalOptions>,
}
impl fmt::Debug for CapturedTerminalStartup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CapturedTerminalStartup([private])")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartupPrerequisite {
    ReviewedNormalTerminalRouting,
    FreshConversationSharedAcrossAllLanes,
    FullRequestedFeaturesAndActualNativeViews,
    OwnedKeyAndApprovedDetectorBeforeGuestExecution,
    VoiceAndNativeInspectionIntegration,
    FinalSourceAndNativeValidation,
}

impl StartupPrerequisite {
    /// Review of the routing and final source and native validation are
    /// process evidence. They hold merge and install. A running startup can
    /// neither produce nor record them, and they are no part of its runtime
    /// eligibility.
    pub fn is_delivery_evidence(self) -> bool {
        matches!(
            self,
            Self::ReviewedNormalTerminalRouting | Self::FinalSourceAndNativeValidation
        )
    }
}

/// Obligations retained through consuming access; these are not host-policy
/// permissions, and this shared object never says that a session is ready.
/// It keeps one observation about the source and the invocation: that the
/// actual router selected this startup. It is shared by every conversation of
/// the startup, including the ones a fresh clear builds, so nothing about one
/// built session is stored here. Readiness of a build belongs to that actual
/// bound session and is checked where it issues a turn
/// (`PrivateBuiltSession::task_owner`).
pub struct StartupRequirements {
    pending: [StartupPrerequisite; 6],
    options: Option<CapturedTerminalOptions>,
    // Set once, by the router, after its read-only preflight of this source
    // and invocation: the policy is enabled, voice is explicitly off (so none
    // of the unimplemented voice integration is requested) and key files
    // exist. An observation, not a prerequisite label.
    selected: AtomicBool,
}
impl StartupRequirements {
    fn pending() -> Self {
        Self {
            pending: [
                StartupPrerequisite::ReviewedNormalTerminalRouting,
                StartupPrerequisite::FreshConversationSharedAcrossAllLanes,
                StartupPrerequisite::FullRequestedFeaturesAndActualNativeViews,
                StartupPrerequisite::OwnedKeyAndApprovedDetectorBeforeGuestExecution,
                StartupPrerequisite::VoiceAndNativeInspectionIntegration,
                StartupPrerequisite::FinalSourceAndNativeValidation,
            ],
            options: None,
            selected: AtomicBool::new(false),
        }
    }
    pub fn prerequisites(&self) -> &[StartupPrerequisite] {
        &self.pending
    }
    /// Always refuses. These retained obligations alone activate nothing: a
    /// routed startup submits model turns and effects only through its own
    /// bound session, which checks that binding for every submission.
    pub fn require_normal_activation(&self) -> Result<()> {
        Err(StartupError::NotYetAvailable)
    }
    /// Private to this module and its router. A startup without the actual
    /// parsed invocation's options cannot be selected.
    fn select_normal_routing(&self) -> Result<()> {
        if self.options.is_none() {
            return Err(StartupError::Invocation);
        }
        self.selected.store(true, Ordering::SeqCst);
        Ok(())
    }
    /// Whether the actual router selected this startup.
    #[cfg(unix)]
    pub(crate) fn normal_routing_selected(&self) -> bool {
        self.selected.load(Ordering::SeqCst)
    }
    pub fn terminal_options(&self) -> Option<&CapturedTerminalOptions> {
        self.options.as_ref()
    }
}
impl fmt::Debug for StartupRequirements {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PrivateStartupRequirements([pending])")
    }
}

/// Actual parsed terminal input, not user policy, source approval, model
/// compatibility or readiness. No public from-flags/Config/state constructor.
pub struct CapturedTerminalOptions {
    permission_mode: Option<PermissionMode>,
    auto_approve: bool,
    mouse_capture: bool,
    environment_shell_allow_list: Vec<String>,
    identity: Arc<()>,
    approval: OnceLock<Arc<Mutex<ApprovalState>>>,
}
impl fmt::Debug for CapturedTerminalOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CapturedTerminalOptions([private])")
    }
}
impl CapturedTerminalOptions {
    fn capture(cli: &crate::Cli) -> Self {
        // Reuse the original host source exactly: missing/non-Unicode means no
        // entries, comma-separated values are trimmed and empty entries ignored.
        let environment_shell_allow_list = ApprovalState::default()
            .with_env_allow_list()
            .shell_allow_list;
        Self {
            permission_mode: cli
                .permission_mode
                .as_deref()
                .and_then(PermissionMode::parse),
            auto_approve: cli.auto_approve,
            mouse_capture: cli.mouse || !cli.no_mouse,
            environment_shell_allow_list,
            identity: Arc::new(()),
            approval: OnceLock::new(),
        }
    }
    pub fn permission_mode_override(&self) -> Option<PermissionMode> {
        self.permission_mode
    }
    pub fn auto_approve_override(&self) -> bool {
        self.auto_approve
    }
    pub fn mouse_capture(&self) -> bool {
        self.mouse_capture
    }
    fn approval_start(&self, cfg: &Config) -> CapturedApprovalStart {
        let state = self.approval.get_or_init(|| {
            // SAME normal precedence, derived without changing the original
            // captured Config. Auto wins even over an explicit Plan override.
            let auto = self.auto_approve || cfg.auto_approve;
            let mut shell_allow_list = cfg.shell_allow_list.clone();
            shell_allow_list.extend(self.environment_shell_allow_list.iter().cloned());
            Arc::new(Mutex::new(ApprovalState {
                mode: if auto {
                    PermissionMode::BypassPermissions
                } else {
                    self.permission_mode.unwrap_or(cfg.permission_mode)
                },
                auto_approve: auto,
                auto_approve_network: auto || cfg.auto_approve_web(),
                shell_allow_list,
            }))
        });
        CapturedApprovalStart {
            identity: self.identity.clone(),
            state: state.clone(),
        }
    }
}

/// One actual startup's live state. Copying equal ApprovalState values cannot
/// construct this owner or bind them to another startup/requirements object.
pub struct CapturedApprovalStart {
    identity: Arc<()>,
    state: Arc<Mutex<ApprovalState>>,
}
impl fmt::Debug for CapturedApprovalStart {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CapturedApprovalStart([private])")
    }
}
impl CapturedApprovalStart {
    /// Caller must first recheck the actual bootstrap/conversation source, then
    /// pass its retained requirements. This is a binding/refusal check only.
    pub fn state_for(
        &self,
        requirements: &StartupRequirements,
    ) -> Result<Arc<Mutex<ApprovalState>>> {
        let options = requirements
            .options
            .as_ref()
            .ok_or(StartupError::ApprovalSource)?;
        let state = options.approval.get().ok_or(StartupError::ApprovalSource)?;
        if !Arc::ptr_eq(&self.identity, &options.identity) || !Arc::ptr_eq(&self.state, state) {
            return Err(StartupError::ApprovalSource);
        }
        if self.state.is_poisoned() {
            return Err(StartupError::ApprovalSource);
        }
        Ok(self.state.clone())
    }
}

pub struct OwnedTerminalStartup {
    configuration: Config,
    policy: CapturedPolicy,
    source: RetainedUserConfigurationSource,
    requirements: StartupRequirements,
}
impl fmt::Debug for OwnedTerminalStartup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("OwnedTerminalStartup([private])")
    }
}

impl CapturedTerminalStartup {
    pub fn recheck(&self) -> Result<()> {
        self.source.recheck().map_err(policy_error)?;
        self.policy.recheck().map_err(policy_error)?;
        self.source.recheck().map_err(policy_error)
    }
    pub fn configuration(&self) -> Result<&Config> {
        self.recheck()?;
        Ok(&self.configuration)
    }
    pub fn policy(&self) -> Result<&CapturedPolicy> {
        self.recheck()?;
        Ok(&self.policy)
    }
    pub fn into_owned(self) -> Result<OwnedTerminalStartup> {
        self.recheck()?;
        let mut requirements = StartupRequirements::pending();
        requirements.options = self.options;
        Ok(OwnedTerminalStartup {
            configuration: self.configuration,
            policy: self.policy,
            source: self.source,
            requirements,
        })
    }
}
impl OwnedTerminalStartup {
    pub fn configuration(&self) -> Result<&Config> {
        self.source.recheck().map_err(policy_error)?;
        self.policy.recheck().map_err(policy_error)?;
        self.source.recheck().map_err(policy_error)?;
        Ok(&self.configuration)
    }
    pub fn requirements(&self) -> &StartupRequirements {
        &self.requirements
    }
    /// Issue the same cached live state from this original source and actual
    /// invocation only, before the existing consuming four-owner handoff.
    pub fn approval_start(&self) -> Result<CapturedApprovalStart> {
        self.source.recheck().map_err(policy_error)?;
        self.policy.recheck().map_err(policy_error)?;
        let options = self
            .requirements
            .options
            .as_ref()
            .ok_or(StartupError::ApprovalSource)?;
        let approval = options.approval_start(&self.configuration);
        self.source.recheck().map_err(policy_error)?;
        Ok(approval)
    }
    pub fn into_configuration_policy(
        self,
    ) -> Result<(
        Config,
        CapturedPolicy,
        RetainedUserConfigurationSource,
        StartupRequirements,
    )> {
        self.source.recheck().map_err(policy_error)?;
        self.policy.recheck().map_err(policy_error)?;
        self.source.recheck().map_err(policy_error)?;
        Ok((
            self.configuration,
            self.policy,
            self.source,
            self.requirements,
        ))
    }
}

impl TerminalStartupCapture {
    /// Actual argv is parsed without a new selector or command. Normal main
    /// passes its own parsed invocation to the router instead of calling this.
    pub fn capture_actual() -> Result<Self> {
        let cli = or_return!(crate::Cli::try_parse(), StartupError::Invocation);
        Self::capture_for_parsed_cli(&cli)
    }
    pub(crate) fn capture_for_parsed_cli(cli: &crate::Cli) -> Result<Self> {
        let controls = ActualTerminalControls::capture(cli);
        match capture_user_source(&UserSourceOrigin::Actual)? {
            SourceSelection::Legacy(configuration) => Ok(Self::Legacy(configuration)),
            SourceSelection::Proposed(source) => {
                controls.refuse_unsupported(&source)?;
                let options = CapturedTerminalOptions::capture(cli);
                let mut captured = finish_source(source)?;
                captured.options = Some(options);
                Ok(Self::Captured(captured))
            }
        }
    }
}

fn policy_error(error: PolicyError) -> StartupError {
    match error {
        PolicyError::Invalid => StartupError::Invalid,
        PolicyError::Source => StartupError::Source,
        PolicyError::NotYetAvailable => StartupError::NotYetAvailable,
    }
}

struct ActualTerminalControls {
    other_frontend: bool,
    one_shot: bool,
    resume: bool,
    terminal: bool,
}
impl ActualTerminalControls {
    fn capture(cli: &crate::Cli) -> Self {
        Self {
            other_frontend: cli.command.is_some(),
            one_shot: cli.message.is_some(),
            resume: cli.session.is_some(),
            terminal: std::io::stdin().is_terminal() && std::io::stdout().is_terminal(),
        }
    }
    fn refuse_unsupported(&self, source: &CapturedUserConfiguration) -> Result<()> {
        if self.other_frontend {
            return Err(StartupError::Frontend);
        }
        if self.one_shot {
            return Err(StartupError::OneShot);
        }
        if self.resume {
            return Err(StartupError::Resume);
        }
        if !self.terminal {
            return Err(StartupError::Terminal);
        }
        #[cfg(not(feature = "gadgets"))]
        {
            let _ = source;
            Err(StartupError::DetectorUnavailable)
        }
        #[cfg(feature = "gadgets")]
        {
            if source.has_identifiers() {
                return Err(StartupError::IdentifiersUnavailable);
            }
            Ok(())
        }
    }
}

enum UserSourceOrigin {
    Actual,
    #[cfg(all(test, unix))]
    OwnedFixture(std::path::PathBuf),
}
enum LegacyPresence {
    Any,
    ObservedFile,
}
impl UserSourceOrigin {
    fn capture_root(&self) -> crate::privacy_key::Result<ConfigRoot> {
        match self {
            Self::Actual => ConfigRoot::capture_actual(),
            #[cfg(all(test, unix))]
            Self::OwnedFixture(path) => ConfigRoot::capture_owned_startup_fixture(path.clone()),
        }
    }
    fn legacy_only(
        &self,
        unavailable: StartupError,
        presence: LegacyPresence,
    ) -> Result<SourceSelection> {
        let loaded = match self {
            Self::Actual => Config::load(),
            #[cfg(all(test, unix))]
            Self::OwnedFixture(path) => {
                Config::load_owned_startup_fixture(&path.join("config.json"))
            }
        };
        // The unchanged legacy loader already rejects enabled or malformed
        // actual user policy. It can never turn these errors into privacy-off.
        let configuration = loaded.map_err(|error| {
            if error.downcast_ref::<PolicyError>() == Some(&PolicyError::NotYetAvailable) {
                unavailable
            } else if error.downcast_ref::<PolicyError>() == Some(&PolicyError::Invalid) {
                StartupError::Invalid
            } else {
                StartupError::Source
            }
        })?;
        if matches!(presence, LegacyPresence::ObservedFile) && configuration.is_none() {
            return Err(StartupError::Source);
        }
        Ok(SourceSelection::Legacy(configuration))
    }
}

enum SourceSelection {
    Legacy(Option<Config>),
    Proposed(CapturedUserConfiguration),
}
fn capture_user_source(origin: &UserSourceOrigin) -> Result<SourceSelection> {
    let root = match origin.capture_root() {
        Ok(root) => root,
        Err(KeyError::Changed) => return Err(StartupError::Source),
        Err(KeyError::SecureStorageUnsupported) => {
            return origin.legacy_only(StartupError::SecureStorageUnsupported, LegacyPresence::Any);
        }
        Err(_) => return origin.legacy_only(StartupError::Source, LegacyPresence::Any),
    };
    let source = match CapturedUserConfiguration::capture(root) {
        Ok(source) => source,
        Err(UserConfigurationCaptureError::BeforeSource(KeyError::Changed | KeyError::Io)) => {
            return Err(StartupError::Source);
        }
        Err(UserConfigurationCaptureError::BeforeSource(_)) => {
            return origin.legacy_only(StartupError::Source, LegacyPresence::ObservedFile);
        }
        Err(UserConfigurationCaptureError::Invalid) => return Err(StartupError::Invalid),
        Err(UserConfigurationCaptureError::Refused(error)) => return Err(policy_error(error)),
    };
    if source.is_enabled() {
        Ok(SourceSelection::Proposed(source))
    } else {
        let configuration = source.into_legacy_configuration().map_err(policy_error)?;
        Ok(SourceSelection::Legacy(configuration))
    }
}

fn finish_source(source: CapturedUserConfiguration) -> Result<CapturedTerminalStartup> {
    let (configuration, policy, retained_source) = source
        .into_configuration_and_policy()
        .map_err(policy_error)?;
    Ok(CapturedTerminalStartup {
        configuration: configuration.ok_or(StartupError::Invalid)?,
        policy,
        source: retained_source,
        options: None,
    })
}

#[cfg(all(test, unix))]
mod options_tests;

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::path::PathBuf;

    struct OwnedUserFile {
        _temporary: tempfile::TempDir,
        root: PathBuf,
    }
    impl OwnedUserFile {
        fn new() -> Self {
            let temporary = tempfile::tempdir().expect("isolated fixture directory");
            let root = temporary
                .path()
                .canonicalize()
                .expect("canonical fixture parent")
                .join("user");
            fs::create_dir(&root).expect("isolated user directory");
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700))
                .expect("private fixture mode");
            Self {
                _temporary: temporary,
                root,
            }
        }
        fn write(&self, bytes: &[u8]) {
            fs::write(self.root.join("config.json"), bytes).expect("synthetic user source");
            fs::set_permissions(
                self.root.join("config.json"),
                fs::Permissions::from_mode(0o600),
            )
            .expect("synthetic source mode");
        }
        fn origin(&self) -> UserSourceOrigin {
            UserSourceOrigin::OwnedFixture(self.root.clone())
        }
        fn captured(&self) -> CapturedTerminalStartup {
            match capture_user_source(&self.origin()).expect("owned user source capture") {
                SourceSelection::Proposed(source) => {
                    finish_source(source).expect("captured policy completion")
                }
                SourceSelection::Legacy(_) => panic!("expected actual user proposal"),
            }
        }
        fn no_owned_effects(&self, before: &[u8]) {
            let after = fs::read(self.root.join("config.json")).expect("unchanged user source");
            assert!(after == before, "capture changed the user source");
            let entries = fs::read_dir(&self.root)
                .expect("fixture directory contents")
                .map(|entry| entry.expect("fixture directory entry").file_name())
                .collect::<Vec<_>>();
            assert!(
                entries.len() == 1 && entries[0] == "config.json",
                "capture created an additional user file"
            );
        }
    }

    fn enabled_user_source() -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "model": "synthetic-local-model",
            "api_key": "${UNSELECTED_STARTUP_CREDENTIAL}",
            "unknown_setting": {"keep": [1, 2, 3]},
            "privacy": {
                "enabled": true,
                "detector": {
                    "gadget": "synthetic-detector",
                    "function": "detect",
                    "component_sha256": "ab".repeat(32),
                    "manifest_sha256": "cd".repeat(32)
                },
                "identifiers": [],
                "file_data": []
            }
        }))
        .expect("synthetic source encoding")
    }

    #[test]
    fn privacy_startup_actual_user_source_owns_config_policy_and_consumed_requirements() {
        let fixture = OwnedUserFile::new();
        let bytes = enabled_user_source();
        fixture.write(&bytes);
        let captured = fixture.captured();
        let configuration = captured.configuration().expect("captured configuration");
        assert!(configuration.model.as_deref() == Some("synthetic-local-model"));
        assert!(configuration.api_key == "${UNSELECTED_STARTUP_CREDENTIAL}");
        assert!(configuration.extra["unknown_setting"]["keep"] == serde_json::json!([1, 2, 3]));
        let detector = captured
            .policy()
            .expect("captured policy")
            .proposed_detector()
            .expect("unchanged source")
            .expect("actual user proposal");
        assert!(detector.gadget() == "synthetic-detector");
        assert!(detector.component_pin() == &[0xab; 32]);
        assert!(detector.manifest_pin() == &[0xcd; 32]);
        let owned = captured.into_owned().expect("consuming source ownership");
        assert!(matches!(
            owned.requirements().require_normal_activation(),
            Err(StartupError::NotYetAvailable)
        ));
        let (configuration, policy, source, requirements) = owned
            .into_configuration_policy()
            .expect("same configuration and policy owners");
        assert!(configuration.api_key == "${UNSELECTED_STARTUP_CREDENTIAL}");
        assert!(policy.proposed_detector().expect("held source").is_some());
        assert!(source.recheck().is_ok());
        assert!(
            requirements
                .prerequisites()
                .contains(&StartupPrerequisite::FullRequestedFeaturesAndActualNativeViews)
        );
        assert!(requirements.require_normal_activation().is_err());
        fixture.no_owned_effects(&bytes);
    }

    #[test]
    fn privacy_startup_project_and_merged_config_cannot_select_actual_user_policy() {
        let fixture = OwnedUserFile::new();
        let project = fixture
            ._temporary
            .path()
            .canonicalize()
            .expect("fixture project parent")
            .join("project/.craftsman");
        fs::create_dir_all(&project).expect("synthetic project directory");
        fs::write(project.join("config.json"), enabled_user_source())
            .expect("untrusted project source");
        assert!(matches!(
            capture_user_source(&fixture.origin()),
            Ok(SourceSelection::Legacy(None))
        ));
        let bytes = br#"{"model":"synthetic-local-model","privacy":{"enabled":false}}"#;
        fixture.write(bytes);
        let mut merged = match capture_user_source(&fixture.origin()).expect("disabled user source")
        {
            SourceSelection::Legacy(Some(configuration)) => configuration,
            _ => panic!("disabled user source did not remain legacy"),
        };
        let caller: Config =
            serde_json::from_slice(&enabled_user_source()).expect("synthetic caller configuration");
        merged
            .extra
            .insert("privacy".into(), caller.extra["privacy"].clone());
        assert!(merged.extra["privacy"]["enabled"] == true);
        match capture_user_source(&fixture.origin()).expect("actual source remains unchanged") {
            SourceSelection::Legacy(Some(configuration)) => {
                assert!(configuration.extra["privacy"]["enabled"] == false);
            }
            _ => panic!("caller or project data selected privacy"),
        }
        fixture.no_owned_effects(bytes);
    }

    #[test]
    fn privacy_startup_absent_and_disabled_legacy_files_preserve_loader_behavior() {
        let fixture = OwnedUserFile::new();
        let missing = fixture.root.join("missing-user");
        assert!(matches!(
            capture_user_source(&UserSourceOrigin::OwnedFixture(missing.clone())),
            Ok(SourceSelection::Legacy(None))
        ));
        assert!(!missing.exists(), "capture created an absent user root");
        let bytes = br#"{"api_key":"${UNSELECTED_STARTUP_CREDENTIAL}","model":"fixture-model","privacy":{"enabled":false},"legacy_unknown":{"keep":7}}"#;
        fixture.write(bytes);
        // This is a valid legacy directory but not a private-key directory.
        fs::set_permissions(&fixture.root, fs::Permissions::from_mode(0o755))
            .expect("legacy fixture directory mode");
        let original = Config::load_owned_startup_fixture(&fixture.root.join("config.json"))
            .expect("same original loader")
            .expect("legacy user configuration");
        let captured = match capture_user_source(&fixture.origin()).expect("legacy passthrough") {
            SourceSelection::Legacy(Some(configuration)) => configuration,
            _ => panic!("disabled configuration changed startup mode"),
        };
        assert!(
            serde_json::to_value(original).expect("original configuration value")
                == serde_json::to_value(&captured).expect("captured configuration value")
        );
        assert!(captured.api_key == "${UNSELECTED_STARTUP_CREDENTIAL}");
        fixture.no_owned_effects(bytes);
        fixture.write(&enabled_user_source());
        assert!(matches!(
            capture_user_source(&fixture.origin()),
            Err(StartupError::Source)
        ));
    }

    #[test]
    fn privacy_startup_legacy_size_remains_distinct_from_selected_source_bounds() {
        let fixture = OwnedUserFile::new();
        let bytes = serde_json::to_vec(&serde_json::json!({
            "unknown_large_legacy_field": "x".repeat(crate::privacy_key::root::CONFIG_LIMIT + 1)
        }))
        .expect("large legacy source encoding");
        fixture.write(&bytes);
        let original = Config::load_owned_startup_fixture(&fixture.root.join("config.json"))
            .expect("uncapped absent-policy legacy parse")
            .expect("existing legacy configuration");
        let configuration =
            match capture_user_source(&fixture.origin()).expect("legacy size behavior") {
                SourceSelection::Legacy(Some(configuration)) => configuration,
                _ => panic!("legacy field selected privacy"),
            };
        assert!(
            serde_json::to_value(original).expect("original configuration")
                == serde_json::to_value(configuration).expect("preserved configuration")
        );
        fixture.no_owned_effects(&bytes);
        let bytes = serde_json::to_vec(&serde_json::json!({
            "privacy": {"enabled": false},
            "unknown_large_legacy_field": "x".repeat(crate::privacy_key::root::CONFIG_LIMIT + 1)
        }))
        .expect("bounded-policy source encoding");
        fixture.write(&bytes);
        assert!(Config::load_owned_startup_fixture(&fixture.root.join("config.json")).is_err());
        assert!(matches!(
            capture_user_source(&fixture.origin()),
            Err(StartupError::Invalid)
        ));
        fixture.no_owned_effects(&bytes);
    }

    #[test]
    fn privacy_startup_disabled_legacy_links_remain_legacy_without_source_authority() {
        let fixture = OwnedUserFile::new();
        let bytes = br#"{"privacy":{"enabled":false},"unrelated_user_field":[1,2,3]}"#;
        let target = fixture._temporary.path().join("legacy-source.json");
        fs::write(&target, bytes).expect("synthetic legacy target");
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600))
            .expect("synthetic legacy target mode");
        let named = fixture.root.join("config.json");
        symlink(&target, &named).expect("legacy symlink");
        for hardlink in [false, true] {
            if hardlink {
                fs::remove_file(&named).expect("remove only synthetic fixture symlink");
                fs::hard_link(&target, &named).expect("legacy second hard link");
            }
            let original = Config::load_owned_startup_fixture(&named)
                .expect("original legacy link behavior")
                .expect("legacy linked source");
            let captured =
                match capture_user_source(&fixture.origin()).expect("legacy link capture") {
                    SourceSelection::Legacy(Some(configuration)) => configuration,
                    _ => panic!("legacy linked source minted private authority"),
                };
            assert!(
                serde_json::to_value(original).expect("original legacy configuration")
                    == serde_json::to_value(captured).expect("captured legacy configuration")
            );
            fixture.no_owned_effects(bytes);
        }
    }

    #[test]
    fn privacy_startup_malformed_actual_policy_never_becomes_legacy() {
        let cases: &[&[u8]] = &[
            br#"{"privacy":{"enabled":"yes"}}"#,
            br#"{"privacy":{"enabled":true}}"#,
            br#"{"privacy":{"enabled":false,"caller_grant":true}}"#,
            br#"{"privacy":{"enabled":false},"privacy":{"enabled":false}}"#,
            br#"{"privacy":{"enabled":false},"model":[]}"#,
            b"\xff",
        ];
        for bytes in cases {
            let fixture = OwnedUserFile::new();
            fixture.write(bytes);
            assert!(matches!(
                capture_user_source(&fixture.origin()),
                Err(StartupError::Invalid)
            ));
            fixture.no_owned_effects(bytes);
        }
    }

    #[test]
    fn privacy_startup_replaced_and_mutated_user_source_refuses_before_access() {
        let fixture = OwnedUserFile::new();
        let bytes = enabled_user_source();
        fixture.write(&bytes);
        let captured = fixture.captured();
        let replacement = fixture.root.join("replacement.json");
        fs::write(&replacement, &bytes).expect("synthetic replacement source");
        fs::set_permissions(&replacement, fs::Permissions::from_mode(0o600))
            .expect("replacement source mode");
        fs::rename(&replacement, fixture.root.join("config.json"))
            .expect("actual source replacement");
        assert!(matches!(
            captured.configuration(),
            Err(StartupError::Source)
        ));
        assert!(matches!(captured.policy(), Err(StartupError::Source)));
        assert!(matches!(captured.into_owned(), Err(StartupError::Source)));
        fixture.no_owned_effects(&bytes);
        let captured = fixture.captured();
        let changed = bytes
            .iter()
            .map(|byte| if *byte == b'a' { b'b' } else { *byte })
            .collect::<Vec<_>>();
        fixture.write(&changed);
        assert!(matches!(
            captured.configuration(),
            Err(StartupError::Source)
        ));
        assert!(matches!(captured.into_owned(), Err(StartupError::Source)));
        fixture.no_owned_effects(&changed);
    }

    #[test]
    fn privacy_startup_held_root_identity_and_link_rules_cannot_be_rebound() {
        let fixture = OwnedUserFile::new();
        let bytes = enabled_user_source();
        fixture.write(&bytes);
        let captured = fixture.captured();
        let old = fixture.root.with_file_name("old-user");
        fs::rename(&fixture.root, &old).expect("retained root rename");
        fs::create_dir(&fixture.root).expect("synthetic replacement root");
        fs::set_permissions(&fixture.root, fs::Permissions::from_mode(0o700))
            .expect("replacement private mode");
        fixture.write(&bytes);
        assert!(matches!(
            captured.configuration(),
            Err(StartupError::Source)
        ));
        assert!(matches!(captured.into_owned(), Err(StartupError::Source)));
        fixture.no_owned_effects(&bytes);

        let hardlinked = OwnedUserFile::new();
        hardlinked.write(&bytes);
        let other = hardlinked._temporary.path().join("other-source.json");
        fs::hard_link(hardlinked.root.join("config.json"), &other)
            .expect("synthetic second hard link");
        assert!(matches!(
            capture_user_source(&hardlinked.origin()),
            Err(StartupError::Source)
        ));
        hardlinked.no_owned_effects(&bytes);

        let linked = OwnedUserFile::new();
        let other = linked._temporary.path().join("other-source.json");
        fs::write(&other, &bytes).expect("synthetic symlink target");
        fs::set_permissions(&other, fs::Permissions::from_mode(0o600))
            .expect("synthetic target mode");
        symlink(&other, linked.root.join("config.json")).expect("synthetic user symlink");
        assert!(matches!(
            capture_user_source(&linked.origin()),
            Err(StartupError::Source)
        ));
        linked.no_owned_effects(&bytes);
    }

    #[test]
    fn privacy_startup_fixed_debug_and_errors_do_not_render_captured_config() {
        let fixture = OwnedUserFile::new();
        let bytes = enabled_user_source();
        fixture.write(&bytes);
        let captured = fixture.captured();
        let rendered = format!("{captured:?}");
        assert!(!rendered.contains("synthetic-local-model"));
        assert!(!rendered.contains("UNSELECTED_STARTUP_CREDENTIAL"));
        assert!(!rendered.contains("synthetic-detector"));
        let owned = captured.into_owned().expect("captured source ownership");
        let rendered = format!("{owned:?} {:?}", owned.requirements());
        assert!(!rendered.contains("UNSELECTED_STARTUP_CREDENTIAL"));
        assert!(!rendered.contains("synthetic-detector"));
        let (configuration, policy, source, requirements) = owned
            .into_configuration_policy()
            .expect("same retained configuration");
        assert!(configuration.api_key == "${UNSELECTED_STARTUP_CREDENTIAL}");
        assert!(source.recheck().is_ok());
        let rendered = format!(
            "{policy:?} {source:?} {requirements:?} {}",
            StartupError::Source
        );
        assert!(!rendered.contains("UNSELECTED_STARTUP_CREDENTIAL"));
        assert!(!rendered.contains("synthetic-detector"));
        fixture.no_owned_effects(&bytes);
    }

    #[test]
    fn consuming_private_bootstrap_retains_actual_configuration_and_pending_requirements() {
        let fixture = OwnedUserFile::new();
        let bytes = enabled_user_source();
        fixture.write(&bytes);
        let workspace = fixture._temporary.path().canonicalize().unwrap();
        let bootstrap = fixture
            .captured()
            .into_owned()
            .unwrap()
            .into_private_bootstrap(&workspace)
            .unwrap();
        let configuration = bootstrap.captured_configuration().unwrap().unwrap();
        assert!(configuration.model.as_deref() == Some("synthetic-local-model"));
        assert!(configuration.api_key == "${UNSELECTED_STARTUP_CREDENTIAL}");
        assert!(configuration.extra["unknown_setting"]["keep"] == serde_json::json!([1, 2, 3]));
        let requirements = bootstrap.startup_requirements().unwrap().unwrap();
        for prerequisite in [
            StartupPrerequisite::ReviewedNormalTerminalRouting,
            StartupPrerequisite::FreshConversationSharedAcrossAllLanes,
            StartupPrerequisite::FullRequestedFeaturesAndActualNativeViews,
            StartupPrerequisite::OwnedKeyAndApprovedDetectorBeforeGuestExecution,
            StartupPrerequisite::VoiceAndNativeInspectionIntegration,
            StartupPrerequisite::FinalSourceAndNativeValidation,
        ] {
            assert!(requirements.prerequisites().contains(&prerequisite));
        }
        assert!(matches!(
            requirements.require_normal_activation(),
            Err(StartupError::NotYetAvailable)
        ));
        assert!(matches!(
            bootstrap.status(),
            Ok(crate::privacy_cli::FilePresence::Missing)
        ));
        let debug = format!("{bootstrap:?}");
        assert!(!debug.contains("UNSELECTED_STARTUP_CREDENTIAL"));
        assert!(!debug.contains("synthetic-local-model"));
        fixture.no_owned_effects(&bytes);
    }

    #[test]
    fn consuming_private_bootstrap_refuses_replaced_source_before_any_new_capture() {
        let fixture = OwnedUserFile::new();
        let bytes = enabled_user_source();
        fixture.write(&bytes);
        let owned = fixture.captured().into_owned().unwrap();
        let replacement = fixture.root.join("replacement.json");
        fs::write(&replacement, &bytes).unwrap();
        fs::set_permissions(&replacement, fs::Permissions::from_mode(0o600)).unwrap();
        fs::rename(&replacement, fixture.root.join("config.json")).unwrap();
        let workspace = fixture._temporary.path().canonicalize().unwrap();
        assert!(matches!(
            owned.into_private_bootstrap(&workspace),
            Err(crate::privacy_bootstrap::BootstrapError::ConfigurationChanged)
        ));
        fixture.no_owned_effects(&bytes);
    }

    #[test]
    fn consuming_private_bootstrap_rechecks_retained_source_before_configuration_and_key_status() {
        let fixture = OwnedUserFile::new();
        let bytes = enabled_user_source();
        fixture.write(&bytes);
        let workspace = fixture._temporary.path().canonicalize().unwrap();
        let bootstrap = fixture
            .captured()
            .into_owned()
            .unwrap()
            .into_private_bootstrap(&workspace)
            .unwrap();
        let changed = bytes
            .iter()
            .map(|byte| if *byte == b'a' { b'b' } else { *byte })
            .collect::<Vec<_>>();
        fixture.write(&changed);
        assert!(bootstrap.captured_configuration().is_err());
        assert!(bootstrap.startup_requirements().is_err());
        assert!(bootstrap.status().is_err());
        fixture.no_owned_effects(&changed);
    }

    #[test]
    fn captured_frontend_preflight_uses_actual_voice_source_before_key_work() {
        for voice in [
            None,
            Some(serde_json::json!(true)),
            Some(serde_json::json!("false")),
            Some(serde_json::json!(false)),
        ] {
            let fixture = OwnedUserFile::new();
            let mut value: serde_json::Value =
                serde_json::from_slice(&enabled_user_source()).unwrap();
            if let Some(voice) = &voice {
                value["voice"] = serde_json::json!({"enabled": voice});
            }
            let bytes = serde_json::to_vec(&value).unwrap();
            fixture.write(&bytes);
            let workspace = fixture._temporary.path().canonicalize().unwrap();
            let bootstrap = fixture
                .captured()
                .into_owned()
                .unwrap()
                .into_private_bootstrap(&workspace)
                .unwrap();
            let result =
                crate::privacy_unlock::OwnedSecretTerminal::check_captured_bootstrap(&bootstrap);
            if voice == Some(serde_json::json!(false)) {
                assert!(result.is_ok());
            } else {
                assert!(matches!(
                    result,
                    Err(crate::privacy_unlock::PrivateTerminalError::Build(
                        crate::privacy_bootstrap::PrivateBuildError::Refused(
                            crate::privacy_session::BuildRefusal::RequestedPart
                        )
                    ))
                ));
            }
            assert!(matches!(
                bootstrap.status(),
                Ok(crate::privacy_cli::FilePresence::Missing)
            ));
            fixture.no_owned_effects(&bytes);
        }
    }

    #[test]
    fn captured_frontend_preflight_refuses_missing_or_replaced_startup_ownership() {
        let fixture = OwnedUserFile::new();
        let mut value: serde_json::Value = serde_json::from_slice(&enabled_user_source()).unwrap();
        value["voice"] = serde_json::json!({"enabled":false});
        let bytes = serde_json::to_vec(&value).unwrap();
        fixture.write(&bytes);
        let workspace = fixture._temporary.path().canonicalize().unwrap();
        let legacy = crate::privacy_bootstrap::PrivateBootstrap::from_captured_root(
            ConfigRoot::capture_owned_startup_fixture(fixture.root.clone()).unwrap(),
            &workspace,
        )
        .unwrap();
        assert!(matches!(
            crate::privacy_unlock::OwnedSecretTerminal::check_captured_bootstrap(&legacy),
            Err(crate::privacy_unlock::PrivateTerminalError::Build(
                crate::privacy_bootstrap::PrivateBuildError::Unavailable
            ))
        ));
        let captured = fixture
            .captured()
            .into_owned()
            .unwrap()
            .into_private_bootstrap(&workspace)
            .unwrap();
        let replacement = fixture.root.join("replacement.json");
        fs::write(&replacement, &bytes).unwrap();
        fs::set_permissions(&replacement, fs::Permissions::from_mode(0o600)).unwrap();
        fs::rename(&replacement, fixture.root.join("config.json")).unwrap();
        assert!(matches!(
            crate::privacy_unlock::OwnedSecretTerminal::check_captured_bootstrap(&captured),
            Err(crate::privacy_unlock::PrivateTerminalError::Source(_))
        ));
        fixture.no_owned_effects(&bytes);
    }
}
