//! Private admission owner for one conversation: the only place raw source
//! text becomes an admitted node with a model view.
//!
//! Nothing selects this module. No frontend, configuration key, command, key
//! source, provider, tool, approval or display path uses it yet, and normal
//! sessions are unchanged.
//!
//! The types are distinct on purpose:
//!
//! - [`CapturedRaw`] is one actual raw source: what a producer really took in.
//!   It carries no admitted authority.
//! - [`AdmittedNode`] exists only after that particular capture went through
//!   the vault's successful mask and commit, including a mask that found
//!   nothing, and after the send guard admitted its records. It keeps the
//!   capture and the vault's own result.
//! - Text the model produced is a different origin and is not handled here.
//!
//! There is no constructor from a string, digest, label, flag or position
//! that grants admitted authority. Raw text that contains a placeholder
//! already issued in the conversation is refused as ambiguous; it is never
//! restored and masked again. Equal bytes arriving as a new capture are a new
//! raw source, whatever an existing node contains.
//!
//! The canonical record of what was typed, returned or received is kept by
//! its own owner and is not changed here. A node's model view is a separate
//! projection of it.

#[cfg(test)]
mod deadline_tests;
#[cfg(test)]
mod tests;
mod transport;
mod view;
#[cfg(test)]
mod view_tests;

pub use transport::{ViewChatProvider, ViewNativeProvider};
pub use view::{
    ActualToolResult, ApprovedOperation, CommittedModelNode, ConversationBoundary, FieldPlace,
    NativeReplay, ReceivedModel, SourceField,
};

use crate::checkpoints::LibsqlCheckpointer;
use crate::privacy_hook::{Detector, HookError, MAX_TEXT_BYTES, TextSpan};
use crate::privacy_send::{SendGuard, SendRefusal};
use crate::privacy_vault::{
    Category, DeclaredIdentifiers, DetectorReport, DetectorSpan, HostNamespace, MappingSession,
    MaskedText, PrivacyError, ResolutionRecords, Vault,
};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::time::Instant;
use zeroize::Zeroizing;

/// The vault's own per-scope ceiling. A draft that would exceed it is refused
/// here before any effect; the vault's whole-file ceilings stay with the vault.
const SCOPE_MAPPINGS: usize = 512;
/// No placeholder is longer than this, which bounds the model view.
const MAX_PLACEHOLDER_BYTES: usize = 256;
/// One clock per admission, started at entry. Waiting for the owner spends
/// it; it is never restarted after the wait.
const ADMISSION_BOUND: Duration = Duration::from_secs(45);
/// Admissions that may be inside the owner at once: one running, the rest
/// waiting. A further one is turned away at entry.
const ADMISSION_SLOTS: usize = 16;

fn admission_deadline(deadline: Instant, now: Instant) -> Result<(), SessionStop> {
    if now >= deadline {
        Err(SessionStop::Deadline)
    } else {
        Ok(())
    }
}

/// A refusal of one unaccepted draft, decided before any vault or detector
/// effect that the mandatory policy treats as fatal. The session stays usable
/// and the draft is unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum AdmitRefusal {
    #[error("The text is too large to be admitted as one unit.")]
    TooLarge,
    #[error("The text contains a privacy placeholder already issued here.")]
    IssuedPlaceholder,
    #[error("The text contains a known private value that was not reported.")]
    UncoveredValue,
    #[error("The text reports one value under two categories.")]
    ConflictingCategory,
    #[error("This conversation has reached its private value capacity.")]
    ScopeFull,
    #[error("Too many texts are waiting to be admitted.")]
    Waiting,
}

/// Why the whole session can no longer admit or send.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SessionStop {
    #[error("The privacy session is closed.")]
    Closed,
    #[error("The conversation could not be bound to a privacy scope.")]
    Scope,
    #[error("The private vault refused or failed.")]
    Vault(PrivacyError),
    #[error("The private send guard refused.")]
    Guard(SendRefusal),
    #[error("The selected detector failed or is not approved.")]
    Detector(HookError),
    #[error("A privacy admission exceeded its time bound.")]
    Deadline,
    /// An accepted fact, such as a returned tool result or an accepted
    /// answer, has no model view. The fact itself is not changed.
    #[error("An accepted input or result could not be given a model view.")]
    Projection(AdmitRefusal),
    /// A part of a request has no source of this conversation, or its source
    /// does not fit it.
    #[error("A part of the conversation has no source that fits it.")]
    Unbound,
}

