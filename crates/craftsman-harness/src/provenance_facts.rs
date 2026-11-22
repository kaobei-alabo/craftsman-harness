//! Event witnesses issued by the agent, never reconstructed from a checkpoint
//! or a caller's mutable state. They describe retention and invocation only;
//! the host still validates its own source, transport and permission owners.

use crate::{HitlAction, ModelResponse, Msg, ToolCall, ToolOutcome, Usage};
use std::sync::{Arc, Mutex};

use super::{FrameworkCommit, ReachedTool, ReplyEdit, SourcePart, SourceRef};

#[derive(Clone)]
pub struct ActualAgentOwner(Arc<()>);

/// Issued when this agent actually parks a call. Reopening cannot recreate it.
#[derive(Clone)]
pub struct ActualParkedCall(Arc<Parked>);
struct Parked {
    owner: ActualAgentOwner,
    call: ToolCall,
}
impl ActualParkedCall {
    pub(crate) fn issued(owner: &ActualAgentOwner, call: ToolCall) -> Self {
        Self(Arc::new(Parked {
            owner: owner.clone(),
            call,
        }))
    }
    pub fn owner(&self) -> &ActualAgentOwner {
        &self.0.owner
    }
    pub fn call(&self) -> &ToolCall {
        &self.0.call
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
        }))
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

/// The actual event behind an item in a model request. The host validates
/// its own source against this immutable event; none of the variants grants
/// transport, human, restoration or tool permission authority.
#[derive(Clone)]
#[non_exhaustive]
pub enum ActualModelInput {
    Retained(ActualRetainedCommit),
    StagedSummary(ActualStagedSummary),
    RequestInput(ActualRequestInput),
}
impl ActualModelInput {
    pub fn owner(&self) -> &ActualAgentOwner {
        match self {
            Self::Retained(commit) => commit.owner(),
            Self::StagedSummary(stage) => stage.owner(),
            Self::RequestInput(input) => input.owner(),
        }
    }
    pub fn message(&self) -> &Msg {
        match self {
            Self::Retained(commit) => commit.message(),
            Self::StagedSummary(stage) => stage.message(),
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
    pub(crate) provider_returns: Vec<ActualProviderReturn>,
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
    decision: Option<SourceRef>,
    action: Option<HitlAction>,
}

impl ActualToolInvocation {
    pub(crate) fn issued(
        owner: &ActualAgentOwner,
        call: ToolCall,
        resumed: bool,
        decision: Option<SourceRef>,
        action: Option<HitlAction>,
    ) -> Self {
        Self(Arc::new(Invocation {
            owner: owner.clone(),
            call,
            resumed,
            decision,
            action,
        }))
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
        self.0.decision.as_ref()
    }
    pub fn decision_action(&self) -> Option<&HitlAction> {
        self.0.action.as_ref()
    }
    pub fn human_edited(&self) -> bool {
        matches!(self.0.action, Some(HitlAction::ApproveEdited { .. }))
    }

    pub fn same_invocation(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
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
    ActualParkedCall,
    ActualReceivedReply,
    ActualRetainedCommit,
    ActualRequestInput,
    ActualStagedSummary,
    ActualModelInput,
    ActualProviderReturn,
    ActualToolInvocation,
    ActualTerminalOutcome,
    ActualToolFact
);
