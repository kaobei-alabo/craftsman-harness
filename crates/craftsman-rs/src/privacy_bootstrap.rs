//! Private owned-input/key/policy/conversation composition. No normal CLI,
//! frontend or enabled policy can select this path. It builds no Agent: the
//! full expected-set source/provider/mutator selection is not available yet.
use crate::privacy_cli::{FilePresence, PrivateKeyOperations};
use crate::privacy_key::{ConfigRoot, KeyError, UnlockedVault};
use crate::privacy_policy::{CapturedPolicy, PolicyError};
use crate::privacy_session::{
    AdmitError, AdmittedNode, BuildRefusal, CapturedRaw, CapturedUserPolicy,
    FreshConversationBinding, PrivacySession, PrivacyViewSelection, RawOrigin, SessionStop,
};
use crate::tui::privacy_unlock::OwnedSecretTerminal;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum BootstrapError {
    #[error("The private bootstrap key operation stopped: {0}")]
    Key(KeyError),
    #[error("The private bootstrap user policy was refused.")]
    Policy(PolicyError),
    #[error(
        "The captured user configuration changed or is unavailable; the private conversation has ended. Start a fresh privacy bootstrap before continuing."
    )]
    ConfigurationChanged,
    #[error(
        "Settings cannot be saved while this private conversation is active; its configuration is unchanged."
    )]
    SettingsSaveRefused,
    #[error("The private bootstrap requires a captured proposed user privacy policy.")]
    NoPolicy,
    #[error("The private bootstrap requires the source-built detector feature.")]
    DetectorUnavailable,
    #[error(
        "The private bootstrap cannot omit declared identifiers that its session cannot yet admit."
    )]
    IdentifiersUnavailable,
    #[error("The private bootstrap detector source was refused.")]
    Source,
    #[error("The private bootstrap workspace authority is unavailable or changed.")]
    Workspace,
    #[error("The private bootstrap conversation could not be opened.")]
    Session(SessionStop),
    #[error("The private bootstrap input could not be admitted.")]
    Admission(AdmitError),
}
pub type Result<T> = std::result::Result<T, BootstrapError>;

impl From<KeyError> for BootstrapError {
    fn from(error: KeyError) -> Self {
        Self::Key(error)
    }
}
impl From<PolicyError> for BootstrapError {
    fn from(error: PolicyError) -> Self {
        Self::Policy(error)
    }
}

/// Captured from the caller's actual workspace, not a caller-supplied thread or
/// scope label. Identity observations are sampled, not a rename-proof OS lease.
struct Workspace {
    canonical: PathBuf,
    #[cfg(unix)]
    directory: std::fs::File,
}
impl Workspace {
    fn capture(path: &Path) -> Result<Self> {
        if !path.is_absolute() {
            return Err(BootstrapError::Workspace);
        }
        let canonical = std::fs::canonicalize(path).map_err(|_| BootstrapError::Workspace)?;
        #[cfg(unix)]
        {
            let captured = Self {
                directory: Self::open(&canonical)?,
                canonical,
            };
            captured.recheck()?;
            Ok(captured)
        }
        #[cfg(not(unix))]
        {
            let _ = canonical;
            Err(BootstrapError::Workspace)
        }
    }
    #[cfg(unix)]
    fn open(path: &Path) -> Result<std::fs::File> {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)
            .map_err(|_| BootstrapError::Workspace)
    }
    fn recheck(&self) -> Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let held = self
                .directory
                .metadata()
                .map_err(|_| BootstrapError::Workspace)?;
            let named = Self::open(&self.canonical)?
                .metadata()
                .map_err(|_| BootstrapError::Workspace)?;
            if !held.is_dir()
                || !named.is_dir()
                || held.dev() != named.dev()
                || held.ino() != named.ino()
                || std::fs::canonicalize(&self.canonical).map_err(|_| BootstrapError::Workspace)?
                    != self.canonical
            {
                return Err(BootstrapError::Workspace);
            }
            Ok(())
        }
        #[cfg(not(unix))]
        {
            Err(BootstrapError::Workspace)
        }
    }
    fn reserve(&self) -> Result<FreshConversationBinding> {
        self.recheck()?;
        let binding =
            FreshConversationBinding::reserve(&self.canonical).map_err(BootstrapError::Session)?;
        self.recheck()?;
        Ok(binding)
    }
}