/// A draft was refused and may be changed, or the session has stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum AdmitError {
    #[error(transparent)]
    Refused(AdmitRefusal),
    #[error(transparent)]
    Stopped(SessionStop),
}

/// The actual identity of one new conversation, reserved before any privacy
/// or client object is built: the fresh thread id from the application's own
/// allocator, and the workspace it runs in. The same object later gives the
/// session builder its thread id, so the vault scope and the conversation
/// cannot name different threads.
///
/// It cannot be built from a caller's string or flag, and it cannot be
/// cloned. A new conversation, including one started by clearing, reserves a
/// new one.
pub struct FreshConversationBinding {
    thread_id: String,
    workspace: String,
}

impl FreshConversationBinding {
    /// `workspace` is the session's absolute workspace root. It is resolved to
    /// its canonical path once; only that path's SHA-256 is kept.
    pub fn reserve(workspace: &Path) -> Result<Self, SessionStop> {
        if !workspace.is_absolute() {
            return Err(SessionStop::Scope);
        }
        let workspace = std::fs::canonicalize(workspace).map_err(|_| SessionStop::Scope)?;
        let mut digest = Sha256::new();
        digest.update(b"craftsman/privacy-session/workspace/v1\0");
        digest.update(workspace.as_os_str().as_encoded_bytes());
        Ok(Self {
            thread_id: LibsqlCheckpointer::new_thread_id(),
            workspace: format!("{:x}", digest.finalize()),
        })
    }

    /// The reserved thread id, for the one builder of this conversation.
    pub fn thread_id(&self) -> &str {
        &self.thread_id
    }

    fn namespace(&self) -> Result<HostNamespace, SessionStop> {
        HostNamespace::supplied_by_host(
            "craftsman/privacy-session/v1",
            &self.workspace,
            &self.thread_id,
            "conversation",
            "main+delegated+compaction/v1",
        )
        .map_err(|_| SessionStop::Scope)
    }
}

/// The user's captured privacy policy, as far as the session builder needs
/// it. The application's policy owner implements this for the policy it
/// captured from the user's own configuration, before any project file was
/// merged. A flag, a label, a digest or a statement in a test is not a
/// policy. Outside tests nothing in this crate implements the trait yet, so
/// no selection can be made.
pub trait CapturedUserPolicy: Send + Sync {
    /// Whether the user's policy enables privacy for a conversation in the
    /// workspace with this digest, which is the one
    /// [`PrivacySession::workspace_sha256`] gives.
    fn enables_privacy_in(&self, workspace_sha256: &str) -> bool;

    /// Whether the policy requires that everything a provider is sent can be
    /// inspected. Native wires send back provider items that cannot.
    fn requires_inspectable_provider_items(&self) -> bool;
}

/// Why no session was built for a selection. Each is decided before the
/// session exists; none of them ends the conversation that was offered,
/// except where the text says so.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum BuildRefusal {
    #[error("The user's policy does not enable privacy for this workspace.")]
    PolicyDisabled,
    #[error("The privacy conversation has already ended.")]
    Ended,
    #[error(
        "A privacy conversation starts fresh: no caller thread id, separate guard or prepared runtime."
    )]
    NotFresh,
    #[error("A requested part of the session has no source path and may not be left out.")]
    RequestedPart,
    #[error(
        "The policy requires inspectable provider items, and this wire sends back items that cannot be inspected."
    )]
    OpaqueProviderItems,
    /// The conversation that was offered may have ended: a client whose own
    /// settings hold a supported value ends it.
    #[error("The model connection has no private view.")]
    Provider,
    #[error("The system text has no private view.")]
    SystemText,
    /// The installed parts that have no source path, by name.
    #[error("These parts of the session have no source path: {}.", .0.join(", "))]
    MissingSourcePaths(Vec<String>),
}

/// Where the session builder records why it refused a selection. The
/// builder's own result has no room for a reason.
#[derive(Default)]
pub struct SelectionReport {
    refusal: std::sync::Mutex<Option<BuildRefusal>>,
}

impl SelectionReport {
    /// The first refusal the builder recorded, if it refused.
    pub fn refusal(&self) -> Option<BuildRefusal> {
        self.refusal.lock().unwrap().clone()
    }

