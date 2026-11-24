//! Event witnesses issued by the agent, never reconstructed from a checkpoint
//! or a caller's mutable state. They describe retention and invocation only;
//! the host still validates its own source, transport and permission owners.

use crate::{AgentError, AgentState, HitlAction, ModelResponse, Msg, ToolCall, ToolOutcome, Usage};
use std::sync::{Arc, Mutex, OnceLock};

use super::{FrameworkCommit, ReachedTool, ReplyEdit, SourcePart, SourceRef};

#[derive(Clone)]
pub struct ActualAgentOwner(Arc<()>);

/// The controller objects actually installed by this agent's builder and
/// their captured source declarations. Feasibility only, not permission or
/// callback authority. No public issuer, JSON or state-based reconstruction.
#[derive(Clone)]
pub struct ActualControllerBindings {
    owner: ActualAgentOwner,
    policy: Option<Arc<dyn crate::AgentBoundaryPolicy>>,
    completion: Option<Arc<dyn crate::CompletionCheck>>,
    policy_source: Option<SourceRef>,
    completion_source: Option<SourceRef>,
}
impl ActualControllerBindings {
    pub(crate) fn issued(
        owner: &ActualAgentOwner,
        policy: Option<Arc<dyn crate::AgentBoundaryPolicy>>,
        completion: Option<Arc<dyn crate::CompletionCheck>>,
    ) -> Self {
        let policy_source = policy.as_ref().and_then(|policy| policy.source_for_view());
        let completion_source = completion
            .as_ref()
            .and_then(|policy| policy.source_for_view());
        Self {
            owner: owner.clone(),
            policy,
            completion,
            policy_source,
            completion_source,
        }
    }
    pub fn owner(&self) -> &ActualAgentOwner {
        &self.owner
    }
    pub fn has_policy(&self) -> bool {
        self.policy.is_some()
    }
    pub fn has_completion(&self) -> bool {
        self.completion.is_some()
    }
    pub fn policy_source(&self) -> Option<&SourceRef> {
        self.policy_source.as_ref()
    }
    pub fn completion_source(&self) -> Option<&SourceRef> {
        self.completion_source.as_ref()
    }
    pub fn policy_matches(&self, policy: &Arc<dyn crate::AgentBoundaryPolicy>) -> bool {
        self.policy
            .as_ref()
            .is_some_and(|held| Arc::ptr_eq(held, policy))
    }
    pub fn completion_matches(&self, completion: &Arc<dyn crate::CompletionCheck>) -> bool {
        self.completion
            .as_ref()
            .is_some_and(|held| Arc::ptr_eq(held, completion))
    }
    pub(crate) fn feasible(&self) -> bool {
        self.policy.as_ref().is_none_or(|policy| {
            policy.view_support() == super::ViewSupport::Tracked && self.policy_source.is_some()
        }) && self
            .completion
            .as_ref()
            .is_none_or(|_| self.completion_source.is_some())
    }
}