/// These owners all come from the SAME captured actual user ConfigRoot. The
/// policy is loaded before any project merge or key operation. A proposed
/// enabled policy is usable only by this unselected private entry; normal
/// Config::load still refuses enabled privacy as not-yet-available.
pub struct PrivateBootstrap {
    policy: CapturedPolicy,
    operations: PrivateKeyOperations,
    workspace: Workspace,
    binding: Option<FreshConversationBinding>,
}
impl std::fmt::Debug for PrivateBootstrap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PrivatePrivacyBootstrap([private])")
    }
}
impl PrivateBootstrap {
    pub fn capture_actual(workspace: &Path) -> Result<Self> {
        Self::from_captured_root(ConfigRoot::capture_actual()?, workspace)
    }
    /// A retained ConfigRoot is authority; a path/flag/merged extra is not.
    pub fn from_captured_root(root: ConfigRoot, workspace: &Path) -> Result<Self> {
        let key_root = root.duplicate_owned()?;
        let policy = CapturedPolicy::capture_from_root(root)?;
        policy.root().with_captured_path(|_| Ok(()))?;
        let workspace = Workspace::capture(workspace)?;
        let binding = workspace.reserve()?;
        policy.recheck()?;
        Ok(Self {
            policy,
            operations: PrivateKeyOperations::from_captured_root(key_root),
            workspace,
            binding: Some(binding),
        })
    }
    pub fn status(&self) -> Result<FilePresence> {
        self.policy.recheck()?;
        Ok(self.operations.status()?)
    }
    /// Explicit init with a real confirmation popup, exclusive creation and
    /// no enabling, adoption, cleanup deletion or subsequent model request.
    pub async fn initialize(self, terminal: &mut OwnedSecretTerminal) -> Result<()> {
        self.policy.recheck()?;
        let unlocked = terminal.initialize(&self.operations).await?;
        self.policy.recheck()?;
        unlocked.close();
        Ok(())
    }
    /// Existing-only unlock plus a genuine pinned detector and the reserved
    /// conversation. This returns an admission owner, never a reduced Agent.
    pub async fn unlock_private(
        self,
        terminal: &mut OwnedSecretTerminal,
    ) -> Result<PrivateConversation> {
        self.require_detector_policy()?;
        self.workspace.recheck()?;
        let unlocked = terminal.unlock(&self.operations).await?;
        let detector = detector(&self.policy, &self.workspace)?;
        // A clear captures authority before asking, but reserves its next
        // actual conversation only AFTER the fresh key authenticated.
        let binding = match self.binding {
            Some(binding) => binding,
            None => self.workspace.reserve()?,
        };
        let session = Arc::new(
            PrivacySession::open(unlocked.vault(), binding, detector)
                .await
                .map_err(BootstrapError::Session)?,
        );
        let conversation = PrivateConversation {
            policy: self.policy,
            unlocked,
            workspace: self.workspace,
            session,
        };
        conversation.check_sources()?;
        Ok(conversation)
    }
    fn require_detector_policy(&self) -> Result<()> {
        let proposed = self
            .policy
            .proposed_detector()?
            .ok_or(BootstrapError::NoPolicy)?;
        // B575 always masks with DeclaredIdentifiers::none(). Never silently
        // turn a nonempty actual user policy into that narrower fixture path.
        if proposed.literals().next().is_some() {
            return Err(BootstrapError::IdentifiersUnavailable);
        }
        #[cfg(not(feature = "gadgets"))]
        return Err(BootstrapError::DetectorUnavailable);
        #[cfg(feature = "gadgets")]
        Ok(())
    }
}