    pub(crate) fn refuse(&self, refusal: BuildRefusal) {
        self.refusal.lock().unwrap().get_or_insert(refusal);
    }
}

/// The selection the session builder takes for one live privacy
/// conversation: the user's captured policy and the conversation that was
/// opened under it. It cannot be built from configuration. It carries no
/// permission to run with fewer parts than the user configured: the builder
/// refuses a session in which any part has no source-preserving path, and
/// says which through [`SelectionReport`].
pub struct PrivacyViewSelection {
    policy: Arc<dyn CapturedUserPolicy>,
    session: Arc<PrivacySession>,
    report: Arc<SelectionReport>,
    #[cfg(test)]
    fixture_limited_set: bool,
}

impl PrivacyViewSelection {
    /// Refused when the policy does not enable privacy for the conversation's
    /// workspace, or the conversation has ended.
    pub fn from_user_policy(
        policy: Arc<dyn CapturedUserPolicy>,
        session: Arc<PrivacySession>,
    ) -> Result<Self, BuildRefusal> {
        if session.check().is_err() {
            return Err(BuildRefusal::Ended);
        }
        if !policy.enables_privacy_in(session.workspace_sha256()) {
            return Err(BuildRefusal::PolicyDisabled);
        }
        Ok(Self {
            policy,
            session,
            report: Arc::new(SelectionReport::default()),
            #[cfg(test)]
            fixture_limited_set: false,
        })
    }

    /// Where the builder records why it refused, for the host that made the
    /// selection.
    pub fn report(&self) -> Arc<SelectionReport> {
        self.report.clone()
    }

    /// Test fixtures only: a selection under a fixture policy that builds
    /// with the parts that have a source-preserving path and names the
    /// others to the frontend. Neither exists outside tests.
    #[cfg(test)]
    pub(crate) fn fixture_limited_set(session: Arc<PrivacySession>) -> Self {
        Self {
            fixture_limited_set: true,
            ..Self::from_user_policy(Arc::new(FixturePolicy::default()), session)
                .expect("a live fixture conversation")
        }
    }

    pub(crate) fn session(&self) -> &Arc<PrivacySession> {
        &self.session
    }

    /// Whether the policy still enables privacy for this conversation.
    pub(crate) fn policy_holds(&self) -> bool {
        self.policy
            .enables_privacy_in(self.session.workspace_sha256())
    }

    pub(crate) fn requires_inspectable_items(&self) -> bool {
        self.policy.requires_inspectable_provider_items()
    }

    /// Whether parts without a source-preserving path are left out and named
    /// instead of refusing the build. Never outside tests.
    pub(crate) fn leaves_out_unadapted_parts(&self) -> bool {
        #[cfg(test)]
        {
            self.fixture_limited_set
        }
        #[cfg(not(test))]
        {
            false
        }
    }
}

/// A policy for test fixtures. It is a fixed statement, not a user's policy,
/// and is compiled only into test builds.
#[cfg(test)]
pub(crate) struct FixturePolicy {
    pub(crate) disabled: bool,
    pub(crate) inspectable_items: bool,
}

#[cfg(test)]
impl Default for FixturePolicy {
    fn default() -> Self {
        Self {
            disabled: false,
            inspectable_items: false,
        }
    }
}

#[cfg(test)]
impl CapturedUserPolicy for FixturePolicy {
    fn enables_privacy_in(&self, _: &str) -> bool {
        !self.disabled
    }

    fn requires_inspectable_provider_items(&self) -> bool {
        self.inspectable_items
    }
}

/// The privacy conversation a session was built for, with the boundary its
/// agent selected.
pub struct PrivacyView {
    session: Arc<PrivacySession>,
    pub(crate) human_source: Option<Arc<crate::privacy_entry::HumanDecisionProducer>>,
    boundary: Arc<ConversationBoundary>,
}

impl PrivacyView {
    pub(crate) fn new(session: Arc<PrivacySession>, boundary: Arc<ConversationBoundary>) -> Self {
        Self {
            session,
            boundary,
            human_source: None,
        }
    }

    pub(crate) fn with_human_source(
        mut self,
        source: Arc<crate::privacy_entry::HumanDecisionProducer>,
    ) -> Self {
        self.human_source = Some(source);
        self
    }