/// One actual dispatch proposal, issued before its approval/review chain.
/// Equal call bytes cannot recreate the event. A plan is neither permission
/// nor proof that a terminal ran; the final invocation retains it separately.
#[derive(Clone)]
pub struct ActualPlannedInvocation(Arc<PlannedInvocation>);
struct PlannedInvocation {
    owner: ActualAgentOwner,
    call: ToolCall,
    host: Option<ActualHostScheduledCall>,
    reviewing: Mutex<bool>,
    live: OnceLock<Arc<dyn Fn() -> Result<(), AgentError> + Send + Sync>>,
}
impl ActualPlannedInvocation {
    pub(crate) fn issued(owner: &ActualAgentOwner, call: ToolCall) -> Self {
        Self(Arc::new(PlannedInvocation {
            owner: owner.clone(),
            call,
            host: None,
            reviewing: Mutex::new(true),
            live: OnceLock::new(),
        }))
    }
    pub fn owner(&self) -> &ActualAgentOwner {
        &self.0.owner
    }
    pub fn call(&self) -> &ToolCall {
        &self.0.call
    }
    pub fn same_plan(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    pub(crate) fn host_issued(scheduled: ActualHostScheduledCall) -> Self {
        Self(Arc::new(PlannedInvocation {
            owner: scheduled.owner().clone(),
            call: scheduled.call().clone(),
            host: Some(scheduled),
            reviewing: Mutex::new(true),
            live: OnceLock::new(),
        }))
    }
    pub fn host_scheduled(&self) -> Option<&ActualHostScheduledCall> {
        self.0.host.as_ref()
    }
    /// Actual pre-review/chain liveness; data only. Private app producers
    /// still validate their registration, source and once-only use.
    pub fn in_review(&self) -> bool {
        *self.0.reviewing.lock().unwrap()
    }
    /// Actual framework activity/cancellation owner, not a caller status or
    /// permission. Direct data-only test events have no callable live owner.
    pub fn check_live(&self) -> Result<(), AgentError> {
        if !self.in_review() {
            return Err(AgentError::State("This planned review has ended.".into()));
        }
        self.0
            .live
            .get()
            .ok_or_else(|| AgentError::State("This review has no actual activity owner.".into()))?(
        )
    }
    pub(crate) fn bind_live(
        &self,
        live: Arc<dyn Fn() -> Result<(), AgentError> + Send + Sync>,
    ) -> Result<(), AgentError> {
        self.0.live.set(live).map_err(|_| {
            AgentError::State("This review already has its actual activity owner.".into())
        })
    }
    pub(crate) fn own_review(&self) -> PlannedReviewOwnership {
        PlannedReviewOwnership(self.clone())
    }
    pub(crate) fn seal_review(&self) {
        *self.0.reviewing.lock().unwrap() = false;
    }
}
pub(crate) struct PlannedReviewOwnership(ActualPlannedInvocation);
impl Drop for PlannedReviewOwnership {
    fn drop(&mut self) {
        self.0.seal_review();
    }
}

/// Issued when this agent actually parks a call. Reopening cannot recreate it.
#[derive(Clone)]
pub struct ActualParkedCall(Arc<Parked>);
struct Parked {
    planned: ActualPlannedInvocation,
    review_source: Option<SourceRef>,
    shown: Option<[u8; 32]>,
}
impl ActualParkedCall {
    pub(crate) fn issued(
        planned: ActualPlannedInvocation,
        review_source: Option<SourceRef>,
        shown: Option<[u8; 32]>,
    ) -> Self {
        Self(Arc::new(Parked {
            planned,
            review_source,
            shown,
        }))
    }
    pub fn owner(&self) -> &ActualAgentOwner {
        self.0.planned.owner()
    }
    pub fn call(&self) -> &ToolCall {
        self.0.planned.call()
    }
    pub fn planned(&self) -> &ActualPlannedInvocation {
        &self.0.planned
    }
    pub fn review_source(&self) -> Option<&SourceRef> {
        self.0.review_source.as_ref()
    }
    /// The selected host's digest for this exact pre-review source. It is
    /// carried to the registered human input callback; it grants no authority.
    pub fn shown(&self) -> Option<[u8; 32]> {
        self.0.shown
    }
    pub fn same_park(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl ActualAgentOwner {
    pub(crate) fn issued() -> Self {
        Self(Arc::new(()))
    }

    pub fn same_agent(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// What the agent actually received from its selected provider. This is not a
/// transport certificate: that provider's opaque receipt needs host validation.
#[derive(Clone)]
pub struct ActualReceivedReply(Arc<Received>);

struct Received {
    owner: ActualAgentOwner,
    message: Msg,
    usage: Usage,
    receipt: SourceRef,
}

impl ActualReceivedReply {
    pub(crate) fn issued(
        owner: &ActualAgentOwner,
        message: Msg,
        usage: Usage,
        receipt: SourceRef,
    ) -> Self {
        Self(Arc::new(Received {
            owner: owner.clone(),
            message,
            usage,
            receipt,
        }))
    }

    pub fn owner(&self) -> &ActualAgentOwner {
        &self.0.owner
    }
    pub fn message(&self) -> &Msg {
        &self.0.message
    }
    pub fn usage(&self) -> &Usage {
        &self.0.usage
    }
    pub fn transport_receipt(&self) -> &SourceRef {
        &self.0.receipt
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RetainedEvent {
    Input,
    ModelReply,
    Summary,
    ToolResult,
    Framework(FrameworkCommit),
    /// Retained by adopting a transferred conversation. The event that first
    /// produced the message is the transferred original's own; see
    /// [`ActualRetainedCommit::adopted`].
    Adopted,
}

/// Issued at the agent's actual append/commit, with the immutable item that
/// was retained there. No constructor is exported. It claims neither a save
/// nor permission; save failures and durability remain the agent's facts.
#[derive(Clone)]
pub struct ActualRetainedCommit(Arc<Retained>);

struct Retained {
    owner: ActualAgentOwner,
    event: RetainedEvent,
    message: Msg,
    received: Option<ActualReceivedReply>,
    edits: Vec<ReplyEdit>,
    summary: Option<ActualStagedSummary>,
    parts: Option<Arc<[SourcePart]>>,
    host_stage: Option<ActualStagedHostMessage>,
    /// The adopted message this commit was issued for, held erased: only
    /// an adoption makes one, and only `adopted` reads it.
    adopted: Option<SourceRef>,
}

impl ActualRetainedCommit {
    pub(crate) fn issued(
        owner: &ActualAgentOwner,
        event: RetainedEvent,
        message: Msg,
        received: Option<ActualReceivedReply>,
    ) -> Self {
        Self(Arc::new(Retained {
            owner: owner.clone(),
            event,
            message,
            received,
            edits: Vec::new(),
            summary: None,
            parts: None,
            host_stage: None,
            adopted: None,
        }))
    }

    pub(crate) fn model_issued(
        owner: &ActualAgentOwner,
        message: Msg,
        received: ActualReceivedReply,
        edits: Vec<ReplyEdit>,
    ) -> Self {
        Self(Arc::new(Retained {
            owner: owner.clone(),
            event: RetainedEvent::ModelReply,
            message,
            received: Some(received),
            edits,
            summary: None,
            parts: None,
            host_stage: None,
            adopted: None,
        }))
    }

    pub(crate) fn summary_issued(stage: ActualStagedSummary) -> Self {
        Self(Arc::new(Retained {
            owner: stage.owner().clone(),
            event: RetainedEvent::Summary,
            message: stage.message().clone(),
            received: Some(stage.received().clone()),
            edits: Vec::new(),
            summary: Some(stage),
            parts: None,
            host_stage: None,
            adopted: None,
        }))
    }

    pub(crate) fn envelope_issued(
        owner: &ActualAgentOwner,
        event: FrameworkCommit,
        message: Msg,
        parts: Arc<[SourcePart]>,
    ) -> Self {
        Self(Arc::new(Retained {
            owner: owner.clone(),
            event: RetainedEvent::Framework(event),
            message,
            received: None,
            edits: Vec::new(),
            summary: None,
            parts: Some(parts),
            host_stage: None,
            adopted: None,
        }))
    }

    pub(crate) fn host_issued(stage: ActualStagedHostMessage) -> Self {
        Self(Arc::new(Retained {
            owner: stage.owner().clone(),
            event: RetainedEvent::Framework(FrameworkCommit::HostNote),
            message: stage.message().clone(),
            received: None,
            edits: Vec::new(),
            summary: None,
            parts: Some(stage.0.parts.clone()),
            host_stage: Some(stage),
            adopted: None,
        }))
    }

    /// Issued only where a fresh agent adopts a transferred conversation,
    /// for the message at `adoption`'s place. The message is the original's.
    pub(crate) fn adopted_issued(owner: &ActualAgentOwner, adoption: ActualAdoptedMessage) -> Self {
        Self(Arc::new(Retained {
            owner: owner.clone(),
            event: RetainedEvent::Adopted,
            message: adoption.original().commit().message().clone(),
            received: None,
            edits: Vec::new(),
            summary: None,
            parts: None,
            host_stage: None,
            adopted: Some(SourceRef::new(adoption.0)),
        }))
    }

    /// The transferred original behind an adopted message: the retiring
    /// agent's own commit, with its true event, and the source that agent's
    /// boundary kept beside it. `None` for every other event.
    pub fn adopted(&self) -> Option<ActualAdoptedMessage> {
        match &self.0.adopted {
            Some(adoption) => adoption
                .downcast::<AdoptedMessage>()
                .map(ActualAdoptedMessage),
            None => None,
        }
    }

    pub fn host_stage(&self) -> Option<&ActualStagedHostMessage> {
        self.0.host_stage.as_ref()
    }

    pub fn owner(&self) -> &ActualAgentOwner {
        &self.0.owner
    }
    pub fn event(&self) -> RetainedEvent {
        self.0.event
    }
    pub fn message(&self) -> &Msg {
        &self.0.message
    }
    pub fn received(&self) -> Option<&ActualReceivedReply> {
        self.0.received.as_ref()
    }
    pub fn edits(&self) -> &[ReplyEdit] {
        &self.0.edits
    }
    pub fn summary(&self) -> Option<&ActualStagedSummary> {
        self.0.summary.as_ref()
    }
    pub fn parts(&self) -> Option<&[SourcePart]> {
        self.0.parts.as_deref()
    }
    pub fn same_commit(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// An instruction the framework actually put in a request, without appending
/// it to the conversation. It is neither a human decision nor a retained
/// conversation commit. Only the request construction event issues it.
#[derive(Clone)]
pub struct ActualRequestInput(Arc<RequestInput>);
struct RequestInput {
    owner: ActualAgentOwner,
    message: Msg,
}
impl ActualRequestInput {
    pub(crate) fn issued(owner: &ActualAgentOwner, message: Msg) -> Self {
        Self(Arc::new(RequestInput {
            owner: owner.clone(),
            message,
        }))
    }
    pub fn owner(&self) -> &ActualAgentOwner {
        &self.0.owner
    }
    pub fn message(&self) -> &Msg {
        &self.0.message
    }
}

/// A summary replacement actually staged in the round's working state. A
/// later retained commit is issued only if that state really becomes the
/// conversation. A failed or cancelled stage is not a saved summary.
#[derive(Clone)]
pub struct ActualStagedSummary(Arc<StagedSummary>);
struct StagedSummary {
    owner: ActualAgentOwner,
    message: Msg,
    received: ActualReceivedReply,
    open: SourcePart,
    close: SourcePart,
}
impl ActualStagedSummary {
    pub(crate) fn issued(
        owner: &ActualAgentOwner,
        message: Msg,
        received: ActualReceivedReply,
        open: SourcePart,
        close: SourcePart,
    ) -> Self {
        Self(Arc::new(StagedSummary {
            owner: owner.clone(),
            message,
            received,
            open,
            close,
        }))
    }
    pub fn owner(&self) -> &ActualAgentOwner {
        &self.0.owner
    }
    pub fn message(&self) -> &Msg {
        &self.0.message
    }
    pub fn received(&self) -> &ActualReceivedReply {
        &self.0.received
    }
    pub fn open(&self) -> &SourcePart {
        &self.0.open
    }
    pub fn close(&self) -> &SourcePart {
        &self.0.close
    }
    pub fn same_stage(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// A host message actually appended to the working state. It is distinct
/// from an actual retained commit and carries its complete source parts.
#[derive(Clone)]
pub struct ActualStagedHostMessage(Arc<StagedHost>);
struct StagedHost {
    owner: ActualAgentOwner,
    message: Msg,
    parts: Arc<[SourcePart]>,
}
impl ActualStagedHostMessage {
    pub(crate) fn issued(owner: &ActualAgentOwner, message: Msg, parts: Arc<[SourcePart]>) -> Self {
        Self(Arc::new(StagedHost {
            owner: owner.clone(),
            message,
            parts,
        }))
    }
    pub fn owner(&self) -> &ActualAgentOwner {
        &self.0.owner
    }
    pub fn message(&self) -> &Msg {
        &self.0.message
    }
    pub fn parts(&self) -> &[SourcePart] {
        &self.0.parts
    }
    pub fn same_stage(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// A source already bound to a real immutable retained message. The getter
/// verifies the complete current message; this cannot be minted from caller
/// JSON, a role, an index, a label or equal text. It grants no app authority.
#[derive(Clone)]
pub struct ActualSourcedMessage {
    commit: ActualRetainedCommit,
    source: SourceRef,
}
impl ActualSourcedMessage {
    pub(crate) fn issued(commit: ActualRetainedCommit, source: SourceRef) -> Self {
        Self { commit, source }
    }
    pub fn commit(&self) -> &ActualRetainedCommit {
        &self.commit
    }
    pub fn source(&self) -> &SourceRef {
        &self.source
    }
}

/// The whole recorded conversation of a selected agent, as it stood when that
/// agent's own retirement closed its source view. Only
/// `Agent::retire_conversation_with_view` issues one: for an idle root agent
/// with no parked approval, after every recorded message had a source of the
/// retiring boundary. It is immutable, and a fresh selected agent adopts it at
/// most once. It is not a checkpoint image, a caller's state or JSON: nothing
/// outside this crate can make one. It grants the host no source, transport,
/// restoration or permission authority; the adopting host validates every
/// original against its own retired owner.
#[derive(Clone)]
pub struct ActualConversationTransfer(Arc<Transfer>);
struct Transfer {
    owner: ActualAgentOwner,
    /// The recorded state, until the one adoption takes it.
    state: std::sync::Mutex<Option<AgentState>>,
    messages: Vec<ActualSourcedMessage>,
    todos: Option<Vec<super::PartFrom>>,
    thread: String,
    next_seq: u64,
    checkpoint_failed: bool,
    recovery: Option<crate::checkpoint::RecoveryInfo>,
    adopted: std::sync::atomic::AtomicBool,
}
impl ActualConversationTransfer {
    /// `saved` is the retiring agent's thread, its next save sequence,
    /// whether its last save failed and its recovery notice.
    pub(crate) fn issued(
        owner: &ActualAgentOwner,
        state: AgentState,
        messages: Vec<ActualSourcedMessage>,
        todos: Option<Vec<super::PartFrom>>,
        saved: (String, u64, bool, Option<crate::checkpoint::RecoveryInfo>),
    ) -> Self {
        let (thread, next_seq, checkpoint_failed, recovery) = saved;
        Self(Arc::new(Transfer {
            owner: owner.clone(),
            state: std::sync::Mutex::new(Some(state)),
            messages,
            todos,
            thread,
            next_seq,
            checkpoint_failed,
            recovery,
            adopted: std::sync::atomic::AtomicBool::new(false),
        }))
    }
    /// The retired agent.
    pub fn owner(&self) -> &ActualAgentOwner {
        &self.0.owner
    }
    /// Every recorded message with the source the retired boundary kept
    /// beside it, in the recorded order.
    pub fn messages(&self) -> &[ActualSourcedMessage] {
        &self.0.messages
    }
    /// A copy of the recorded state at retirement: messages, todos, files
    /// and custom state. The adopting agent takes exactly this, and takes
    /// it out: `None` once an agent has adopted the conversation. Every
    /// adopted commit keeps this transfer for the retired agent's own
    /// commit and source of its message ([`messages`](Self::messages)),
    /// and those stay; a second copy of the whole record does not.
    pub fn state(&self) -> Option<AgentState> {
        self.0.state.lock().unwrap().clone()
    }
    /// The thread the retired agent saved under.
    pub fn thread_id(&self) -> &str {
        &self.0.thread
    }
    pub fn same_transfer(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    /// Whether an agent has adopted this conversation.
    pub fn is_adopted(&self) -> bool {
        self.0.adopted.load(std::sync::atomic::Ordering::Acquire)
    }
    pub(crate) fn todos(&self) -> Option<&[super::PartFrom]> {
        self.0.todos.as_deref()
    }
    pub(crate) fn next_seq(&self) -> u64 {
        self.0.next_seq
    }
    pub(crate) fn checkpoint_failed(&self) -> bool {
        self.0.checkpoint_failed
    }
    pub(crate) fn recovery(&self) -> Option<crate::checkpoint::RecoveryInfo> {
        self.0.recovery
    }
    /// Marks the one adoption and hands out the recorded state, which the
    /// transfer no longer holds afterwards. `None` when it was adopted
    /// before.
    pub(crate) fn take(&self) -> Option<AgentState> {
        if self
            .0
            .adopted
            .swap(true, std::sync::atomic::Ordering::AcqRel)
        {
            return None;
        }
        self.0.state.lock().unwrap().take()
    }
}

/// One message of a transferred conversation at the adoption that retained
/// it: the transfer and the message's place in it. Issued only with the
/// adopting agent's commit; a place, a role or equal text cannot make one.
#[derive(Clone)]
pub struct ActualAdoptedMessage(Arc<AdoptedMessage>);
struct AdoptedMessage {
    transfer: ActualConversationTransfer,
    index: usize,
}
impl ActualAdoptedMessage {
    /// `index` is a place of `transfer`; the one caller iterates them.
    pub(crate) fn issued(transfer: ActualConversationTransfer, index: usize) -> Self {
        Self(Arc::new(AdoptedMessage { transfer, index }))
    }
    pub fn transfer(&self) -> &ActualConversationTransfer {
        &self.0.transfer
    }
    pub fn index(&self) -> usize {
        self.0.index
    }
    /// The retired agent's own commit and the source its boundary kept.
    pub fn original(&self) -> &ActualSourcedMessage {
        &self.0.transfer.0.messages[self.0.index]
    }
}

/// The completion controller actually scheduled this exact host call after
/// its directive returned and the carrier message was appended. Neither a
/// model reply nor user permission is inferred from it. No public issuer.
#[derive(Clone)]
pub struct ActualHostScheduledCall(Arc<HostScheduled>);
struct HostScheduled {
    owner: ActualAgentOwner,
    controller: Arc<dyn crate::CompletionCheck>,
    call: ToolCall,
    commit: ActualRetainedCommit,
    source: Option<SourceRef>,
}
impl ActualHostScheduledCall {
    pub(crate) fn issued(
        owner: &ActualAgentOwner,
        controller: Arc<dyn crate::CompletionCheck>,
        call: ToolCall,
        commit: ActualRetainedCommit,
    ) -> Self {
        let source = controller.source_for_view();
        Self(Arc::new(HostScheduled {
            owner: owner.clone(),
            controller,
            call,
            commit,
            source,
        }))
    }
    pub fn owner(&self) -> &ActualAgentOwner {
        &self.0.owner
    }
    pub fn call(&self) -> &ToolCall {
        &self.0.call
    }
    pub fn message(&self) -> &Msg {
        self.0.commit.message()
    }
    pub fn commit(&self) -> &ActualRetainedCommit {
        &self.0.commit
    }
    pub fn same_schedule(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    pub fn source(&self) -> Option<&SourceRef> {
        self.0.source.as_ref()
    }
    pub fn same_controller(&self, controller: &Arc<dyn crate::CompletionCheck>) -> bool {
        Arc::ptr_eq(&self.0.controller, controller)
    }
}

/// The original boundary-controller object installed on this actual agent,
/// with genuine retained sources for a future selected controller adapter.
/// This proves installation/source binding, not that a callback ran or that
/// mutable custom state is provenance. No public issuer or serialized form.
#[derive(Clone)]
pub struct ActualBoundaryController {
    owner: ActualAgentOwner,
    controller: Arc<dyn crate::AgentBoundaryPolicy>,
    messages: Vec<ActualSourcedMessage>,
}
impl ActualBoundaryController {
    pub(crate) fn issued(
        owner: &ActualAgentOwner,
        controller: Arc<dyn crate::AgentBoundaryPolicy>,
        messages: Vec<ActualSourcedMessage>,
    ) -> Self {
        Self {
            owner: owner.clone(),
            controller,
            messages,
        }
    }
    pub fn owner(&self) -> &ActualAgentOwner {
        &self.owner
    }
    pub fn messages(&self) -> &[ActualSourcedMessage] {
        &self.messages
    }
    pub fn same_controller(&self, controller: &Arc<dyn crate::AgentBoundaryPolicy>) -> bool {
        Arc::ptr_eq(&self.controller, controller)
    }
}

/// The real accepted-input callback of this installed controller,
/// minted after the exact input/source append and before its checkpoint.
/// Kind and index locate the actual event; they grant no application origin.
#[derive(Clone)]
pub struct ActualBoundaryInput(Arc<BoundaryInput>);
struct BoundaryInput {
    owner: ActualAgentOwner,
    controller: Arc<dyn crate::AgentBoundaryPolicy>,
    input: ActualSourcedMessage,
    index: usize,
    kind: crate::HumanInputKind,
    live: Mutex<bool>,
}
impl ActualBoundaryInput {
    pub(crate) fn issued(
        owner: &ActualAgentOwner,
        controller: Arc<dyn crate::AgentBoundaryPolicy>,
        input: ActualSourcedMessage,
        index: usize,
        kind: crate::HumanInputKind,
    ) -> Self {
        Self(Arc::new(BoundaryInput {
            owner: owner.clone(),
            controller,
            input,
            index,
            kind,
            live: Mutex::new(true),
        }))
    }
    pub fn owner(&self) -> &ActualAgentOwner {
        &self.0.owner
    }
    pub fn input(&self) -> &ActualSourcedMessage {
        &self.0.input
    }
    pub fn index(&self) -> usize {
        self.0.index
    }
    pub fn kind(&self) -> crate::HumanInputKind {
        self.0.kind
    }
    pub fn same_controller(&self, controller: &Arc<dyn crate::AgentBoundaryPolicy>) -> bool {
        Arc::ptr_eq(&self.0.controller, controller)
    }
    pub fn same_input_event(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    /// Actual callback liveness only. A host must still validate its private
    /// producer and consume this event once; this is no human/permission grant.
    pub fn in_callback(&self) -> bool {
        *self.0.live.lock().unwrap()
    }
    pub(crate) fn own_callback(&self) -> BoundaryInputOwnership {
        BoundaryInputOwnership(self.clone())
    }
}
pub(crate) struct BoundaryInputOwnership(ActualBoundaryInput);
impl Drop for BoundaryInputOwnership {
    fn drop(&mut self) {
        *self.0.0.live.lock().unwrap() = false;
    }
}

/// Issued only after the actual exclusive idle replacement, before its
/// original controller callback/final save. Source data does not claim save
/// or application authority; a staged summary source remains distinguishable.
#[derive(Clone)]
pub struct ActualIdleCompaction(Arc<IdleCompaction>);
struct IdleCompaction {
    owner: ActualAgentOwner,
    controller: Arc<dyn crate::AgentBoundaryPolicy>,
    source: SourceRef,
    before: Vec<ActualSourcedMessage>,
    after: Vec<ActualSourcedMessage>,
    live: Mutex<bool>,
}
impl ActualIdleCompaction {
    pub(crate) fn issued(
        owner: &ActualAgentOwner,
        controller: Arc<dyn crate::AgentBoundaryPolicy>,
        source: SourceRef,
        before: Vec<ActualSourcedMessage>,
        after: Vec<ActualSourcedMessage>,
    ) -> Self {
        Self(Arc::new(IdleCompaction {
            owner: owner.clone(),
            controller,
            source,
            before,
            after,
            live: Mutex::new(true),
        }))
    }
    pub fn owner(&self) -> &ActualAgentOwner {
        &self.0.owner
    }
    pub fn source(&self) -> &SourceRef {
        &self.0.source
    }
    pub fn before(&self) -> &[ActualSourcedMessage] {
        &self.0.before
    }
    pub fn after(&self) -> &[ActualSourcedMessage] {
        &self.0.after
    }
    pub fn same_controller(&self, controller: &Arc<dyn crate::AgentBoundaryPolicy>) -> bool {
        Arc::ptr_eq(&self.0.controller, controller)
    }
    pub fn in_callback(&self) -> bool {
        *self.0.live.lock().unwrap()
    }
    pub fn same_compaction(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    pub(crate) fn own_callback(&self) -> IdleCompactionOwnership {
        IdleCompactionOwnership(self.clone())
    }
}
pub(crate) struct IdleCompactionOwnership(ActualIdleCompaction);
impl Drop for IdleCompactionOwnership {
    fn drop(&mut self) {
        *self.0.0.live.lock().unwrap() = false;
    }
}

/// The actual event behind an item in a model request. The host validates
/// its own source against this immutable event; none of the variants grants
/// transport, human, restoration or tool permission authority.
#[derive(Clone)]
#[non_exhaustive]
pub enum ActualModelInput {
    Retained(ActualRetainedCommit),
    StagedSummary(ActualStagedSummary),
    StagedHostMessage(ActualStagedHostMessage),
    RequestInput(ActualRequestInput),
}
impl ActualModelInput {
    pub fn owner(&self) -> &ActualAgentOwner {
        match self {
            Self::Retained(commit) => commit.owner(),
            Self::StagedSummary(stage) => stage.owner(),
            Self::StagedHostMessage(stage) => stage.owner(),
            Self::RequestInput(input) => input.owner(),
        }
    }
    pub fn message(&self) -> &Msg {
        match self {
            Self::Retained(commit) => commit.message(),
            Self::StagedSummary(stage) => stage.message(),
            Self::StagedHostMessage(stage) => stage.message(),
            Self::RequestInput(input) => input.message(),
        }
    }
}

#[derive(Default)]
pub(crate) struct ActualFacts {
    pub(crate) received: Vec<ActualReceivedReply>,
    pub(crate) retained: Vec<ActualRetainedCommit>,
    pub(crate) terminals: Vec<ActualTerminalOutcome>,
    pub(crate) chains: Vec<ActualToolFact>,
    pub(crate) request_inputs: Vec<ActualRequestInput>,
    pub(crate) staged_summaries: Vec<ActualStagedSummary>,
    pub(crate) staged_host_messages: Vec<ActualStagedHostMessage>,
    pub(crate) host_calls: Vec<ActualHostScheduledCall>,
    pub(crate) provider_returns: Vec<ActualProviderReturn>,
    pub(crate) tasks: Vec<ActualTaskReturn>,
    pub(crate) entry_failures: Vec<String>,
    pub(crate) control_inputs: Vec<ActualBoundaryInput>,
    pub(crate) idle_compactions: Vec<ActualIdleCompaction>,
}

/// The real return of an owned selected child task, including failures and
/// its actual partial state. It is neither a registry status nor permission.
#[derive(Clone)]
pub struct ActualTaskReturn(Arc<TaskReturn>);
struct TaskReturn {
    owner: ActualAgentOwner,
    parent: Option<ActualToolInvocation>,
    final_text: Option<String>,
    suspension: Option<crate::Suspension>,
    failure: Option<String>,
    state: AgentState,
    provider_returns: Vec<ActualProviderReturn>,
    terminals: Vec<ActualTerminalOutcome>,
    chains: Vec<ActualToolFact>,
    retained: Vec<ActualRetainedCommit>,
    entry_failures: Vec<String>,
    control_inputs: Vec<ActualBoundaryInput>,
}
impl ActualTaskReturn {
    pub(crate) fn issued(
        owner: &ActualAgentOwner,
        parent: Option<ActualToolInvocation>,
        result: &Result<crate::subagent::TaskResult, AgentError>,
        state: AgentState,
        facts: &ActualFacts,
    ) -> Self {
        let (final_text, suspension, failure) = match result {
            Ok(crate::subagent::TaskResult::Done(outcome)) => {
                (Some(outcome.final_text.clone()), None, None)
            }
            Ok(crate::subagent::TaskResult::Suspended(suspension)) => {
                (None, Some(suspension.clone()), None)
            }
            Err(error) => (None, None, Some(error.to_string())),
        };
        Self(Arc::new(TaskReturn {
            owner: owner.clone(),
            parent,
            final_text,
            suspension,
            failure,
            state,
            provider_returns: facts.provider_returns.clone(),
            terminals: facts.terminals.clone(),
            chains: facts.chains.clone(),
            retained: facts.retained.clone(),
            entry_failures: facts.entry_failures.clone(),
            control_inputs: facts.control_inputs.clone(),
        }))
    }
    pub fn owner(&self) -> &ActualAgentOwner {
        &self.0.owner
    }
    pub fn parent_invocation(&self) -> Option<&ActualToolInvocation> {
        self.0.parent.as_ref()
    }
    pub fn final_text(&self) -> Option<&str> {
        self.0.final_text.as_deref()
    }
    pub fn suspension(&self) -> Option<&crate::Suspension> {
        self.0.suspension.as_ref()
    }
    pub fn failure(&self) -> Option<&str> {
        self.0.failure.as_deref()
    }
    /// Actual entry/final-handler/save errors retained before fallible
    /// projection. Data only, not a source or completion certificate.
    pub fn entry_failures(&self) -> &[String] {
        &self.0.entry_failures
    }
    pub fn control_inputs(&self) -> &[ActualBoundaryInput] {
        &self.0.control_inputs
    }
    pub fn state(&self) -> &AgentState {
        &self.0.state
    }
    pub fn provider_returns(&self) -> &[ActualProviderReturn] {
        &self.0.provider_returns
    }
    pub fn terminal_outcomes(&self) -> &[ActualTerminalOutcome] {
        &self.0.terminals
    }
    pub fn tool_facts(&self) -> &[ActualToolFact] {
        &self.0.chains
    }
    pub fn retained_commits(&self) -> &[ActualRetainedCommit] {
        &self.0.retained
    }
}

/// Data the selected provider handed back while its actual terminal call
/// was owned. Kept before its later receipt/projection checks. This is data
/// retention only: a provider can be synthetic, and this record certifies
/// neither HTTP receipt, committed origin, usage settlement nor authority.
#[derive(Clone)]
pub struct ActualProviderReturn {
    owner: ActualAgentOwner,
    response: ModelResponse,
}
impl ActualProviderReturn {
    pub fn owner(&self) -> &ActualAgentOwner {
        &self.owner
    }
    pub fn message(&self) -> &Msg {
        &self.response.message
    }
    pub fn usage(&self) -> &Usage {
        &self.response.usage
    }
}

/// Attached only at the framework's actual provider terminal, not given to
/// middleware as a return factory. One owned call can retain one return.
pub(crate) struct ReturnRecorder {
    owner: ActualAgentOwner,
    facts: Arc<Mutex<ActualFacts>>,
    state: Mutex<ReturnState>,
}
struct ReturnState {
    open: bool,
    returned: bool,
}
impl ReturnRecorder {
    pub(crate) fn new(owner: &ActualAgentOwner, facts: Arc<Mutex<ActualFacts>>) -> Arc<Self> {
        Arc::new(Self {
            owner: owner.clone(),
            facts,
            state: Mutex::new(ReturnState {
                open: true,
                returned: false,
            }),
        })
    }
    pub(crate) fn retain(&self, response: &ModelResponse) -> bool {
        let mut state = self.state.lock().unwrap();
        if !state.open || state.returned {
            return false;
        }
        self.facts
            .lock()
            .unwrap()
            .provider_returns
            .push(ActualProviderReturn {
                owner: self.owner.clone(),
                response: response.clone(),
            });
        state.returned = true;
        true
    }
    pub(crate) fn owned(self: &Arc<Self>) -> ReturnOwnership {
        ReturnOwnership(self.clone())
    }
}
pub(crate) struct ReturnOwnership(Arc<ReturnRecorder>);
impl Drop for ReturnOwnership {
    fn drop(&mut self) {
        self.0.state.lock().unwrap().open = false;
    }
}

/// A particular post-chain terminal entry. It is issued for the exact call
/// before the tool future is polled, and is never an approval or effect claim.
#[derive(Clone)]
pub struct ActualToolInvocation(Arc<Invocation>);

struct Invocation {
    owner: ActualAgentOwner,
    call: ToolCall,
    resumed: bool,
    // A delegated gate can decide inside the actual terminal. Its source
    // is attached once there, before its inner tool runs; never replaced.
    decision: OnceLock<SourceRef>,
    decision_open: Mutex<bool>,
    action: Option<HitlAction>,
    inline_action: OnceLock<HitlAction>,
    planned: ActualPlannedInvocation,
    review_source: Option<SourceRef>,
    shown: Option<[u8; 32]>,
    parent: Option<ActualToolInvocation>,
    parent_source: Option<SourceRef>,
}

impl ActualToolInvocation {
    pub(crate) fn issued(
        owner: &ActualAgentOwner,
        call: ToolCall,
        resumed: bool,
        decision: Option<SourceRef>,
        action: Option<HitlAction>,
        planned: ActualPlannedInvocation,
        review_source: Option<SourceRef>,
        shown: Option<[u8; 32]>,
    ) -> Self {
        let held_decision = OnceLock::new();
        if let Some(decision) = decision {
            let _ = held_decision.set(decision);
        }
        Self(Arc::new(Invocation {
            owner: owner.clone(),
            call,
            resumed,
            decision: held_decision,
            decision_open: Mutex::new(true),
            action,
            inline_action: OnceLock::new(),
            planned,
            review_source,
            shown,
            parent: None,
            parent_source: None,
        }))
    }

    /// The real inner handler entry of an already running terminal. Neither
    /// caller arguments nor this event supplies tool/application permission.
    pub(crate) fn inner_issued(
        parent: &Self,
        parent_source: Option<SourceRef>,
        call: ToolCall,
        decision: Option<SourceRef>,
        action: Option<HitlAction>,
    ) -> Self {
        let held_decision = OnceLock::new();
        if let Some(decision) = decision {
            let _ = held_decision.set(decision);
        }
        Self(Arc::new(Invocation {
            owner: parent.owner().clone(),
            call,
            resumed: parent.resumed(),
            decision: held_decision,
            decision_open: Mutex::new(true),
            action,
            inline_action: OnceLock::new(),
            planned: parent.planned().clone(),
            review_source: parent.review_source().cloned(),
            shown: parent.shown(),
            parent: Some(parent.clone()),
            parent_source,
        }))
    }
    pub fn parent(&self) -> Option<&Self> {
        self.0.parent.as_ref()
    }
    /// The host's source for that actual outer event; erasure is not authority.
    pub fn parent_source(&self) -> Option<&SourceRef> {
        self.0.parent_source.as_ref()
    }

    pub fn owner(&self) -> &ActualAgentOwner {
        &self.0.owner
    }
    pub fn call(&self) -> &ToolCall {
        &self.0.call
    }
    pub fn resumed(&self) -> bool {
        self.0.resumed
    }
    pub fn decision_source(&self) -> Option<&SourceRef> {
        self.0.decision.get()
    }
    #[cfg(test)]
    pub(crate) fn note_decision(&self, source: SourceRef) -> Result<(), AgentError> {
        self.note_decision_action(source, None)
    }
    pub(crate) fn note_decision_action(
        &self,
        source: SourceRef,
        action: Option<HitlAction>,
    ) -> Result<(), AgentError> {
        let open = self.0.decision_open.lock().unwrap();
        if !*open {
            return Err(AgentError::State(
                "This invocation has ended its decision entry.".into(),
            ));
        }
        if self.0.decision.get().is_some() {
            return Err(AgentError::State(
                "This invocation already has a decision source.".into(),
            ));
        }
        if let Some(action) = action {
            self.0.inline_action.set(action).map_err(|_| {
                AgentError::State("This invocation already has an inline action.".into())
            })?;
        }
        self.0
            .decision
            .set(source)
            .map_err(|_| AgentError::State("This invocation already has a decision source.".into()))
    }
    pub(crate) fn own_decision(&self) -> InvocationDecisionOwnership {
        InvocationDecisionOwnership(self.clone())
    }
    pub(crate) fn seal_decision(&self) {
        *self.0.decision_open.lock().unwrap() = false;
    }
    pub fn decision_action(&self) -> Option<&HitlAction> {
        self.0
            .action
            .as_ref()
            .or_else(|| self.0.inline_action.get())
    }
    pub fn human_edited(&self) -> bool {
        matches!(
            self.decision_action(),
            Some(HitlAction::ApproveEdited { .. })
        )
    }

    pub fn planned(&self) -> &ActualPlannedInvocation {
        &self.0.planned
    }
    /// Event linkage only. Middleware and a human can change arguments;
    /// consumers must also validate the exact final call and sealed decision.
    pub fn was_planned_as(&self, planned: &ActualPlannedInvocation) -> bool {
        self.0.planned.same_plan(planned)
    }
    pub fn review_source(&self) -> Option<&SourceRef> {
        self.0.review_source.as_ref()
    }
    pub fn shown(&self) -> Option<[u8; 32]> {
        self.0.shown
    }

    pub fn same_invocation(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

pub(crate) struct InvocationDecisionOwnership(ActualToolInvocation);
impl Drop for InvocationDecisionOwnership {
    fn drop(&mut self) {
        self.0.seal_decision();
    }
}

#[derive(Clone)]
pub struct ActualTerminalOutcome {
    invocation: ActualToolInvocation,
    source: Option<SourceRef>,
    reached: ReachedTool,
}

impl ActualTerminalOutcome {
    pub(crate) fn issued(
        invocation: ActualToolInvocation,
        source: Option<SourceRef>,
        reached: ReachedTool,
    ) -> Self {
        Self {
            invocation,
            source,
            reached,
        }
    }

    pub fn invocation(&self) -> &ActualToolInvocation {
        &self.invocation
    }
    pub fn operation_source(&self) -> Option<&SourceRef> {
        self.source.as_ref()
    }
    pub fn reached(&self) -> &ReachedTool {
        &self.reached
    }
}

/// The actual chain outcome and every actual terminal outcome, retained
/// before the host is asked for any projection. Metadata stays with outcomes.
#[derive(Clone)]
pub struct ActualToolFact(Arc<ToolFact>);

struct ToolFact {
    owner: ActualAgentOwner,
    proposed_call: ToolCall,
    commit: Option<ActualRetainedCommit>,
    terminals: Vec<ActualTerminalOutcome>,
    returned: Option<ToolOutcome>,
    failure: Option<String>,
    appended: Vec<SourcePart>,
    parts: Vec<SourcePart>,
}

impl ActualToolFact {
    pub(crate) fn issued(
        owner: &ActualAgentOwner,
        proposed_call: ToolCall,
        terminals: Vec<ActualTerminalOutcome>,
        returned: Option<ToolOutcome>,
        failure: Option<String>,
    ) -> Self {
        Self(Arc::new(ToolFact {
            owner: owner.clone(),
            proposed_call,
            commit: None,
            terminals,
            returned,
            failure,
            appended: Vec::new(),
            parts: Vec::new(),
        }))
    }

    pub(crate) fn with_parts(&self, appended: Vec<SourcePart>, parts: Vec<SourcePart>) -> Self {
        Self(Arc::new(ToolFact {
            owner: self.0.owner.clone(),
            proposed_call: self.0.proposed_call.clone(),
            commit: self.0.commit.clone(),
            terminals: self.0.terminals.clone(),
            returned: self.0.returned.clone(),
            failure: self.0.failure.clone(),
            appended,
            parts,
        }))
    }

    pub(crate) fn retained(&self, commit: ActualRetainedCommit) -> Self {
        Self(Arc::new(ToolFact {
            owner: self.0.owner.clone(),
            proposed_call: self.0.proposed_call.clone(),
            commit: Some(commit),
            terminals: self.0.terminals.clone(),
            returned: self.0.returned.clone(),
            failure: self.0.failure.clone(),
            appended: self.0.appended.clone(),
            parts: self.0.parts.clone(),
        }))
    }

    pub fn owner(&self) -> &ActualAgentOwner {
        &self.0.owner
    }
    pub fn proposed_call(&self) -> &ToolCall {
        &self.0.proposed_call
    }
    pub fn commit(&self) -> Option<&ActualRetainedCommit> {
        self.0.commit.as_ref()
    }
    pub fn terminals(&self) -> &[ActualTerminalOutcome] {
        &self.0.terminals
    }
    pub fn returned(&self) -> Option<&ToolOutcome> {
        self.0.returned.as_ref()
    }
    pub fn failure(&self) -> Option<&str> {
        self.0.failure.as_deref()
    }
    pub fn appended(&self) -> &[SourcePart] {
        &self.0.appended
    }
    pub fn parts(&self) -> &[SourcePart] {
        &self.0.parts
    }
}

macro_rules! opaque_debug {
    ($($type:ty),+ $(,)?) => {$(
        impl std::fmt::Debug for $type {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(concat!(stringify!($type), "([actual event])"))
            }
        }
    )+};
}

opaque_debug!(
    ActualAgentOwner,
    ActualControllerBindings,
    ActualPlannedInvocation,
    ActualParkedCall,
    ActualReceivedReply,
    ActualRetainedCommit,
    ActualRequestInput,
    ActualStagedSummary,
    ActualStagedHostMessage,
    ActualSourcedMessage,
    ActualConversationTransfer,
    ActualAdoptedMessage,
    ActualHostScheduledCall,
    ActualBoundaryController,
    ActualBoundaryInput,
    ActualIdleCompaction,
    ActualModelInput,
    ActualProviderReturn,
    ActualTaskReturn,
    ActualToolInvocation,
    ActualTerminalOutcome,
    ActualToolFact
);