#[cfg(feature = "gadgets")]
fn detector(
    policy: &CapturedPolicy,
    workspace: &Workspace,
) -> Result<crate::privacy_hook::Detector> {
    use crate::privacy_hook::{Detector, DetectorPin, PinnedDetector};
    let selected = policy
        .proposed_detector()?
        .ok_or(BootstrapError::NoPolicy)?;
    let hex = |bytes: &[u8; 32]| {
        bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    };
    let pin = DetectorPin::supplied_by_host(
        selected.gadget(),
        selected.function(),
        &hex(selected.component_pin()),
        &hex(selected.manifest_pin()),
    )
    .map_err(|_| BootstrapError::Source)?;
    workspace.recheck()?;
    // Runtime creation does not read/compile/describe a guest. Its pinned
    // call rechecks owned bytes, hashes, zero capabilities and stored exact
    // source approval BEFORE compilation/describe/guest execution.
    let pinned = policy
        .root()
        .with_captured_path(|root| {
            PinnedDetector::new(&workspace.canonical, root, pin).map_err(|_| KeyError::Root)
        })
        .map_err(|_| BootstrapError::Source)?;
    policy.recheck()?;
    workspace.recheck()?;
    Ok(Detector::pinned(pinned))
}
#[cfg(not(feature = "gadgets"))]
fn detector(_: &CapturedPolicy, _: &Workspace) -> Result<crate::privacy_hook::Detector> {
    Err(BootstrapError::DetectorUnavailable)
}

/// Retains policy AND unlocked key/vault for the life of the conversation.
/// The future full-selection builder must consume this owner, not take an
/// unowned session Arc or a fixture Boolean in its place.
pub struct PrivateConversation {
    policy: CapturedPolicy,
    unlocked: UnlockedVault,
    workspace: Workspace,
    session: Arc<PrivacySession>,
}
impl std::fmt::Debug for PrivateConversation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PrivatePrivacyConversation([private])")
    }
}
struct ClearBeforeBuild<'a> {
    owner: &'a mut PrivateConversation,
    completed: bool,
}
impl Drop for ClearBeforeBuild<'_> {
    fn drop(&mut self) {
        if !self.completed {
            self.owner.close();
        }
    }
}
impl PrivateConversation {
    pub(crate) fn session_guard_for_retirement(&self) -> Arc<crate::privacy_send::SendGuard> {
        self.session.send_guard()
    }
    /// A fresh clear keeps the OLD retained root/workspace observations as
    /// prerequisites, recaptures the same real source, and delays allocation
    /// until actual unlock. No equal digest or ambient path renews authority.
    pub(crate) fn capture_after_retirement(&self) -> Result<PrivateBootstrap> {
        self.policy.recheck().map_err(|error| match error {
            PolicyError::Source => BootstrapError::ConfigurationChanged,
            error => BootstrapError::Policy(error),
        })?;
        self.workspace.recheck()?;
        let root = self.policy.root().duplicate_owned()?;
        let key_root = root.duplicate_owned()?;
        let policy = CapturedPolicy::capture_from_root(root)?;
        let workspace = Workspace::capture(&self.workspace.canonical)?;
        self.policy.recheck()?;
        self.workspace.recheck()?;
        Ok(PrivateBootstrap {
            policy,
            operations: PrivateKeyOperations::from_captured_root(key_root),
            workspace,
            binding: None,
        })
    }
    pub fn thread_id(&self) -> &str {
        self.session.thread_id()
    }
    pub fn check_sources(&self) -> Result<()> {
        let result = (|| {
            self.check_key_and_policy()?;
            self.session.check().map_err(BootstrapError::Session)
        })();
        if result.is_err() {
            self.close();
        }
        result
    }
    fn check_key_and_policy(&self) -> Result<()> {
        self.policy.recheck().map_err(|error| match error {
            PolicyError::Source => BootstrapError::ConfigurationChanged,
            error => BootstrapError::Policy(error),
        })?;
        self.workspace.recheck()?;
        // Sample the retained key/source owner without copying raw key bytes.
        self.unlocked.recheck_key_source()?;
        Ok(())
    }
    /// Actual terminal drafts only. Real source approval and authenticated
    /// vault map commits precede release of this input's separate model view.
    /// Canonical accepted human input belongs to the future Agent, unchanged.
    pub async fn admit_typed(&self, typed: &str) -> Result<AdmittedNode> {
        self.check_sources()?;
        let raw = CapturedRaw::from_producer(RawOrigin::TerminalDraft, typed);
        let node = self
            .session
            .admit_raw(&raw)
            .await
            .map_err(BootstrapError::Admission)?;
        self.check_sources()?;
        Ok(node)
    }
    /// Before any full Agent build exists: clear closes the OLD owner first,
    /// reserves a NEW actual id and opens a NEW scope. A future terminal /clear
    /// must also rebuild all lanes with that id; a void setter is insufficient.
    pub async fn clear_before_new_build(&mut self) -> Result<()> {
        self.session.stop();
        // The borrowed owner remains under this guard through the new scope's
        // actual open/check. Error, panic or future abandonment closes BOTH
        // the old/new scope and key; only completed pre-build clear keeps it.
        let mut clear = ClearBeforeBuild {
            owner: self,
            completed: false,
        };
        let result = async {
            clear.owner.check_key_and_policy()?;
            let binding = clear.owner.workspace.reserve()?;
            let detector = detector(&clear.owner.policy, &clear.owner.workspace)?;
            clear.owner.session = Arc::new(
                PrivacySession::open(clear.owner.unlocked.vault(), binding, detector)
                    .await
                    .map_err(BootstrapError::Session)?,
            );
            clear.owner.check_sources()
        }
        .await;
        clear.completed = result.is_ok();
        result
    }
    pub fn close_before_rebind(&self) {
        self.close();
    }
    /// Revoke the actual session and key even when a policy/tool keeps an Arc.
    /// Existing vault references retain their derived material until release.
    pub fn close(&self) {
        self.session.stop();
        self.unlocked.close();
    }
    /// The private terminal refuses its own config/model/effort saves BEFORE
    /// mutation. A refused save keeps the live conversation and configuration;
    /// an actually changed/unavailable captured source closes both separately.
    pub fn before_configuration_save(&self) -> Result<()> {
        self.check_sources()?;
        Err(BootstrapError::SettingsSaveRefused)
    }

