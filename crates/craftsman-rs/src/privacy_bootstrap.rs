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
        "The captured user configuration changed; the private conversation has ended. Start a fresh privacy bootstrap before continuing."
    )]
    ConfigurationChanged,
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
    binding: FreshConversationBinding,
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
            binding,
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
        let session = Arc::new(
            PrivacySession::open(unlocked.vault(), self.binding, detector)
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
impl PrivateConversation {
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
        self.check_key_and_policy()?;
        let binding = self.workspace.reserve()?;
        let detector = detector(&self.policy, &self.workspace)?;
        self.session = Arc::new(
            PrivacySession::open(self.unlocked.vault(), binding, detector)
                .await
                .map_err(BootstrapError::Session)?,
        );
        self.check_sources()
    }
    pub fn close_before_rebind(&self) {
        self.session.stop();
    }
    /// Revoke the actual session and key even when a policy/tool keeps an Arc.
    /// Existing vault references retain their derived material until release.
    pub fn close(&self) {
        self.session.stop();
        self.unlocked.close();
    }
    /// The private terminal refuses its own config/model/effort saves before
    /// their rename. No equal caller digest can renew the captured source.
    pub fn before_configuration_save(&self) -> Result<()> {
        self.check_sources()?;
        self.close();
        Err(BootstrapError::ConfigurationChanged)
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
            self.session.stop();
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
    pub fn thread_id(&self) -> &str {
        &self.session.thread_id
    }
    pub async fn turn(
        &self,
        typed: &str,
    ) -> std::result::Result<craftsman_harness::TurnResult, PrivateTurnError> {
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
        let result = view
            .turn(&self.session.agent, typed)
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
    pub fn close(&self) {
        self.owner.close();
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