    pub fn session(&self) -> &Arc<PrivacySession> {
        &self.session
    }

    pub fn boundary(&self) -> &Arc<ConversationBoundary> {
        &self.boundary
    }
}

/// What kind of producer a raw capture came from. It describes the source; it
/// is not a trust level and grants nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RawOrigin {
    /// Text the user is still composing. It has not been accepted.
    TerminalDraft,
    /// An answer, edit or feedback the user gave and the host accepted.
    HumanReply,
    /// The content a tool actually returned.
    ToolResult,
    /// A result the dispatch chain recorded for a call that is not the tool's
    /// own returned content: a refusal or denial on the way to the tool, a
    /// cut or reformatted outcome, or the text of a failure.
    ChainResult,
    /// A file or stored document the host read.
    HostFile,
    /// Configuration or policy text the host loaded.
    HostConfig,
    /// Fixed text built into the application.
    HostStatic,
}

struct RawInner {
    origin: RawOrigin,
    text: Zeroizing<String>,
}

/// One actual raw source, immutable once captured. Cloning shares the same
/// capture. It is raw: holding one proves nothing about its content, and it
/// can become a model view only through [`PrivacySession::admit_raw`].
#[derive(Clone)]
pub struct CapturedRaw {
    inner: Arc<RawInner>,
}

impl CapturedRaw {
    /// Called by the producer that actually took the text in, with the origin
    /// that producer really is. A capture is never derived from an admitted
    /// node's view or from a model reply.
    pub fn from_producer(origin: RawOrigin, text: &str) -> Self {
        Self {
            inner: Arc::new(RawInner {
                origin,
                text: Zeroizing::new(text.to_owned()),
            }),
        }
    }

    pub fn origin(&self) -> RawOrigin {
        self.inner.origin
    }

    /// Whether the host has already accepted this source as a fact. A draft
    /// can be refused and edited. An accepted fact cannot be refused: if it
    /// has no model view, the session stops and the fact stays as it is.
    pub fn is_accepted_fact(&self) -> bool {
        self.inner.origin != RawOrigin::TerminalDraft
    }

