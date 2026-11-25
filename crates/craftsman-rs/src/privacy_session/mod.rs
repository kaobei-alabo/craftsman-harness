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
//!
//! A conversation is never rebound. When its settings change, it is retired
//! and its record is carried into a new conversation of the same thread,
//! which has a scope of its own and admits every original anew
//! (`view/adoption.rs`).

mod control;
#[cfg(test)]
mod deadline_tests;
mod file_data;
mod file_view;
// The retained directory of a file fixture where the target is not unix:
// on Windows its identity by handle.
#[cfg(all(test, not(unix)))]
pub(crate) mod fixture_directory;
mod human;
// Private lexical facts used by the full-image contact-data validator;
// parser facts themselves grant no source, policy or effect authority.
mod json_ranges;
#[cfg(test)]
mod tests;
mod transport;
mod view;
#[cfg(test)]
mod view_tests;
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

#[cfg(test)]
pub(crate) mod wire_fixtures;

pub(crate) use control::{SelectedControls, SelectedVerification};
pub use file_data::{
    FileDataRequest, FileEffectRequest, FileTargetPrecheck, FileTargetRequest, RetainedFileEffect,
    SupportedDataApplication, SupportedFileData,
};
pub(crate) use file_view::authorized_write;
#[cfg(test)]
pub(crate) use file_view::call_to_show;
pub use file_view::{
    DerivedFileOperation, FileChange, FilePreview, FileRefusal, FileReview, RestoredFileTool,
    ReviewedFile,
};
pub use human::{
    HeldStream, HumanRefusal, HumanReply, HumanReplySink, HumanText, ReplyDisplay, Streamed,
};
#[cfg(test)]
pub use transport::delegated_provider;
pub use transport::{ViewChatProvider, ViewNativeProvider};
pub use view::{
    ActualToolResult, ApprovedOperation, CommittedModelNode, ConversationBoundary, FieldPlace,
    NativeReplay, PlannedOperation, ReceivedModel, SourceField,
};
// Handed on where the owned terminal that carries a conversation is compiled.
#[cfg(unix)]
pub use view::RetiredConversation;

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
    /// How many scopes this thread had before this one. A reserved
    /// conversation has none. Only the retirement of a conversation issues
    /// the binding of its successor, which keeps the thread and is one
    /// further; see `RetiredConversation::successor`.
    generation: u32,
}

impl FreshConversationBinding {
    /// `workspace` is the session's absolute workspace root. It is resolved to
    /// its canonical path once; only that path's SHA-256 is kept.
    pub fn reserve(workspace: &Path) -> Result<Self, SessionStop> {
        if !workspace.is_absolute() {
            return Err(SessionStop::Scope);
        }
        let workspace = or_return!(std::fs::canonicalize(workspace), SessionStop::Scope);
        let mut digest = Sha256::new();
        digest.update(b"craftsman/privacy-session/workspace/v1\0");
        digest.update(workspace.as_os_str().as_encoded_bytes());
        Ok(Self {
            thread_id: LibsqlCheckpointer::new_thread_id(),
            workspace: format!("{:x}", digest.finalize()),
            generation: 0,
        })
    }

    /// The reserved thread id, for the one builder of this conversation.
    pub fn thread_id(&self) -> &str {
        &self.thread_id
    }

    /// Whether this binding was made for `workspace`: its canonical path
    /// has the digest this binding keeps.
    #[cfg(unix)]
    pub(crate) fn is_for(&self, workspace: &Path) -> bool {
        let Ok(workspace) = std::fs::canonicalize(workspace) else {
            return false;
        };
        let mut digest = Sha256::new();
        digest.update(b"craftsman/privacy-session/workspace/v1\0");
        digest.update(workspace.as_os_str().as_encoded_bytes());
        format!("{:x}", digest.finalize()) == self.workspace
    }

    /// A carried conversation keeps its thread and gets a scope of its own:
    /// the namespace names how many scopes came before it, so it is never
    /// the retired conversation's.
    fn namespace(&self) -> Result<HostNamespace, SessionStop> {
        let carried;
        HostNamespace::supplied_by_host(
            "craftsman/privacy-session/v1",
            &self.workspace,
            &self.thread_id,
            match self.generation {
                0 => "conversation",
                generation => {
                    carried = format!("conversation/carried-{generation}");
                    &carried
                }
            },
            "main+delegated+compaction/v1",
        )
        .map_err(|_| SessionStop::Scope)
    }
}

