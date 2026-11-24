//! Private owned-input/key/policy/conversation and full-session composition.
//! Only the owned interactive router selects it, through the startup capture.
//! Readiness of a build belongs to its own bound session, which checks its
//! binding each time it issues a turn.
use crate::privacy_cli::{FilePresence, PrivateKeyOperations};
use crate::privacy_key::{ConfigRoot, KeyError, UnlockedVault};
use crate::privacy_policy::RetainedUserConfigurationSource;
use crate::privacy_policy::{CapturedPolicy, PolicyError};
use crate::privacy_session::{
    AdmitError, AdmittedNode, BuildRefusal, CapturedRaw, CapturedUserPolicy,
    FreshConversationBinding, PrivacySession, PrivacyViewSelection, RawOrigin, SessionStop,
};
use crate::privacy_startup::{OwnedTerminalStartup, StartupError, StartupRequirements};
use crate::tui::privacy_unlock::OwnedSecretTerminal;
use std::path::{Path, PathBuf};
use std::sync::Arc;

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
        let canonical = or_return!(std::fs::canonicalize(path), BootstrapError::Workspace);
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
            let held = or_return!(self.directory.metadata(), BootstrapError::Workspace);
            let named = or_return!(
                Self::open(&self.canonical)?.metadata(),
                BootstrapError::Workspace
            );
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
    #[cfg(unix)]
    fn duplicate_owned(&self) -> Result<Self> {
        self.recheck()?;
        #[cfg(unix)]
        {
            let captured = Self {
                canonical: self.canonical.clone(),
                directory: self
                    .directory
                    .try_clone()
                    .map_err(|_| BootstrapError::Workspace)?,
            };
            captured.recheck()?;
            Ok(captured)
        }
        #[cfg(not(unix))]
        {
            Err(BootstrapError::Workspace)
        }
    }
}

/// The consuming startup adapter keeps all four issued owners together. An
/// Arc shares this actual retained object; it cannot reconstruct it from a
/// Config, pathname, policy label, digest or caller requirements.
struct StartupOwners {
    configuration: crate::config::Config,
    policy: CapturedPolicy,
    source: RetainedUserConfigurationSource,
    requirements: StartupRequirements,
}
enum PolicyOwner {
    Legacy(CapturedPolicy),
    Startup(Arc<StartupOwners>),
}
impl std::ops::Deref for PolicyOwner {
    type Target = CapturedPolicy;
    fn deref(&self) -> &CapturedPolicy {
        match self {
            Self::Legacy(policy) => policy,
            Self::Startup(owners) => &owners.policy,
        }
    }
}
impl PolicyOwner {
    fn recheck(&self) -> std::result::Result<(), PolicyError> {
        match self {
            Self::Legacy(policy) => policy.recheck(),
            Self::Startup(owners) => {
                owners.source.recheck()?;
                owners.policy.recheck()?;
                owners.source.recheck()
            }
        }
    }
    fn recheck_startup(&self) -> std::result::Result<(), PolicyError> {
        match self {
            Self::Legacy(_) => Ok(()),
            Self::Startup(_) => self.recheck(),
        }
    }
    fn startup(&self) -> Option<&StartupOwners> {
        match self {
            Self::Legacy(_) => None,
            Self::Startup(owners) => Some(owners),
        }
    }
}

impl OwnedTerminalStartup {
    /// Consume only the actual startup issuer's configuration, policy, source
    /// and requirements. The owned interactive router and the private
    /// captured entry are its callers.
    /// Workspace capture/reservation reuse the existing actual native owners.
    pub fn into_private_bootstrap(self, workspace: &Path) -> Result<PrivateBootstrap> {
        self.bootstrap(workspace, true)
    }
    /// The same four owners for a read-only inspection of the source, the
    /// key files and the detector's stored approval. No conversation is
    /// reserved and no thread exists afterwards; only a real startup
    /// allocates its fresh binding.
    pub(crate) fn into_inspection(self, workspace: &Path) -> Result<PrivateBootstrap> {
        self.bootstrap(workspace, false)
    }
    fn bootstrap(self, workspace: &Path, reserve: bool) -> Result<PrivateBootstrap> {
        let (configuration, policy, source, requirements) = match self.into_configuration_policy() {
            Ok(owners) => owners,
            Err(StartupError::Source) => return Err(BootstrapError::ConfigurationChanged),
            Err(StartupError::Invalid) => return Err(BootstrapError::Policy(PolicyError::Invalid)),
            Err(StartupError::NotYetAvailable) => {
                return Err(BootstrapError::Policy(PolicyError::NotYetAvailable));
            }
            Err(_) => return Err(BootstrapError::Source),
        };
        let policy = PolicyOwner::Startup(Arc::new(StartupOwners {
            configuration,
            policy,
            source,
            requirements,
        }));
        policy.recheck()?;
        let key_root = policy.root().duplicate_owned()?;
        policy.root().with_captured_path(|_| Ok(()))?;
        let workspace = Workspace::capture(workspace)?;
        let binding = match reserve {
            true => Some(workspace.reserve()?),
            false => None,
        };
        policy.recheck()?;
        Ok(PrivateBootstrap {
            policy,
            operations: PrivateKeyOperations::from_captured_root(key_root),
            workspace,
            binding,
        })
    }
}