    fn same_capture(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

struct NodeInner {
    raw: CapturedRaw,
    /// The vault's own result for this capture, after its commit.
    masked: Arc<MaskedText>,
    replacements: usize,
    owner: Arc<Inner>,
}

/// One raw capture that this session admitted: the capture, the vault's
/// committed result for it and its model view. It exists only as the return
/// of a successful admission. Cloning shares the same node.
#[derive(Clone)]
pub struct AdmittedNode {
    inner: Arc<NodeInner>,
}

impl AdmittedNode {
    /// The text a model may see for this source, while the session is live.
    pub fn model_view(&self) -> Result<&str, SessionStop> {
        self.inner.owner.live()?;
        self.inner
            .masked
            .text_for_later_model_request()
            .map_err(|_| SessionStop::Closed)
    }

    pub fn origin(&self) -> RawOrigin {
        self.inner.raw.origin()
    }

    pub fn replacements(&self) -> usize {
        self.inner.replacements
    }

    /// Whether this node is the admission of exactly that capture.
    pub fn admits(&self, raw: &CapturedRaw) -> bool {
        self.inner.raw.same_capture(raw)
    }

    /// Whether this exact session admitted the node. An equal-looking session
    /// or node never qualifies.
    pub fn admitted_by(&self, session: &PrivacySession) -> bool {
        Arc::ptr_eq(&self.inner.owner, &session.inner)
    }

    /// The live scope and the records the vault returned for this node, for a
    /// host view of this one admitted source. Held for the callback only.
    pub fn with_records<T>(
        &self,
        read: impl FnOnce(&MappingSession, &ResolutionRecords) -> T,
    ) -> Result<T, SessionStop> {
        let owner = &self.inner.owner;
        owner.live()?;
        let records = self
            .inner
            .masked
            .resolution_records_for_host_fields()
            .map_err(|_| SessionStop::Closed)?;
        let result = read(&owner.session, records);
        owner.live()?;
        Ok(result)
    }
}

/// One mapping the conversation holds, with the committed result that
/// returned it. The record is read in place from that result.
struct Known {
    placeholder: String,
    masked: Arc<MaskedText>,
    index: usize,
}

/// Every mapping returned by this session's own successful commits. The scope
/// belongs to one fresh conversation, so nothing else has written to it.
struct Ledger {
    known: Vec<Known>,
}

impl Ledger {
    fn absorb(&mut self, masked: &Arc<MaskedText>) -> Result<(), PrivacyError> {
        let records = masked.resolution_records_for_host_fields()?;
        for index in 0..records.len()? {
            let placeholder =
                records.with_host_field(index, |field| field.known_alias().to_owned())?;
            if !self
                .known
                .iter()
                .any(|known| known.placeholder == placeholder)
            {
                self.known.push(Known {
                    placeholder,
                    masked: masked.clone(),
                    index,
                });
            }
        }
        Ok(())
    }
}

struct Inner {
    binding: FreshConversationBinding,
    session: Arc<MappingSession>,
    guard: Arc<SendGuard>,
    detector: Detector,
    /// One admission at a time. Held only around detector and vault work of
    /// that admission, never across a model request, a tool or a child task.
    operation: tokio::sync::Mutex<Ledger>,
    slots: tokio::sync::Semaphore,
    stopped: AtomicBool,
    // The most recent actual map return survives a late admission stop. It is
    // private retention, never a model node or permission to resolve a value.
    last_map_commit: std::sync::Mutex<Option<Arc<MaskedText>>>,
    /// Whether the one boundary of this conversation has been handed out.
    bound: AtomicBool,
    agent_owner: std::sync::Mutex<Option<craftsman_harness::provenance::ActualAgentOwner>>,
    #[cfg(test)]
    after_commit: std::sync::Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
}

impl Inner {
    fn live(&self) -> Result<(), SessionStop> {
        if self.stopped.load(Ordering::Acquire) || self.guard.check().is_err() {
            return Err(SessionStop::Closed);
        }
        Ok(())
    }

    fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
        self.guard.close();
        self.session.close();
    }

    fn stopped(&self, stop: SessionStop) -> AdmitError {
        self.stop();
        AdmitError::Stopped(stop)
    }
}

/// One conversation's privacy owner. Dropping or stopping it closes its send
/// guard and its vault scope, and the views of nodes it admitted can no
/// longer be read. It never reopens and is never rebound to another thread.
pub struct PrivacySession {
    inner: Arc<Inner>,
}

impl PrivacySession {
    /// Opens the reserved conversation's scope in an already open vault and
    /// binds the guard to it. The vault, its key and its location are the
    /// caller's; this function opens no file and reads no configuration. The
    /// detector is the host's selected one; its source is checked on every
    /// call it makes.
    pub async fn open(
        vault: &Vault,
        binding: FreshConversationBinding,
        detector: Detector,
    ) -> Result<Self, SessionStop> {
        let session = Arc::new(
            vault
                .session(binding.namespace()?)
                .map_err(SessionStop::Vault)?,
        );
        // An authentic, empty record set of this same live scope.
        let records = session
            .resolve_exact_aliases(&[])
            .await
            .map_err(SessionStop::Vault)?;
        let guard = SendGuard::from_vault(session.clone(), &records).map_err(SessionStop::Guard)?;
        Ok(Self {
            inner: Arc::new(Inner {
                binding,
                session,
                guard,
                detector,
                operation: tokio::sync::Mutex::new(Ledger { known: Vec::new() }),
                slots: tokio::sync::Semaphore::new(ADMISSION_SLOTS),
                stopped: AtomicBool::new(false),
                last_map_commit: std::sync::Mutex::new(None),
                bound: AtomicBool::new(false),
                agent_owner: std::sync::Mutex::new(None),
                #[cfg(test)]
                after_commit: std::sync::Mutex::new(None),
            }),
        })
    }

    /// The SHA-256 of the canonical workspace this conversation was reserved
    /// for, as lowercase hex.
    pub fn workspace_sha256(&self) -> &str {
        &self.inner.binding.workspace
    }

    /// The thread id this conversation was reserved with. The session builder
    /// of the same conversation must use this value and no other.
    pub fn thread_id(&self) -> &str {
        self.inner.binding.thread_id()
    }

    /// The guard every model client of this conversation must be bound to.
    pub fn send_guard(&self) -> Arc<SendGuard> {
        self.inner.guard.clone()
    }

    pub fn check(&self) -> Result<(), SessionStop> {
        self.inner.live()
    }