/// The user's captured privacy policy, as far as the session builder needs
/// it. The application's policy owner implements this for the policy it
/// captured from the user's own configuration, before any project file was
/// merged. A flag, a label, a digest or a statement in a test is not a
/// policy. The private captured key/user-root owner implements this trait;
/// its owned terminal entry is unselected by normal routes. Data-file format
/// proposals remain inert until the supported-field/effect guard exists.
pub trait CapturedUserPolicy: Send + Sync {
    /// Refusal-only actual target eligibility, before any private selector
    /// restoration/comparison. No image, effect or tool permission is granted.
    fn precheck_file_target(&self, _request: &FileTargetRequest) -> Option<FileTargetPrecheck> {
        None
    }
    /// Actual supported whole-object/schema/application/field validator record.
    /// A format/path proposal alone remains inert; absence is refusal.
    fn supported_file_data(&self, _request: &FileDataRequest) -> Option<SupportedFileData> {
        None
    }
    /// Actual retained user/root/destination provider; no ambient path or
    /// caller label grants a final effect. Absence remains refusal.
    fn retained_file_effect(&self, _request: &FileEffectRequest) -> Option<RetainedFileEffect> {
        None
    }
    /// Whether the user's policy enables privacy for a conversation in the
    /// workspace with this digest, which is the one
    /// [`PrivacySession::workspace_sha256`] gives.
    fn enables_privacy_in(&self, workspace_sha256: &str) -> bool;

    /// Whether the policy requires that everything a provider is sent can be
    /// inspected. Native wires send back provider items that cannot.
    fn requires_inspectable_provider_items(&self) -> bool;