/// These owners all come from the SAME captured actual user ConfigRoot. The
/// policy is loaded before any project merge or key operation. A proposed
/// enabled policy is usable only through this owner; every other frontend's
/// Config::load still refuses enabled privacy as not-yet-available.
pub struct PrivateBootstrap {
    policy: PolicyOwner,
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
            policy: PolicyOwner::Legacy(policy),
            operations: PrivateKeyOperations::from_captured_root(key_root),
            workspace,
            binding: Some(binding),
        })
    }
    /// The original captured configuration is read-only and source-checked.
    /// Legacy root-only bootstrap has no startup configuration authority.
    pub fn captured_configuration(&self) -> Result<Option<&crate::config::Config>> {
        self.policy.recheck()?;
        Ok(self.policy.startup().map(|owners| &owners.configuration))
    }
    pub fn startup_requirements(&self) -> Result<Option<&StartupRequirements>> {
        self.policy.recheck()?;
        Ok(self.policy.startup().map(|owners| &owners.requirements))
    }
    pub fn status(&self) -> Result<FilePresence> {
        self.policy.recheck()?;
        Ok(self.operations.status()?)
    }
    /// The bootstrap of the one conversation that succeeds a retired one:
    /// the binding that the retirement itself named, for this same
    /// workspace. Only an unreserved bootstrap takes it. The unlock that
    /// follows opens that conversation's own scope.
    #[cfg(unix)]
    pub(crate) fn carrying(mut self, carried: &CarriedConversation) -> Result<Self> {
        self.policy.recheck()?;
        self.workspace.recheck()?;
        let Some(binding) = carried.retired.successor() else {
            return Err(BootstrapError::Session(SessionStop::Scope));
        };
        if self.binding.is_some() || !binding.is_for(&self.workspace.canonical) {
            return Err(BootstrapError::Workspace);
        }
        self.binding = Some(binding);
        Ok(self)
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
        self.policy.recheck_startup()?;
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
    /// Read-only, before any terminal or key work: the detector that this
    /// policy pins is installed with exactly those bytes, declares no
    /// capability and has a stored source approval. Nothing is compiled,
    /// described, run or approved here; every later call checks again.
    pub(crate) async fn detector_source_approved(&self) -> Result<()> {
        self.require_detector_policy()?;
        #[cfg(feature = "gadgets")]
        {
            let pinned = pinned_detector(&self.policy, &self.workspace)?;
            if pinned.source_approved().await.is_err() {
                return Err(BootstrapError::Source);
            }
            self.policy.recheck()?;
            Ok(())
        }
        #[cfg(not(feature = "gadgets"))]
        Err(BootstrapError::DetectorUnavailable)
    }
    fn require_detector_policy(&self) -> Result<()> {
        self.policy.recheck_startup()?;
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
    Ok(crate::privacy_hook::Detector::pinned(pinned_detector(
        policy, workspace,
    )?))
}
#[cfg(feature = "gadgets")]
fn pinned_detector(
    policy: &CapturedPolicy,
    workspace: &Workspace,
) -> Result<crate::privacy_hook::PinnedDetector> {
    use crate::privacy_hook::{DetectorPin, PinnedDetector};
    let selected = policy
        .proposed_detector()?
        .ok_or(BootstrapError::NoPolicy)?;
    let hex = |bytes: &[u8; 32]| {
        bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    };
    let pin = or_return!(
        DetectorPin::supplied_by_host(
            selected.gadget(),
            selected.function(),
            &hex(selected.component_pin()),
            &hex(selected.manifest_pin()),
        ),
        BootstrapError::Source
    );
    workspace.recheck()?;
    // Runtime creation does not read/compile/describe a guest. Its pinned
    // call rechecks owned bytes, hashes, zero capabilities and stored exact
    // source approval BEFORE compilation/describe/guest execution.
    let pinned = or_return!(
        policy.root().with_captured_path(|root| {
            match PinnedDetector::new(&workspace.canonical, root, pin) {
                Ok(pinned) => Ok(pinned),
                Err(_) => Err(KeyError::Root),
            }
        }),
        BootstrapError::Source
    );
    policy.recheck()?;
    workspace.recheck()?;
    Ok(pinned)
}
#[cfg(not(feature = "gadgets"))]
fn detector(_: &CapturedPolicy, _: &Workspace) -> Result<crate::privacy_hook::Detector> {
    Err(BootstrapError::DetectorUnavailable)
}

/// Retains policy AND unlocked key/vault for the life of the conversation.
/// The future full-selection builder must consume this owner, not take an
/// unowned session Arc or a fixture Boolean in its place.
pub struct PrivateConversation {
    policy: PolicyOwner,
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
    #[cfg(unix)]
    pub(crate) fn session_guard_for_retirement(&self) -> Arc<crate::privacy_send::SendGuard> {
        self.session.send_guard()
    }
    /// Startup retirement shares the retained four-owner object; legacy
    /// root-only retirement recaptures after source/workspace rechecks. Both
    /// delay reservation until unlock; equal bytes or paths renew no authority.
    #[cfg(unix)]
    pub(crate) fn capture_after_retirement(&self) -> Result<PrivateBootstrap> {
        match self.policy.recheck() {
            Ok(()) => {}
            Err(PolicyError::Source) => return Err(BootstrapError::ConfigurationChanged),
            Err(error) => return Err(BootstrapError::Policy(error)),
        }
        self.workspace.recheck()?;
        if let PolicyOwner::Startup(owners) = &self.policy {
            let policy = PolicyOwner::Startup(owners.clone());
            let key_root = policy.root().duplicate_owned()?;
            let workspace = self.workspace.duplicate_owned()?;
            policy.recheck()?;
            return Ok(PrivateBootstrap {
                policy,
                operations: PrivateKeyOperations::from_captured_root(key_root),
                workspace,
                binding: None,
            });
        }
        let root = self.policy.root().duplicate_owned()?;
        let key_root = root.duplicate_owned()?;
        let policy = CapturedPolicy::capture_from_root(root)?;
        let workspace = Workspace::capture(&self.workspace.canonical)?;
        self.policy.recheck()?;
        self.workspace.recheck()?;
        Ok(PrivateBootstrap {
            policy: PolicyOwner::Legacy(policy),
            operations: PrivateKeyOperations::from_captured_root(key_root),
            workspace,
            binding: None,
        })
    }
    pub fn captured_configuration(&self) -> Result<Option<&crate::config::Config>> {
        self.check_sources()?;
        Ok(self.policy.startup().map(|owners| &owners.configuration))
    }
    pub fn startup_requirements(&self) -> Result<Option<&StartupRequirements>> {
        self.check_sources()?;
        Ok(self.policy.startup().map(|owners| &owners.requirements))
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
        match self.policy.recheck() {
            Ok(()) => {}
            Err(PolicyError::Source) => return Err(BootstrapError::ConfigurationChanged),
            Err(error) => return Err(BootstrapError::Policy(error)),
        }
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
    /// The conversation gives up its detector and its own references to
    /// masked texts, and the vault its connection, database and keys. What
    /// is still referenced (a session, a node of a recorded message, a
    /// masked text) keeps its identity and can open nothing. A vault worker
    /// that had started keeps its own reference until it returns.
    pub fn close(&self) {
        self.session.release();
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
        extras: crate::agent::SessionExtras,
    ) -> std::result::Result<PrivateBuiltSession, PrivateBuildError> {
        // The source owner establishes origin. Reference identity only adds
        // a substitution refusal: an equal caller/merged Config is not the
        // original configuration retained in those actual four owners.
        if let Some(owners) = self.policy.startup() {
            self.check_sources().map_err(PrivateBuildError::Source)?;
            if !std::ptr::eq(cfg, &owners.configuration) {
                return Err(PrivateBuildError::Unavailable);
            }
            return self.request_captured_full_session(approval, extras).await;
        }
        // Retain the unchanged shared construction future in one allocation,
        // rather than duplicate its concrete runtime/drop graph here.
        let request: std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = std::result::Result<PrivateBuiltSession, PrivateBuildError>,
                    > + Send
                    + '_,
            >,
        > = Box::pin(self.request_bound_full_session(cfg, approval, extras));
        request.await
    }
    /// Unselected fresh construction from the same four retained startup
    /// owners. No caller Config or merged extras can stand in for its source.
    /// All original requested extras go to the existing full shared builder;
    /// pending StartupRequirements still confer no normal activation.
    #[cfg(unix)]
    pub(crate) async fn request_captured_full_session(
        self: &Arc<Self>,
        approval: Arc<std::sync::Mutex<crate::hitl::ApprovalState>>,
        extras: crate::agent::SessionExtras,
    ) -> std::result::Result<PrivateBuiltSession, PrivateBuildError> {
        self.check_sources().map_err(PrivateBuildError::Source)?;
        let owners = self
            .policy
            .startup()
            .ok_or(PrivateBuildError::Unavailable)?;
        let request: std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = std::result::Result<PrivateBuiltSession, PrivateBuildError>,
                    > + Send
                    + '_,
            >,
        > = Box::pin(self.request_bound_full_session(&owners.configuration, approval, extras));
        request.await
    }
    #[cfg(unix)]
    async fn request_bound_full_session(
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
        // The request as this frontend made it, before the builder takes it.
        let request = extras.original();
        let cancellation = extras.cancellation_session.clone();
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
            return Err(self
                .refuse_materialized(session, PrivateBuildError::Unavailable)
                .await);
        }
        if let Err(error) = self.check_sources() {
            return Err(self
                .refuse_materialized(session, PrivateBuildError::Source(error))
                .await);
        }
        // The witness of this build, from its own composed objects. A
        // requested part that is not there, or a wire the policy cannot
        // inspect, is refused with the builder's own words; the session is
        // shut down for real and never becomes a built owner.
        let witness = match &session.privacy_view {
            Some(view) => BuildWitness {
                report,
                boundary: view.boundary().clone(),
                request,
                cancellation,
            },
            None => {
                return Err(self
                    .refuse_materialized(
                        session,
                        PrivateBuildError::Refused(BuildRefusal::RequestedPart),
                    )
                    .await);
            }
        };
        if let Err(refusal) = witness.report.holds(
            &session,
            &self.session,
            self.requires_inspectable_provider_items(),
            &witness.request,
            witness.cancellation.as_ref(),
        ) {
            return Err(self
                .refuse_materialized(session, PrivateBuildError::Refused(refusal))
                .await);
        }
        Ok(PrivateBuiltSession {
            session,
            owner: self.clone(),
            witness,
        })
    }
    /// The shared builder had already materialized `session` when this
    /// owner's binding or source check refused it. A startup-issued owner
    /// shuts that session down for real before the refusal returns, within
    /// the terminal's 30-second bound. The session never leaves this function
    /// and cannot become a `PrivateBuiltSession`. The refusal is kept; a
    /// shutdown that did not finish is reported around it. A legacy root-only
    /// owner drops the session, as it always did.
    #[cfg(unix)]
    async fn refuse_materialized(
        &self,
        session: crate::agent::Session,
        refusal: PrivateBuildError,
    ) -> PrivateBuildError {
        if self.policy.startup().is_none() {
            return refusal;
        }
        match tokio::time::timeout(REFUSED_SESSION_SHUTDOWN, session.shut_down_owned()).await {
            Ok(true) => refusal,
            _ => PrivateBuildError::CleanupAfter(Box::new(refusal)),
        }
    }
}

/// The owned terminal's shutdown bound, for a session that a startup-issued
/// owner refused after the shared builder had returned it.
#[cfg(unix)]
const REFUSED_SESSION_SHUTDOWN: std::time::Duration = std::time::Duration::from_secs(30);

impl CapturedUserPolicy for PrivateConversation {
    fn precheck_file_target(
        &self,
        request: &crate::privacy_session::FileTargetRequest,
    ) -> Option<crate::privacy_session::FileTargetPrecheck> {
        self.check_sources().ok()?;
        request.require_session(&self.session).ok()?;
        request.planned().check_live().ok()?;
        if request.workspace_sha256() != self.session.workspace_sha256() {
            return None;
        }
        let precheck = self
            .policy
            .precheck_data_target(&self.workspace.canonical, request)?;
        self.check_sources().ok()?;
        Some(precheck)
    }
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
    /// A startup-issued owner refused a session the shared builder had
    /// already returned, and that session's real shutdown did not finish.
    /// The refusal itself is carried unchanged.
    #[error(
        "A refused private terminal session's cleanup did not finish; work may still be completing."
    )]
    CleanupAfter(Box<PrivateBuildError>),
}
impl std::fmt::Debug for PrivateBuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PrivatePrivacyBuildError([fixed])")
    }
}