    pub(crate) fn check_agent(
        &self,
        owner: &craftsman_harness::provenance::ActualAgentOwner,
    ) -> Result<(), SessionStop> {
        self.inner.live()?;
        if !self
            .inner
            .agent_owner
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|held| held.same_agent(owner))
        {
            return Err(SessionStop::Unbound);
        }
        Ok(())
    }

    /// Ends the session: the guard refuses every later send and the scope
    /// refuses every later mapping. It does not retract a send or an effect
    /// that already happened, and it changes no recorded input or result.
    pub fn stop(&self) {
        self.inner.stop();
    }

    /// Admits one raw capture.
    ///
    /// On success the vault has committed the capture's mask, including a
    /// mask that found nothing, the guard knows every original, and the node
    /// carries the model view. `Refused` is returned only for a draft, before
    /// any effect, and leaves the session usable. For an accepted fact the
    /// same condition stops the session instead. Any detector, vault or guard
    /// failure stops the session. No outcome returns raw text as a view.
    pub async fn admit_raw(&self, raw: &CapturedRaw) -> Result<AdmittedNode, AdmitError> {
        let inner = &self.inner;
        // The one clock of this admission starts here, before any waiting.
        let deadline = Instant::now() + ADMISSION_BOUND;
        let refuse = |refusal: AdmitRefusal| {
            if raw.is_accepted_fact() {
                inner.stopped(SessionStop::Projection(refusal))
            } else {
                AdmitError::Refused(refusal)
            }
        };
        inner.live().map_err(AdmitError::Stopped)?;
        let Ok(_slot) = inner.slots.try_acquire() else {
            return Err(refuse(AdmitRefusal::Waiting));
        };
        let Ok(mut ledger) = tokio::time::timeout_at(deadline, inner.operation.lock()).await else {
            return Err(refuse(AdmitRefusal::Waiting));
        };
        inner.live().map_err(AdmitError::Stopped)?;
        let text = raw.inner.text.as_str();
        if text.len() > MAX_TEXT_BYTES {
            return Err(refuse(AdmitRefusal::TooLarge));
        }
        // An issued placeholder in raw text is ambiguous: it may be a value
        // of the user's that happens to look like one. Never guess.
        if ledger
            .known
            .iter()
            .any(|known| text.contains(known.placeholder.as_str()))
        {
            return Err(refuse(AdmitRefusal::IssuedPlaceholder));
        }
        let spans = match tokio::time::timeout_at(deadline, inner.detector.detect(text)).await {
            Err(_) => return Err(inner.stopped(SessionStop::Deadline)),
            Ok(Err(HookError::Input)) => return Err(refuse(AdmitRefusal::TooLarge)),
            Ok(Err(error)) => return Err(inner.stopped(SessionStop::Detector(error))),
            Ok(Ok(spans)) => spans,
        };
        inner.live().map_err(AdmitError::Stopped)?;
        preflight(text, &spans, &ledger).map_err(|error| match error {
            Preflight::Refused(refusal) => refuse(refusal),
            Preflight::Vault(error) => inner.stopped(SessionStop::Vault(error)),
        })?;
        if Instant::now() >= deadline {
            return Err(inner.stopped(SessionStop::Deadline));
        }
        let vault = |error| inner.stopped(SessionStop::Vault(error));
        let captured = inner.session.capture_text(text).map_err(vault)?;
        let report = DetectorReport {
            input_sha256: captured.input_sha256_for_host_detector().map_err(vault)?,
            spans: spans
                .iter()
                .map(|span| DetectorSpan {
                    start: span.start,
                    end: span.end,
                    category: span.category,
                })
                .collect(),
        };
        // The vault's own mask and commit, also when nothing was reported.
        let masked = Arc::new(
            inner
                .session
                .mask(captured, report, DeclaredIdentifiers::none())
                .await
                .map_err(vault)?,
        );
        *inner.last_map_commit.lock().unwrap() = Some(masked.clone());
        // Keep the actual completed map result even if its owner must stop
        // below. This is a retained commit, not a new admitted model node.
        ledger.absorb(&masked).map_err(vault)?;
        admission_deadline(deadline, Instant::now()).map_err(|stop| inner.stopped(stop))?;
        #[cfg(test)]
        if let Some(hook) = inner.after_commit.lock().unwrap().as_ref() {
            hook();
        }
        // The committed result is for this capture and no other.
        let digest: [u8; 32] = Sha256::digest(text.as_bytes()).into();
        if masked.input_sha256_for_host_validation().map_err(vault)? != digest {
            return Err(inner.stopped(SessionStop::Vault(PrivacyError::Integrity)));
        }
        // The guard learns the originals before anything is released.
        inner
            .guard
            .admit(masked.resolution_records_for_host_fields().map_err(vault)?)
            .map_err(|refusal| inner.stopped(SessionStop::Guard(refusal)))?;
        ledger.absorb(&masked).map_err(vault)?;
        // The guard's own verifier over the view about to be released.
        let view = masked.text_for_later_model_request().map_err(vault)?;
        inner
            .guard
            .seal(b"null".to_vec(), &[view])
            .map_err(|refusal| inner.stopped(SessionStop::Guard(refusal)))?;
        admission_deadline(deadline, Instant::now()).map_err(|stop| inner.stopped(stop))?;
        Ok(AdmittedNode {
            inner: Arc::new(NodeInner {
                raw: raw.clone(),
                masked,
                replacements: spans.len(),
                owner: inner.clone(),
            }),
        })
    }