    /// The data format the user's policy names for content that the file
    /// tools write to `destination`, in the workspace with this digest.
    /// `destination` is the absolute path of the file the operation is bound
    /// to, as the host resolved it when the operation was derived: the
    /// opened file for an edit, the observed directory and the new name for
    /// a create. It is never the path as a model wrote it. Private values
    /// are restored into a file's content only under such a rule. `None`,
    /// the default, restores nothing. A file's name or extension is not a
    /// rule: only what the user or the host reviewed for that file is.
    fn restorable_file_data(
        &self,
        workspace_sha256: &str,
        destination: &std::path::Path,
    ) -> Option<crate::privacy_restore::FileDataFormat> {
        let _ = (workspace_sha256, destination);
        None
    }
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

/// Where the session builder records why it refused a selection, and what
/// it accepted for one, at the place where it accepted each part. The
/// builder's own result has room for neither.
///
/// What it accepted is recorded as the objects themselves: the receipt that
/// each lane's view provider issued, the composed parts by the address of
/// the object the builder made or installed, and the middleware it kept.
/// Nothing here is a count, a name or a supported-feature flag.
#[derive(Default)]
pub struct SelectionReport {
    refusal: std::sync::Mutex<Option<BuildRefusal>>,
    accepted: std::sync::Mutex<Accepted>,
}

#[derive(Default)]
struct Accepted {
    /// The original request as the builder held it before it consumed
    /// anything of it. See `SessionExtras::original`.
    request: Option<[usize; crate::agent::REQUEST_FACTS]>,
    /// The receipt of each lane's view provider, kept weakly: the provider
    /// that issued it holds it, and it is gone when that provider is.
    lanes: [Option<std::sync::Weak<WireReceipt>>; 3],
    /// The consumers the builder constructed, kept weakly, in the places of
    /// [`consumer`]. What each holds is read from it, never from the
    /// request.
    consumers: [Option<std::sync::Weak<dyn Consumer>>; 3],
    /// Every tool the builder installed is the agent's own registry entry
    /// of that name.
    registered: bool,
    /// Whether the builder installed each lane's provider, said at the
    /// statement that installs it. An accepted provider that was never
    /// installed anywhere may be gone; an installed one may not.
    installed: [usize; 3],
    /// The address or number of each composed part, in the order of
    /// [`ComposedPart`], and which of them the builder stated, as bits in
    /// that order. A part stated as zero is not a part never stated.
    parts: [usize; COMPOSED_PARTS],
    stated: usize,
    /// The builder decided about the controller adapters.
    controllers: bool,
    /// The choices of the request the builder acted on, each set at the
    /// place where it did, as the bits of the request's own choices.
    produced: usize,
    /// Every middleware the builder kept, after it found that each has a
    /// source path.
    middleware: Option<Vec<std::sync::Weak<dyn craftsman_harness::AgentMiddleware>>>,
}

/// A consumer the builder constructed for a selection: the approval gate,
/// the question tool or the reply display. The record keeps a weak
/// reference to the consumer itself and reads from it what it holds of the
/// request's owners, by address and zero for none. A statement about the
/// request cannot stand in for this.
pub(crate) trait Consumer: Send + Sync {
    // Read only by the unix-only check of a built session.
    #[cfg(unix)]
    fn held(&self) -> [usize; 5];
}

/// The places of the constructed consumers in the record.
pub(crate) mod consumer {
    pub(crate) const APPROVAL: usize = 0;
    pub(crate) const QUESTION: usize = 1;
    pub(crate) const DISPLAY: usize = 2;
}

/// The address of the object an optional owner holds, zero for none.
macro_rules! held_address {
    ($owner:expr) => {
        match $owner {
            Some(owner) => &**owner as *const _ as *const () as usize,
            None => 0,
        }
    };
}
pub(crate) use held_address;

/// The model lanes of a session: the root agent's, the delegated agents'
/// and the summarizer's.
#[derive(Clone, Copy)]
pub(crate) enum ModelLane {
    Main,
    Delegated,
    Summaries,
}

/// What the builder composes or installs for a selection besides its
/// providers. The first six are objects it makes; the others are the owners
/// of the original request, recorded where the builder installs each.
#[derive(Clone, Copy)]
pub(crate) enum ComposedPart {
    /// The review of file writes and edits.
    FileReview,
    /// The adapter around the configured verification controller.
    Verification,
    /// The adapter around the configured context and failure controls.
    Controls,
    /// The verification controller that adapter was made for.
    VerificationOriginal,
    /// The context and failure controls that adapter was made for.
    ControlsOriginal,
    /// The quiet sink the builder makes for the delegated and the summary
    /// lane. Each of those providers says in its receipt what it was given.
    ChildSink,
    /// The workflow middleware, where the builder made one.
    Workflow,
    /// How many extra tools were installed, and the sum of their addresses.
    /// Both are always stated; no extra tool is stated as zero.
    ExtraTools,
    ExtraToolIdentities,
}
pub(crate) const COMPOSED_PARTS: usize = 9;

/// One thing the builder accepted, said at the place where it took it.
pub(crate) enum Accept<'a> {
    /// The original request, before anything of it was consumed.
    Request([usize; crate::agent::REQUEST_FACTS]),
    /// For this lane, the view provider that issued this receipt.
    Lane(ModelLane, &'a Arc<WireReceipt>),
    /// For this lane the provider at this address is installed, here.
    Bound(ModelLane, usize),
    /// This constructed consumer, in this place of [`consumer`].
    Consumer(usize, Arc<dyn Consumer>),
    /// Every tool the builder installed was compared with the agent's own
    /// registry entry of its name and is that entry.
    Registered,
    /// As this part, the object at this address, which the builder made or
    /// installed.
    Part(ComposedPart, usize),
    /// The decision about the controller adapters; the installed ones are
    /// said as parts.
    Controllers,
    /// The builder acted on this choice of the request, here.
    Produced(usize),
    /// Exactly these middleware, each with a source path.
    Middleware(&'a [Arc<dyn craftsman_harness::AgentMiddleware>]),
}

/// Issued by a view provider when it has bound a conversation's guard: the
/// receipt of that one binding. Only the two view providers make one, each
/// in its constructor, and each keeps its own for as long as it lives,
/// through every later wrapping. It holds the conversation and the very
/// guard that provider's client was bound to, and says which of the two
/// providers issued it: the native one sends back provider items that
/// cannot be inspected. When the builder accepts the provider for a lane,
/// the receipt is marked with that selection and lane.
pub struct WireReceipt {
    // Read only by the unix-only check of a built session.
    #[cfg(unix)]
    pub(super) session: Arc<PrivacySession>,
    #[cfg(unix)]
    pub(super) guard: Arc<SendGuard>,
    #[cfg(unix)]
    pub(super) native: bool,
    /// The address of the selection's report that accepted this provider,
    /// with the lane in its low bits; zero until then.
    pub(super) accepted: std::sync::atomic::AtomicUsize,
    /// Where the provider that issued this receipt is, behind its one
    /// shared reference; zero until it was put there. The provider says
    /// it itself (`shared`).
    pub(super) at: std::sync::atomic::AtomicUsize,
    /// The sinks this provider was given, by address and zero for none: the
    /// token sink in its constructor, the round sink and the status sink
    /// where it takes them. The last place says which of the three it was
    /// given at all, as bits in that order: a sink given as none is not a
    /// sink never given. The provider states all of it itself.
    pub(super) sinks: [std::sync::atomic::AtomicUsize; 4],
}

impl SelectionReport {
    /// The first refusal the builder recorded, if it refused.
    pub fn refusal(&self) -> Option<BuildRefusal> {
        self.refusal.lock().unwrap().clone()
    }

    pub(crate) fn refuse(&self, refusal: BuildRefusal) {
        self.refusal.lock().unwrap().get_or_insert(refusal);
    }

    /// The builder says what it accepted. The request, each lane, each part
    /// and the middleware are said once: a second, different statement of
    /// one of them is kept out of the record and refuses the selection.
    pub(crate) fn accept(&self, accepted: Accept<'_>) {
        let mut record = self.accepted.lock().unwrap();
        let mut again = false;
        match accepted {
            Accept::Request(request) => match &record.request {
                Some(held) => again = *held != request,
                None => record.request = Some(request),
            },
            Accept::Lane(lane, receipt) => match &record.lanes[lane as usize] {
                Some(held) => again = held.as_ptr() != Arc::as_ptr(receipt),
                None => {
                    // The receipt itself now names this selection and lane.
                    receipt.accepted.store(
                        self as *const Self as usize | lane as usize,
                        Ordering::Release,
                    );
                    record.lanes[lane as usize] = Some(Arc::downgrade(receipt));
                }
            },
            Accept::Bound(lane, address) => match record.installed[lane as usize] {
                0 => record.installed[lane as usize] = address,
                held => again = held != address,
            },
            Accept::Consumer(place, consumer) => match &record.consumers[place] {
                Some(_) => again = true,
                None => record.consumers[place] = Some(Arc::downgrade(&consumer)),
            },
            Accept::Registered => record.registered = true,
            Accept::Part(part, address) => {
                let bit = 1 << part as usize;
                if record.stated & bit == 0 {
                    record.stated |= bit;
                    record.parts[part as usize] = address;
                } else {
                    again = record.parts[part as usize] != address;
                }
            }
            Accept::Controllers => record.controllers |= true,
            Accept::Produced(choice) => record.produced |= 1 << choice,
            Accept::Middleware(kept) => match &record.middleware {
                Some(_) => again = true,
                None => {
                    let mut held = Vec::with_capacity(kept.len());
                    for entry in kept {
                        held.push(Arc::downgrade(entry));
                    }
                    record.middleware = Some(held);
                }
            },
        }
        drop(record);
        if again {
            self.refuse(BuildRefusal::RequestedPart);
        }
    }

    /// For the host that made the selection: what the builder accepted is
    /// what `built` was built from, for the request that was made.
    /// - **The request.** The builder built from the very request the host
    ///   made (`request`).
    /// - **The consumers.** The approval gate, the question tool and the
    ///   reply display that the builder constructed are alive, and each
    ///   holds the request's own prompts and sinks: read from the consumer,
    ///   not from the request. The approval gate is one of the kept
    ///   middleware; the display is the one the boundary holds.
    /// - **The lanes.** The builder accepted a provider for each of the three
    ///   lanes, and installed the root agent's. Every provider it installed
    ///   is alive: an installed provider that was dropped or replaced leaves
    ///   its receipt unavailable, and that refuses. Only an accepted provider
    ///   that was installed nowhere may be gone. Each live receipt holds this
    ///   conversation and its guard and was accepted by this selection for
    ///   that lane. None is native where the policy requires inspectable
    ///   items. Each states the sinks it was given and which it was given
    ///   at all: the root agent's the display's own, or the request's where
    ///   there is no display, with the activity sink where that provider
    ///   takes one; the other two the quiet sink the builder made and no
    ///   other.
    /// - **The owners.** The cancellation owner installed on the agent is
    ///   the one the host handed over and still holds (`cancellation`), or
    ///   none where none was requested. The delegated agents' tool gates
    ///   hold the request's own owners, as each constructed gate said.
    /// - **The tools.** Every tool the builder installed is the agent's own
    ///   registry entry of its name. The gadget runtime the built session
    ///   holds exists exactly where the builder says it made one, and what
    ///   that runtime itself holds as allowed and trusted is what the
    ///   request allowed and trusted.
    /// - **The choices.** The builder acted on each choice of the request at
    ///   its own place, after its work succeeded: exactly the requested ones
    ///   where the choice alone decides, and no unrequested one where the
    ///   configuration decides too. A controller or a workflow part exists
    ///   only for a requested choice. No output owner, send guard or script
    ///   capture was requested; a privacy conversation has no place that
    ///   installs them.
    /// - **The parts.** The file review and the controller adapters are the
    ///   objects the boundary holds, and it reports an adapter only after
    ///   the agent's installed controllers were found to fit it; an adapter
    ///   exists exactly for each controller the builder made, and those
    ///   controllers are the built session's own.
    /// - **The middleware.** Every kept middleware is still there and still
    ///   states a source path itself.
    #[cfg(unix)]
    pub(crate) fn holds(
        &self,
        built: &crate::agent::Session,
        session: &Arc<PrivacySession>,
        inspectable: bool,
        request: &[usize; crate::agent::REQUEST_FACTS],
        cancellation: Option<&craftsman_harness::SessionCancellation>,
    ) -> Result<(), BuildRefusal> {
        use crate::agent::request_fact::{self as fact, choice};
        use craftsman_harness::provenance::ViewSupport;
        let Some(view) = &built.privacy_view else {
            return Err(BuildRefusal::RequestedPart);
        };
        // A statement the record refused to take twice refuses here too.
        if self.refusal.lock().unwrap().is_some() {
            return Err(BuildRefusal::RequestedPart);
        }
        let record = self.accepted.lock().unwrap();
        let part = &record.parts;
        let [replies, files, verification, controls] = view.boundary().installed();
        // What each constructed consumer itself holds, read from it while it
        // is alive. One that was constructed and is gone refuses.
        let mut held = [[0usize; 5]; 3];
        let mut at = [0usize; 3];
        let mut place = 0;
        while place < record.consumers.len() {
            if let Some(kept) = &record.consumers[place] {
                let Some(live) = kept.upgrade() else {
                    return Err(BuildRefusal::RequestedPart);
                };
                held[place] = live.held();
                at[place] = Arc::as_ptr(&live) as *const () as usize;
            }
            place += 1;
        }
        let gate = held[consumer::APPROVAL];
        let display = held[consumer::DISPLAY];
        let guard = session.send_guard();
        let mut lane = 0;
        while lane < record.lanes.len() {
            let receipt = match &record.lanes[lane] {
                Some(receipt) => receipt.upgrade(),
                // A lane the builder never accepted a provider for.
                None => return Err(BuildRefusal::RequestedPart),
            };
            match receipt {
                Some(receipt) => {
                    if !Arc::ptr_eq(&receipt.session, session)
                        || !Arc::ptr_eq(&receipt.guard, &guard)
                        || receipt.accepted.load(Ordering::Acquire)
                            != (self as *const Self as usize | lane)
                    {
                        return Err(BuildRefusal::RequestedPart);
                    }
                    if inspectable && receipt.native {
                        return Err(BuildRefusal::OpaqueProviderItems);
                    }
                    // What is installed for this lane is the very provider
                    // that issued the receipt. A wrapper in front of it,
                    // which a request with an output owner would get, is
                    // another object. The root agent's is always installed.
                    let installed = record.installed[lane];
                    if (installed == 0 && lane == ModelLane::Main as usize)
                        || (installed != 0 && installed != receipt.at.load(Ordering::Acquire))
                    {
                        return Err(BuildRefusal::RequestedPart);
                    }
                    // The sinks this provider says it was given, and which
                    // of them it was given at all.
                    let given = [
                        receipt.sinks[0].load(Ordering::Acquire),
                        receipt.sinks[1].load(Ordering::Acquire),
                        receipt.sinks[2].load(Ordering::Acquire),
                        receipt.sinks[3].load(Ordering::Acquire),
                    ];
                    // The root agent's provider is given a round sink, and
                    // a status sink where it is not the native one, which
                    // takes none. The other two are given a token sink only.
                    let expected = if lane != ModelLane::Main as usize {
                        [part[ComposedPart::ChildSink as usize], 0, 0, 1]
                    } else {
                        let (token, round) = match replies {
                            0 => (request[fact::TOKEN_SINK], request[fact::ROUND_SINK]),
                            _ => (display[1], display[2]),
                        };
                        match receipt.native {
                            true => [token, round, 0, 3],
                            false => [token, round, request[fact::ACTIVITY_SINK], 7],
                        }
                    };
                    if given[0] != expected[0]
                        || given[1] != expected[1]
                        || given[2] != expected[2]
                        || given[3] != expected[3]
                        || given[0] == 0
                    {
                        return Err(BuildRefusal::RequestedPart);
                    }
                }
                // Installed and gone. The root agent's is always installed.
                None if record.installed[lane] != 0 || lane == ModelLane::Main as usize => {
                    return Err(BuildRefusal::RequestedPart);
                }
                // Accepted, installed nowhere, and gone with the builder.
                None => {}
            }
            lane += 1;
        }
        let Some(kept) = &record.middleware else {
            return Err(BuildRefusal::RequestedPart);
        };
        // The approval gate and the workflow part, where there is one, are
        // among the kept middleware by object.
        let mut gate_kept = false;
        let mut workflow_kept = part[ComposedPart::Workflow as usize] == 0;
        let mut index = 0;
        while index < kept.len() {
            let stated = match kept[index].upgrade() {
                Some(entry) => matches!(
                    entry.view_support(),
                    ViewSupport::Preserving | ViewSupport::Tracked
                ),
                None => false,
            };
            if !stated {
                return Err(BuildRefusal::RequestedPart);
            }
            let address = kept[index].as_ptr() as *const () as usize;
            gate_kept |= address == at[consumer::APPROVAL];
            workflow_kept |= address == part[ComposedPart::Workflow as usize];
            index += 1;
        }
        let verifier = match &built.verification {
            Some(controller) => Arc::as_ptr(controller) as usize,
            None => 0,
        };
        let controller = match &built.controls {
            Some(controller) => Arc::as_ptr(controller) as usize,
            None => 0,
        };
        // Where the choice alone decides, the builder acted exactly on the
        // requested ones. Where the user's configuration decides too
        // (gadgets), it acted on no unrequested one.
        let alone = (1 << choice::INTERRUPT)
            | (1 << choice::ROUND_LIMITS)
            | (1 << choice::MEMORY)
            | (1 << choice::WORKFLOW)
            | (1 << choice::VERIFICATION)
            | (1 << choice::CONTEXT)
            | (1 << choice::FAILURE)
            | (1 << choice::CANCELLATION);
        let never =
            (1 << choice::OUTPUT_GUARD) | (1 << choice::SEND_GUARD) | (1 << choice::SCRIPT_CAPTURE);
        let choices = request[fact::CHOICES];
        if record.produced & alone != choices & alone
            || record.produced & !choices != 0
            || choices & never != 0
            || choices & (1 << choice::WORKSPACE_ROOT) == 0
            || request[fact::OUTPUT_CLOSED] != 0
            // The parts a choice produces exist only for that choice.
            || (verifier != 0 && choices & (1 << choice::VERIFICATION) == 0)
            || (controller != 0
                && choices & ((1 << choice::CONTEXT) | (1 << choice::FAILURE)) == 0)
            || (part[ComposedPart::Workflow as usize] != 0)
                != (choices & (1 << choice::WORKFLOW) != 0)
            || !workflow_kept
            // The cancellation owner on the agent is the host's own.
            || cancellation.is_some() != (choices & (1 << choice::CANCELLATION) != 0)
            || !built.agent.cancellation_session_is(cancellation)
            || part[ComposedPart::ExtraTools as usize] != request[fact::EXTRA_TOOLS]
            || part[ComposedPart::ExtraToolIdentities as usize]
                != request[fact::EXTRA_TOOL_IDENTITIES]
            || !record.registered
        {
            return Err(BuildRefusal::RequestedPart);
        }
        // The parts every selection states, zero or not; the workflow part
        // is stated exactly where there is one.
        let always = (1 << ComposedPart::FileReview as usize)
            | (1 << ComposedPart::ChildSink as usize)
            | (1 << ComposedPart::ExtraTools as usize)
            | (1 << ComposedPart::ExtraToolIdentities as usize);
        if record.stated & always != always
            || (record.stated & (1 << ComposedPart::Workflow as usize) != 0)
                != (part[ComposedPart::Workflow as usize] != 0)
        {
            return Err(BuildRefusal::RequestedPart);
        }
        // The gadget runtime of the built session: it exists exactly where
        // the builder says it made one, and what the runtime itself holds
        // as allowed and as trusted is what the request allowed and
        // trusted. Read from the runtime, not from a statement.
        #[cfg(feature = "gadgets")]
        match &built.gadgets {
            Some(runtime) => {
                let (allowed, trusted) = runtime.allowance();
                if record.produced & (1 << choice::LOCAL_GADGETS) == 0
                    || allowed != request[fact::ALLOWED_GADGETS]
                    || trusted != (choices & (1 << choice::TRUST_WORKSPACE) != 0)
                {
                    return Err(BuildRefusal::RequestedPart);
                }
            }
            None => {
                if record.produced & (1 << choice::LOCAL_GADGETS) != 0 {
                    return Err(BuildRefusal::RequestedPart);
                }
            }
        }
        // What the delegated agents' tool gates hold, as each constructed
        // gate said to this conversation's boundary; none where no gate was.
        if let Some(gates) = view.boundary().delegated_gates()
            && (gates[0] != request[fact::APPROVAL_PROMPT]
                || gates[1] != request[fact::DELEGATED_TOOL_OWNERS]
                || gates[2] != request[fact::DELEGATED_TOOL_OWNERS + 1]
                || gates[3] != request[fact::PRIVACY_PROMPT])
        {
            return Err(BuildRefusal::RequestedPart);
        }
        if record.request.as_ref() != Some(request)
            || !record.controllers
            // The consumers hold the request's own owners.
            || at[consumer::APPROVAL] == 0
            || !gate_kept
            || gate[0] != request[fact::ACTIVITY_SINK]
            || gate[1] != request[fact::TOOL_ACTIVITY_SINK]
            || gate[2] != request[fact::APPROVAL_PROMPT]
            || gate[3] != request[fact::CALL_PROMPT]
            || gate[4] != request[fact::PRIVACY_PROMPT]
            || at[consumer::QUESTION] == 0
            || held[consumer::QUESTION][0] != request[fact::ASK_PROMPT]
            || at[consumer::DISPLAY] != replies
            || (replies != 0) != (request[fact::REPLY_SINK] != 0)
            || display[0] != request[fact::REPLY_SINK]
            || (replies != 0 && display[3] != request[fact::ROUND_SINK])
            || files == 0
            || part[ComposedPart::FileReview as usize] != files
            || part[ComposedPart::Verification as usize] != verification
            || part[ComposedPart::Controls as usize] != controls
            || part[ComposedPart::VerificationOriginal as usize] != verifier
            || part[ComposedPart::ControlsOriginal as usize] != controller
            || (verification != 0) != (verifier != 0)
            || (controls != 0) != (controller != 0)
        {
            return Err(BuildRefusal::RequestedPart);
        }
        Ok(())
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
        })
    }