    /// The policy implementation is this actual captured owner, not a host
    /// flag, string, arbitrary trait answer or fixture supported-set permission.
    pub fn selection(self: &Arc<Self>) -> std::result::Result<PrivacyViewSelection, BuildRefusal> {
        PrivacyViewSelection::from_user_policy(self.clone(), self.session.clone())
    }

    /// Requests the SAME shared builder with real frontend extras. Unsupported
    /// paths are reported by that builder; no field/part is silently turned off.
    /// A successful future build retains these owners beside its real Session.
    #[cfg(unix)]
    pub(crate) async fn request_full_session(
        self: &Arc<Self>,
        cfg: &crate::config::Config,
        approval: Arc<std::sync::Mutex<crate::hitl::ApprovalState>>,
        mut extras: crate::agent::SessionExtras,
    ) -> std::result::Result<PrivateBuiltSession, PrivateBuildError> {
        self.check_sources().map_err(PrivateBuildError::Source)?;
        if extras.privacy_view.is_some() || extras.send_guard.is_some() {
            return Err(PrivateBuildError::Refused(BuildRefusal::NotFresh));
        }
        if let Some(root) = &extras.workspace_root
            && std::fs::canonicalize(root).ok().as_ref() != Some(&self.workspace.canonical)
        {
            return Err(PrivateBuildError::Source(BootstrapError::Workspace));
        }
        extras.workspace_root = Some(self.workspace.canonical.clone());
        let selection = self.selection().map_err(PrivateBuildError::Refused)?;
        let report = selection.report();
        extras.privacy_view = Some(selection);
        // Keep the shared builder's large future owned in one allocation.
        // Erasing its concrete type avoids duplicating its runtime/drop graph
        // in this caller; it changes no decision, ordering or cancellation.
        let build: std::pin::Pin<
            Box<dyn std::future::Future<Output = Option<crate::agent::Session>> + Send + '_>,
        > = Box::pin(crate::agent::build_session_ext(
            cfg, approval, "", false, false, None, extras,
        ));
        let built = build.await;
        let Some(session) = built else {
            return Err(match report.refusal() {
                Some(refusal) => PrivateBuildError::Refused(refusal),
                None => PrivateBuildError::Unavailable,
            });
        };
        // The caller cannot substitute a thread/guard or lose the actual owner
        // while returning a live Agent. Reopen/transfer APIs are not exposed.
        let bound = session.thread_id == self.thread_id()
            && session
                .privacy_view
                .as_ref()
                .is_some_and(|view| Arc::ptr_eq(view.session(), &self.session))
            && session
                .send_guard
                .as_ref()
                .is_some_and(|guard| Arc::ptr_eq(guard, &self.session.send_guard()));
        if !bound {
            self.close();
            return Err(PrivateBuildError::Unavailable);
        }
        self.check_sources().map_err(PrivateBuildError::Source)?;
        Ok(PrivateBuiltSession {
            session,
            owner: self.clone(),
        })
    }
}