    #[cfg(test)]
    pub(crate) fn after_commit(&self, hook: impl Fn() + Send + Sync + 'static) {
        *self.inner.after_commit.lock().unwrap() = Some(Box::new(hook));
    }

    #[cfg(test)]
    pub(crate) async fn known_values(&self) -> usize {
        self.inner.operation.lock().await.known.len()
    }
}

impl Drop for PrivacySession {
    fn drop(&mut self) {
        self.inner.stop();
    }
}

enum Preflight {
    Refused(AdmitRefusal),
    Vault(PrivacyError),
}

/// Everything the vault would refuse for the content of this text, decided
/// here from the text, the checked spans and the records of this session's
/// own commits.
fn preflight(text: &str, spans: &[TextSpan], ledger: &Ledger) -> Result<(), Preflight> {
    let refuse = |refusal| Err(Preflight::Refused(refusal));
    // A conservative bound on the model view.
    if text.len() + spans.len() * MAX_PLACEHOLDER_BYTES > MAX_TEXT_BYTES {
        return refuse(AdmitRefusal::TooLarge);
    }
    let covered = |value: &str| {
        text.match_indices(value).all(|(start, found)| {
            spans
                .iter()
                .any(|span| span.start <= start && span.end >= start + found.len())
        })
    };
    let mut values: Vec<(&str, Category)> = Vec::new();
    for span in spans {
        let value = &text[span.start..span.end];
        match values.iter().find(|(prior, _)| *prior == value) {
            Some((_, category)) if *category != span.category => {
                return refuse(AdmitRefusal::ConflictingCategory);
            }
            Some(_) => {}
            None => values.push((value, span.category)),
        }
    }
    // Every other occurrence of a reported value must be reported too.
    if values.iter().any(|(value, _)| !covered(value)) {
        return refuse(AdmitRefusal::UncoveredValue);
    }
    let mut reused = 0;
    for known in &ledger.known {
        // The original is read in place and never copied out.
        let verdict = known
            .masked
            .resolution_records_for_host_fields()
            .and_then(|records| {
                records.with_host_field(known.index, |field| {
                    let original = field.original_for_validated_host_field();
                    match values.iter().find(|(value, _)| *value == original) {
                        Some((_, category)) if *category != field.category() => {
                            Some(AdmitRefusal::ConflictingCategory)
                        }
                        Some(_) => {
                            reused += 1;
                            None
                        }
                        // A known value that was not reported here.
                        None if !covered(original) => Some(AdmitRefusal::UncoveredValue),
                        None => None,
                    }
                })
            })
            .map_err(Preflight::Vault)?;
        if let Some(refusal) = verdict {
            return refuse(refusal);
        }
    }
    if ledger.known.len() + values.len() - reused > SCOPE_MAPPINGS {
        return refuse(AdmitRefusal::ScopeFull);
    }
    Ok(())
}

macro_rules! private_debug {
    ($($type:ty),+ $(,)?) => {$(
        impl std::fmt::Debug for $type {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(concat!(stringify!($type), "([private])"))
            }
        }
    )+};
}

private_debug!(
    FreshConversationBinding,
    CapturedRaw,
    AdmittedNode,
    PrivacySession
);