/// Holds actual owners through the private full-session path and every
/// shared lane.
pub struct PrivateBuiltSession {
    session: crate::agent::Session,
    owner: Arc<PrivateConversation>,
    #[cfg(unix)]
    witness: BuildWitness,
}

/// What a private conversation leaves when it is retired for a settings
/// change of the person's own: the agent's transfer of its whole record, the
/// conversation's own retired values, and the session's usage totals. Made
/// only by [`PrivateBuiltSession::retire_for_successor`]; adopted once, by
/// the session built for the successor conversation.
#[cfg(unix)]
pub(crate) struct CarriedConversation {
    transfer: craftsman_harness::provenance::ActualConversationTransfer,
    retired: crate::privacy_session::RetiredConversation,
    usage: (u64, u64),
}

/// The adoption of a retired conversation as owned work. See
/// [`PrivateBuiltSession::adoption`].
#[cfg(unix)]
pub(crate) type OwnedAdoption =
    std::pin::Pin<Box<dyn std::future::Future<Output = std::result::Result<(), String>> + Send>>;

/// What one actual build accepted and composed for the request that was made
/// of it, held by the built owner. Every entry is an object of this build:
/// - **the original request**, as the frontend made it before the builder
///   took it: each prompt, sink and owner by the address of that object, and
///   each choice as it was;
/// - **each model lane's provider**, by the receipt that provider issued in
///   its constructor and keeps through every wrapping, which holds the
///   conversation and guard it bound and is marked with this selection and
///   lane where the builder took the provider, before the framework got it;
/// - **each composed part and installed owner**, by the address of the object
///   the builder made or installed, compared with the object the root
///   agent's boundary holds and with the request's own;
/// - **every middleware the builder kept**, each still there and still
///   stating a source path itself.
///
/// It is not the builder's success, a count, a name, a supported-feature
/// flag or anything a caller supplies. A fresh clear makes a new selection
/// and session, which get their own.
#[cfg(unix)]
struct BuildWitness {
    report: Arc<crate::privacy_session::SelectionReport>,
    boundary: Arc<crate::privacy_session::ConversationBoundary>,
    request: [usize; crate::agent::REQUEST_FACTS],
    /// The frontend's own cancellation owner, kept beside the one it handed
    /// the builder: the agent's installed owner is compared with this one.
    cancellation: Option<craftsman_harness::SessionCancellation>,
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
    /// Real shutdown of a session that returned to a frontend which had
    /// already begun to retire and so was never installed. Nothing of the
    /// session is handed out; dropping this afterwards closes the owner.
    #[cfg(unix)]
    pub(crate) async fn shut_down(self) -> bool {
        self.session.shut_down_owned().await
    }
    #[cfg(unix)]
    pub(crate) fn session_for_terminal(&self) -> &crate::agent::Session {
        &self.session
    }
    #[cfg(unix)]
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
        // A startup that the router selected submits model turns and effects
        // only through its own bound session. This is the build-specific half
        // of its readiness, checked for every submission and by this actual
        // session alone: it still is the live conversation's thread, view and
        // send guard, with the sources just rechecked. A stale or foreign
        // binding is refused; a fresh clear builds a new session whose own
        // binding is checked here again.
        #[cfg(unix)]
        if let Some(owners) = self.owner.policy.startup()
            && owners.requirements.normal_routing_selected()
        {
            let guard = match &self.session.send_guard {
                Some(guard) => Arc::ptr_eq(guard, &self.owner.session.send_guard()),
                None => false,
            };
            if !guard
                || self.session.thread_id != self.owner.thread_id()
                || !Arc::ptr_eq(view.session(), &self.owner.session)
                || !Arc::ptr_eq(view.boundary(), &self.witness.boundary)
                || self
                    .witness
                    .report
                    .holds(
                        &self.session,
                        &self.owner.session,
                        {
                            use crate::privacy_session::CapturedUserPolicy;
                            self.owner.requires_inspectable_provider_items()
                        },
                        &self.witness.request,
                        self.witness.cancellation.as_ref(),
                    )
                    .is_err()
            {
                return Err(PrivateTurnError::Source(BootstrapError::Session(
                    SessionStop::Closed,
                )));
            }
        }
        Ok(PrivateTaskOwner {
            agent: self.session.agent.clone(),
            view: view.clone_for_terminal(),
            owner: self.owner.clone(),
        })
    }
    pub fn thread_id(&self) -> &str {
        &self.session.thread_id
    }
    /// The canonical workspace this conversation was captured for.
    #[cfg(unix)]
    pub(crate) fn workspace(&self) -> &Path {
        &self.owner.workspace.canonical
    }
    /// The options of the actual invocation this conversation was started
    /// with, when a startup issued it.
    #[cfg(unix)]
    pub(crate) fn startup_options(
        &self,
    ) -> Option<&crate::privacy_startup::CapturedTerminalOptions> {
        self.owner
            .policy
            .startup()
            .and_then(|owners| owners.requirements.terminal_options())
    }
    /// Whether this conversation's policy requires that everything a
    /// provider is sent can be inspected, as its own build was held to.
    #[cfg(unix)]
    pub(crate) fn requires_inspectable_items(&self) -> bool {
        use crate::privacy_session::CapturedUserPolicy;
        self.owner.requires_inspectable_provider_items()
    }
    /// Whether `original` is, byte for byte, the user file this
    /// conversation was captured from, with the retained root and lease
    /// checked again. A conversation without a startup has no such file.
    #[cfg(unix)]
    pub(crate) fn captured_from(&self, original: &[u8]) -> bool {
        match self.owner.policy.startup() {
            Some(owners) => owners.source.holds(original),
            None => false,
        }
    }
    /// The captured user configuration root and the captured workspace,
    /// for the extensions this conversation's start could see.
    #[cfg(unix)]
    pub(crate) fn extension_roots(&self) -> Option<(PathBuf, &Path)> {
        self.owner.check_sources().ok()?;
        let config = self
            .owner
            .policy
            .root()
            .with_captured_path(|path| Ok(path.to_owned()))
            .ok()?;
        Some((config, &self.owner.workspace.canonical))
    }
    /// The ordinary manual compaction of this bound session's own
    /// conversation, as work for the frontend to own: the same idle
    /// compaction as the ordinary terminal's, through the agent's tracked
    /// path, so the summary and the kept messages keep their sources. The
    /// bound owner is checked before it starts and the sources before its
    /// line is shown.
    #[cfg(unix)]
    pub(crate) fn compact(
        &self,
    ) -> std::result::Result<
        std::pin::Pin<Box<dyn std::future::Future<Output = String> + Send>>,
        String,
    > {
        let task = match self.task_owner() {
            Ok(task) => task,
            Err(error) => return Err(error.to_string()),
        };
        let compaction = self.session.compaction.clone();
        Ok(Box::pin(async move {
            let line = match task.agent.compact_idle_with_view(&compaction).await {
                Ok(Some((before, after))) => {
                    format!("compacted conversation: {before} -> {after} messages (saved)")
                }
                Ok(None) => "(conversation too short to compact)".into(),
                Err(craftsman_harness::AgentError::Cancelled) => {
                    "could not compact conversation: cancelled by the user".into()
                }
                Err(error) => format!("could not compact conversation: {error}"),
            };
            match task.owner.check_sources() {
                Ok(()) => line,
                Err(_) => {
                    "The private source owner stopped; this local result is not shown.".into()
                }
            }
        }))
    }
    /// The view of this bound session, after the same checks as a turn.
    #[cfg(unix)]
    fn bound_view(&self) -> std::result::Result<&crate::privacy_session::PrivacyView, String> {
        if let Err(error) = self.task_owner() {
            return Err(error.to_string());
        }
        match &self.session.privacy_view {
            Some(view) => Ok(view),
            None => Err(BootstrapError::Session(SessionStop::Closed).to_string()),
        }
    }
    /// Read-only: whether this conversation can be carried over as it
    /// stands. The line says why not; nothing is changed either way.
    #[cfg(unix)]
    pub(crate) fn carriable(&self) -> std::result::Result<(), String> {
        let view = self.bound_view()?;
        let agent = &self.session.agent;
        if agent.pending_suspension().is_some() {
            return Err(
                "Resolve the pending approval before changing a setting; nothing was changed."
                    .into(),
            );
        }
        let running = self
            .session
            .registry
            .list()
            .iter()
            .any(|(_, status)| matches!(status, craftsman_harness::AgentStatus::Running));
        if running {
            return Err(
                "A background task is still running. Wait for it or cancel it before changing a setting; nothing was changed."
                    .into(),
            );
        }
        // Every recorded message with its source, as the agent holds them;
        // none while a message has no source yet.
        let recorded = match agent.actual_sourced_messages() {
            Ok(recorded) => Some(recorded),
            Err(_) => None,
        };
        match view
            .session()
            .carriable(view.boundary(), recorded.as_deref())
        {
            Ok(()) => Ok(()),
            Err(why) => Err(why.to_owned()),
        }
    }
    /// Retires this conversation for its successor. In order: the same
    /// read-only checks as [`carriable`](Self::carriable); the
    /// conversation's own record of its private values, read while it is
    /// live; then the agent's own retirement, which issues its whole record
    /// and closes the source view, the send guard and the vault scope; then
    /// the key. A refusal before the agent's retirement keeps the
    /// conversation as it was. Nothing is sent, saved or replayed.
    #[cfg(unix)]
    pub(crate) async fn retire_for_successor(
        &self,
    ) -> std::result::Result<CarriedConversation, String> {
        self.carriable()?;
        let view = self.bound_view()?;
        // The vault's room for the successor scope, asked while this
        // conversation is still whole. Nothing is reserved by it.
        view.session().successor_has_room().await?;
        let retired = match view.session().retiring(view.boundary()) {
            Ok(retired) => retired,
            Err(stop) => return Err(stop.to_string()),
        };
        let usage = *self.session.usage.lock().unwrap();
        let transfer = match self.session.agent.retire_conversation_with_view().await {
            Ok(transfer) => transfer,
            Err(error) => {
                // The agent refuses most retirements with the conversation
                // as it was. A record that no longer agrees with its
                // sources closes the conversation instead, and the line
                // must not say that it was kept.
                let line = error.to_string();
                return Err(match self.owner.check_sources() {
                    Ok(()) => line,
                    Err(_) => format!(
                        "{line} The conversation has ended; what was last saved of it remains in the thread store."
                    ),
                });
            }
        };
        self.owner.close();
        Ok(CarriedConversation {
            transfer,
            retired,
            usage,
        })
    }
    /// The adoption of a retired conversation into this newly built
    /// session, which was built for its successor, as one owned operation.
    /// This session's own binding is checked here, as for a turn; the
    /// returned work holds the same agent, view and owners and is run by
    /// whoever keeps it. The agent issues one adopted commit per recorded
    /// message and this conversation admits every original anew; then the
    /// session's usage totals follow. An error adopts nothing.
    ///
    /// Work that is dropped before it returned adopted nothing: the agent's
    /// one commit is a single step with nothing awaited after it. What the
    /// new scope's vault had already stored for it stays stored, and a
    /// vault or detector call that was running finishes or refuses under
    /// its own bounds.
    #[cfg(unix)]
    pub(crate) fn adoption(
        &self,
        carried: Arc<CarriedConversation>,
    ) -> std::result::Result<OwnedAdoption, String> {
        let owner = match self.task_owner() {
            Ok(owner) => owner,
            Err(error) => return Err(error.to_string()),
        };
        let usage = self.session.usage.clone();
        Ok(Box::pin(async move {
            let adopter = owner
                .view
                .session()
                .adopter(owner.view.boundary(), &carried.retired);
            if let Err(error) = owner
                .agent
                .adopt_conversation_with_view(&carried.transfer, &adopter)
                .await
            {
                return Err(error.to_string());
            }
            // The record is this agent's now. The controllers that were
            // installed on it take it over, in this same step: nothing is
            // awaited between the agent's commit and here.
            if let Err(error) = owner
                .view
                .boundary()
                .settle_adopted(&owner.agent, &carried.transfer)
            {
                return Err(format!(
                    "The record was adopted, and a controller of the new conversation did not take it over: {error}"
                ));
            }
            *usage.lock().unwrap() = carried.usage;
            match owner.owner.check_sources() {
                Ok(()) => Ok(()),
                Err(error) => Err(error.to_string()),
            }
        }))
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