impl CapturedUserPolicy for PrivateConversation {
    fn enables_privacy_in(&self, workspace_sha256: &str) -> bool {
        workspace_sha256 == self.session.workspace_sha256()
            && self.check_sources().is_ok()
            && matches!(self.policy.proposed_detector(), Ok(Some(_)))
    }
    fn requires_inspectable_provider_items(&self) -> bool {
        // The captured schema currently grants no opaque-item exemption.
        // Keep this conservative requirement; never manufacture permission to
        // accept opaque items from a fixture flag or a transport's name.
        true
    }
    fn restorable_file_data(
        &self,
        workspace_sha256: &str,
        _destination: &Path,
    ) -> Option<crate::privacy_restore::FileDataFormat> {
        if workspace_sha256 != self.session.workspace_sha256() || self.check_sources().is_err() {
            return None;
        }
        // DATA-01/02/03 safety hold: path/format/operation proposals remain
        // captured for review but grant NO restoration. The canonical opaque
        // supported-object/field and retained-effect provider must be installed
        // before this actual owner can authorize any data image.
        None
    }
    fn supported_file_data(
        &self,
        request: &crate::privacy_session::FileDataRequest,
    ) -> Option<crate::privacy_session::SupportedFileData> {
        self.check_sources().ok()?;
        request.require_session(&self.session).ok()?;
        request.planned().check_live().ok()?;
        if request.workspace_sha256() != self.session.workspace_sha256() {
            return None;
        }
        let supported = self
            .policy
            .supported_data(&self.workspace.canonical, request)?;
        self.check_sources().ok()?;
        Some(supported)
    }
    fn retained_file_effect(
        &self,
        request: &crate::privacy_session::FileEffectRequest,
    ) -> Option<crate::privacy_session::RetainedFileEffect> {
        self.check_sources().ok()?;
        request.data().require_session(&self.session).ok()?;
        request.data().planned().check_live().ok()?;
        if request.data().workspace_sha256() != self.session.workspace_sha256() {
            return None;
        }
        let retained = self
            .policy
            .retained_data_effect(&self.workspace.canonical, request)?;
        self.check_sources().ok()?;
        Some(retained)
    }
}

#[derive(Clone, thiserror::Error)]
pub enum PrivateBuildError {
    #[error("The full private terminal session was refused.")]
    Refused(BuildRefusal),
    #[error("The private terminal session source owner stopped.")]
    Source(BootstrapError),
    #[error("The full private terminal session is unavailable.")]
    Unavailable,
}
impl std::fmt::Debug for PrivateBuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PrivatePrivacyBuildError([fixed])")
    }
}

/// Holds the key/policy owner through the real session's lifetime, including
/// every shared lane. Currently unreachable: the full source paths refuse.
pub struct PrivateBuiltSession {
    session: crate::agent::Session,
    owner: Arc<PrivateConversation>,
}