    /// Where the builder records why it refused, for the host that made the
    /// selection.
    pub fn report(&self) -> Arc<SelectionReport> {
        self.report.clone()
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

    /// The user's captured policy, for the parts of the session that ask it
    /// about one operation.
    pub(crate) fn policy(&self) -> Arc<dyn CapturedUserPolicy> {
        self.policy.clone()
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
    /// The user's captured policy, kept here for the life of the session
    /// that was built for it. The file review and the file tools refer to
    /// it weakly, so that nothing that outlives the session keeps the
    /// policy's owner alive.
    policy: Option<Arc<dyn CapturedUserPolicy>>,
}

impl PrivacyView {
    pub(crate) fn new(session: Arc<PrivacySession>, boundary: Arc<ConversationBoundary>) -> Self {
        Self {
            session,
            boundary,
            human_source: None,
            policy: None,
        }
    }

    /// Same actual view/lane/input owner for the owned terminal's task.
    /// This clones only existing owners; it mints no source, policy or agent.
    pub(crate) fn clone_for_terminal(&self) -> Self {
        Self {
            session: self.session.clone(),
            human_source: self.human_source.clone(),
            boundary: self.boundary.clone(),
            policy: self.policy.clone(),
        }
    }

    pub(crate) fn with_policy(mut self, policy: Option<Arc<dyn CapturedUserPolicy>>) -> Self {
        self.policy = policy;
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
    /// One private value that the retired conversation of the same thread
    /// held, on its own, with the category that conversation's vault scope
    /// recorded for it. Made only while a retired record is adopted.
    RetiredValue,
}

struct RawInner {
    origin: RawOrigin,
    text: Zeroizing<String>,
    /// The category of a retired value. The whole text is that one value,
    /// and admission takes the recorded category instead of asking the
    /// detector about a value out of its context.
    known: Option<Category>,
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
                known: None,
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
        let records = or_return!(
            self.inner.masked.resolution_records_for_host_fields(),
            SessionStop::Closed
        );
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
    inline_issuers:
        std::sync::Mutex<Vec<std::sync::Weak<crate::privacy_entry::InlineDecisionProducer>>>,
    binding: FreshConversationBinding,
    session: Arc<MappingSession>,
    guard: Arc<SendGuard>,
    /// The host's selected detector, until the conversation is released. An
    /// admission takes its own reference for its one call.
    detector: std::sync::Mutex<Option<Arc<Detector>>>,
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
    lanes: std::sync::Mutex<Vec<std::sync::Weak<view::Lane>>>,
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
    pub(crate) fn register_inline_issuer(
        &self,
        issuer: &Arc<crate::privacy_entry::InlineDecisionProducer>,
    ) {
        let mut issuers = self.inner.inline_issuers.lock().unwrap();
        issuers.retain(|issuer| issuer.strong_count() != 0);
        issuers.push(Arc::downgrade(issuer));
    }
    pub(crate) fn owns_inline_issuer(
        &self,
        issuer: &Arc<crate::privacy_entry::InlineDecisionProducer>,
    ) -> bool {
        self.inner
            .inline_issuers
            .lock()
            .unwrap()
            .iter()
            .filter_map(|issuer| issuer.upgrade())
            .any(|registered| Arc::ptr_eq(&registered, issuer))
    }
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
                inline_issuers: std::sync::Mutex::new(Vec::new()),
                binding,
                session,
                guard,
                detector: std::sync::Mutex::new(Some(Arc::new(detector))),
                operation: tokio::sync::Mutex::new(Ledger { known: Vec::new() }),
                slots: tokio::sync::Semaphore::new(ADMISSION_SLOTS),
                stopped: AtomicBool::new(false),
                last_map_commit: std::sync::Mutex::new(None),
                bound: AtomicBool::new(false),
                agent_owner: std::sync::Mutex::new(None),
                lanes: std::sync::Mutex::new(Vec::new()),
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

    /// Stops the conversation and gives up what it holds beyond its record:
    /// the detector, the last map return it kept, and its own references to
    /// the masked texts of its mappings. For a conversation whose owner
    /// closes for good; the send guard lets its originals go here as well.
    ///
    /// Objects that are still referenced stay what they are: the nodes of
    /// recorded messages keep their captures and views, which are the
    /// record's provenance, and this conversation keeps its identity for
    /// them. Stored mappings are not touched. An admission that is running
    /// at this moment holds the record of mappings until it returns, and
    /// that record is then not cleared.
    pub fn release(&self) {
        let inner = &self.inner;
        inner.stop();
        inner.guard.release();
        if let Ok(mut detector) = inner.detector.lock() {
            *detector = None;
        }
        if let Ok(mut kept) = inner.last_map_commit.lock() {
            *kept = None;
        }
        if let Ok(mut ledger) = inner.operation.try_lock() {
            ledger.known = Vec::new();
        }
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
        let (whole, detected);
        let spans: &[TextSpan] = match raw.inner.known {
            // A retired value on its own: the one span is the whole text.
            Some(category) => {
                whole = [TextSpan {
                    start: 0,
                    end: text.len(),
                    category,
                }];
                &whole
            }
            None => {
                // This admission's own reference to the detector. A released
                // conversation has none.
                let detector = match inner.detector.lock() {
                    Ok(detector) => detector.clone(),
                    Err(_) => None,
                };
                let Some(detector) = detector else {
                    return Err(inner.stopped(SessionStop::Closed));
                };
                detected = match tokio::time::timeout_at(deadline, detector.detect(text)).await {
                    Err(_) => return Err(inner.stopped(SessionStop::Deadline)),
                    Ok(Err(HookError::Input)) => return Err(refuse(AdmitRefusal::TooLarge)),
                    Ok(Err(error)) => return Err(inner.stopped(SessionStop::Detector(error))),
                    Ok(Ok(spans)) => spans,
                };
                &detected
            }
        };
        inner.live().map_err(AdmitError::Stopped)?;
        preflight(text, spans, &ledger).map_err(|error| match error {
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
        // The guard's own verifier over the view about to be released.
        let view = masked.text_for_later_model_request().map_err(vault)?;
        inner
            .guard
            .seal(b"null".to_vec(), &[view])
            .map_err(|refusal| inner.stopped(SessionStop::Guard(refusal)))?;
        let node = AdmittedNode {
            inner: Arc::new(NodeInner {
                raw: raw.clone(),
                masked,
                replacements: spans.len(),
                owner: inner.clone(),
            }),
        };
        admission_deadline(deadline, Instant::now()).map_err(|stop| inner.stopped(stop))?;
        Ok(node)
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