#[derive(thiserror::Error)]
pub enum PrivateTurnError {
    #[error("The private terminal source owner stopped before this input.")]
    Source(BootstrapError),
    #[error("The private terminal entry did not finish successfully.")]
    Entry(crate::privacy_entry::PrivateEntryError),
}
impl std::fmt::Debug for PrivateTurnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PrivatePrivacyTurnError([fixed])")
    }
}
impl std::fmt::Debug for PrivateBuiltSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PrivateBuiltPrivacySession([private])")
    }
}
impl PrivateBuiltSession {
    pub(crate) fn session_for_terminal(&self) -> &crate::agent::Session {
        &self.session
    }
    pub(crate) fn session_for_terminal_mut(&mut self) -> &mut crate::agent::Session {
        &mut self.session
    }
    pub(crate) fn task_owner(&self) -> std::result::Result<PrivateTaskOwner, PrivateTurnError> {
        self.owner
            .check_sources()
            .map_err(PrivateTurnError::Source)?;
        let view = self
            .session
            .privacy_view
            .as_ref()
            .ok_or(PrivateTurnError::Source(BootstrapError::Session(
                SessionStop::Closed,
            )))?;
        Ok(PrivateTaskOwner {
            agent: self.session.agent.clone(),
            view: view.clone_for_terminal(),
            owner: self.owner.clone(),
        })
    }
    pub fn thread_id(&self) -> &str {
        &self.session.thread_id
    }
    pub async fn turn(
        &self,
        typed: &str,
    ) -> std::result::Result<craftsman_harness::TurnResult, PrivateTurnError> {
        self.task_owner()?.turn(typed).await
    }
    pub async fn steer(&self, typed: &str) -> std::result::Result<bool, PrivateTurnError> {
        self.task_owner()?.steer(typed).await
    }
    pub async fn request_decision(
        &self,
        deny_default: &str,
    ) -> std::result::Result<Option<craftsman_harness::TurnResult>, PrivateTurnError> {
        self.task_owner()?.request_decision(deny_default).await
    }
    pub fn close(&self) {
        self.owner.close();
    }
    pub fn before_configuration_save(&self) -> Result<()> {
        self.owner.before_configuration_save()
    }
}

/// Only the actual built session creates this owner. Its same agent/view and
/// captured key/policy stay live through an owned task while the UI retains the
/// actual mutable notification receiver. It has no arbitrary constructor or
/// raw source/agent getter, and creates no new conversation or receipt.
pub(crate) struct PrivateTaskOwner {
    agent: craftsman_harness::Agent,
    view: crate::privacy_session::PrivacyView,
    owner: Arc<PrivateConversation>,
}
impl std::fmt::Debug for PrivateTaskOwner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PrivateTaskOwner([private])")
    }
}
impl PrivateTaskOwner {
    pub async fn turn(
        &self,
        typed: &str,
    ) -> std::result::Result<craftsman_harness::TurnResult, PrivateTurnError> {
        self.owner
            .check_sources()
            .map_err(PrivateTurnError::Source)?;
        let result = self
            .view
            .turn(&self.agent, typed)
            .await
            .map_err(PrivateTurnError::Entry);
        let checked = self.owner.check_sources().map_err(PrivateTurnError::Source);
        match result {
            Err(error) => {
                if checked.is_err() {
                    self.owner.close();
                }
                Err(error)
            }
            Ok(returned) => {
                checked?;
                Ok(returned)
            }
        }
    }
    pub async fn steer(&self, typed: &str) -> std::result::Result<bool, PrivateTurnError> {
        self.owner
            .check_sources()
            .map_err(PrivateTurnError::Source)?;
        let result = self
            .view
            .steer(&self.agent, typed)
            .await
            .map_err(PrivateTurnError::Entry);
        let checked = self.owner.check_sources().map_err(PrivateTurnError::Source);
        match result {
            Err(error) => Err(error),
            Ok(accepted) => {
                checked?;
                Ok(accepted)
            }
        }
    }
    pub async fn request_decision(
        &self,
        deny_default: &str,
    ) -> std::result::Result<Option<craftsman_harness::TurnResult>, PrivateTurnError> {
        self.owner
            .check_sources()
            .map_err(PrivateTurnError::Source)?;
        let result = self
            .view
            .request_decision(&self.agent, deny_default)
            .await
            .map_err(PrivateTurnError::Entry);
        let checked = self.owner.check_sources().map_err(PrivateTurnError::Source);
        match result {
            Err(error) => Err(error),
            Ok(returned) => {
                checked?;
                Ok(returned)
            }
        }
    }
}
impl Drop for PrivateBuiltSession {
    fn drop(&mut self) {
        self.owner.close();
    }
}
impl Drop for PrivateConversation {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(all(test, unix))]
mod tests;
