//! Optional boundary for a session whose model requests are composed from
//! views of sources instead of from the recorded text.
//!
//! Nothing selects it by default, and an agent built without it runs exactly
//! as before. This module holds ownership and ordering only. It knows nothing
//! about what a source is: the host that selects the boundary supplies opaque
//! evidence and checks it itself. There is no HTTP, storage, UI or host type
//! here.
//!
//! The recorded conversation is not changed by any of this. Accepted input,
//! returned tool results, received model replies, usage, the journal and
//! checkpoints stay exactly as the unselected agent keeps them. A source is
//! recorded beside the message it belongs to, after that message is
//! committed, and a failure to obtain one is reported after the real fact has
//! been kept.

use crate::{
    AgentError, HitlAction, ModelRequest, ModelResponse, Msg, ToolCall, ToolDef, ToolOutcome,
};
use async_trait::async_trait;
use std::any::Any;
use std::fmt;
use std::sync::Arc;

#[path = "provenance_facts.rs"]
mod facts;
pub use facts::{
    ActualAgentOwner, ActualBoundaryController, ActualHostScheduledCall, ActualModelInput,
    ActualParkedCall, ActualProviderReturn, ActualReceivedReply, ActualRequestInput,
    ActualRetainedCommit, ActualSourcedMessage, ActualStagedHostMessage, ActualStagedSummary,
    ActualTaskReturn, ActualTerminalOutcome, ActualToolFact, ActualToolInvocation, RetainedEvent,
};
pub(crate) use facts::{ActualFacts, ReturnRecorder};

/// Opaque evidence owned by the host that selected the boundary. This crate
/// only keeps it beside the item it belongs to and hands it back. Holding one
/// proves nothing here: the host checks its own payload.
#[derive(Clone)]
pub struct SourceRef {
    payload: Arc<dyn Any + Send + Sync>,
}

impl SourceRef {
    pub fn new<T: Any + Send + Sync>(payload: Arc<T>) -> Self {
        Self { payload }
    }

    /// The host's own payload, if this is one of that type.
    pub fn downcast<T: Any + Send + Sync>(&self) -> Option<Arc<T>> {
        self.payload.clone().downcast::<T>().ok()
    }

    /// Whether both refer to the same payload object. Equal contents never
    /// qualify.
    pub fn is(&self, other: &SourceRef) -> bool {
        std::ptr::addr_eq(Arc::as_ptr(&self.payload), Arc::as_ptr(&other.payload))
    }
}

impl fmt::Debug for SourceRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SourceRef([opaque])")
    }
}

/// One time a registered tool was actually run for a call: the exact call it
/// was given and what it returned, before any middleware saw the outcome.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ReachedTool {
    pub call: ToolCall,
    /// The tool's own outcome, or the text of the error it failed with.
    pub result: Result<ToolOutcome, String>,
}

impl ReachedTool {
    pub(crate) fn new(call: ToolCall, result: &Result<ToolOutcome, AgentError>) -> Self {
        Self {
            call,
            result: result.as_ref().map(Clone::clone).map_err(|e| e.to_string()),
        }
    }
}

/// One tool result exactly as it was committed to the conversation, with
/// what actually produced it.
#[non_exhaustive]
pub struct ObservedToolResult<'a> {
    pub call_id: &'a str,
    pub tool: &'a str,
    pub content: &'a str,
    pub is_error: bool,
    /// Every run of the tool itself for this call, in order. Empty when no
    /// registered tool ran: the call was refused on the way, or named no
    /// tool.
    pub reached: &'a [ReachedTool],
    /// What the dispatch chain returned, with its metadata. `None` when the
    /// chain failed and the recorded content is the text of that failure.
    pub returned: Option<&'a ToolOutcome>,
    /// Fixed text that middleware announced it appended to the outcome, in
    /// order.
    pub appended: &'a [SourcePart],
    /// The tool's own content as the pieces the tool said it is made of.
    /// Empty when the tool said nothing, or when the recorded content is not
    /// exactly those pieces followed by the appended text.
    pub parts: &'a [SourcePart],
}

/// A message the agent itself appended to the conversation, by the event
/// that produced it. The text is the agent's own, except where noted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum FrameworkCommit {
    /// The fixed result of a call that was not run because the turn was
    /// cancelled.
    CancelledCall,
    /// The fixed closing note of a cancelled turn.
    CancelNote,
    /// The result recorded for a parked call that a human denied: the host's
    /// denial text, followed by the human's feedback when there is some.
    Denial,
    /// The fixed result of a call that an interrupted approval left
    /// unanswered.
    UnansweredCall,
    /// The fixed result of a call whose completion was not recorded before
    /// an interruption.
    UnrecordedCall,
    /// Text from another agent, framed by this one. It has no source of its
    /// own here, unless it arrived with parts.
    Notification,
    /// A message another agent of the conversation sent to this one's inbox,
    /// recorded as it arrived.
    InboxMessage,
    /// A message a middleware of the host added before a round's request:
    /// data the host found at run time, inside the host's own framing. It
    /// is appended with the parts it is made of.
    HostNote,
}

/// What a middleware's author states about it for an agent that selected a
/// source view boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ViewSupport {
    /// No statement. A selected agent refuses to run with this middleware.
    Unadapted,
    /// None of its hooks adds, removes or rewrites a recorded message or
    /// adds system text, and it does not need the text of a model request.
    /// The agent checks the recorded messages after every hook phase and
    /// refuses the turn when one changed. Its `around_model` is not called on
    /// a selected agent; `around_model_with_view` is.
    ///
    /// The statement covers everything the agent records or carries between
    /// rounds: messages, todos, files, custom state and system additions. It
    /// is checked after every hook phase and around every tool call, where
    /// only the tool itself may change that state.
    Preserving,
    /// Its hooks that change what the agent carries have tracked
    /// counterparts, and on a selected agent only those run:
    ///
    /// - `prepare_model` is not called. `prepare_model_with_view` is, and
    ///   adds system text only as [`SourcePart`]s through [`TrackedPrepare`].
    /// - `review_model` is not called. `review_model_with_view` is, and
    ///   changes the received reply only through [`TrackedReply`].
    /// - `around_tool` runs, and announces through [`TrackedDispatch`] every
    ///   change it makes outside the tool: a bookkeeping key of its own in
    ///   the custom state, or fixed text appended to the outcome.
    ///
    /// Everything else is held to the `Preserving` statement and checked the
    /// same way.
    Tracked,
}

/// One piece of text on its way into a request, with where it came from.
/// Pieces stay apart until the host composes the request. The text is owned
/// by its producer: the same owned piece, handed over again, is the same
/// source, while equal text in another piece is not.
#[derive(Clone)]
pub struct SourcePart {
    pub text: Arc<str>,
    pub from: PartFrom,
}

/// Where a [`SourcePart`] came from, as its producer knows it.
#[derive(Clone)]
#[non_exhaustive]
pub enum PartFrom {
    /// Fixed text of the application: headings, labels, instructions.
    Fixed,
    /// Text the producer read from a store, a file or configuration. `what`
    /// names what was read.
    Read { what: String },
    /// Text a tool call carried in its arguments, which the tool stored. The
    /// agent recorded the reply that asked for the call, the call and the
    /// place in its arguments when it saw the tool store it.
    Call {
        reply: SourceRef,
        call_id: String,
        pointer: String,
    },
    /// The JSON encoding of a value a tool call carried at this place of its
    /// arguments, which the tool repeated in its own outcome.
    CallJson {
        reply: SourceRef,
        call_id: String,
        pointer: String,
    },
    /// The whole text of something the host already holds a source for: a
    /// reply that an agent of the conversation committed, or an argument
    /// source its boundary issued. The host checks the source itself.
    Source { source: SourceRef },
}

/// One piece of a tool's own outcome, as the tool says it made it.
#[derive(Clone)]
#[non_exhaustive]
pub enum OutcomePart {
    /// Fixed text of the tool.
    Fixed(Arc<str>),
    /// The JSON encoding of the value at `pointer` of the call's own
    /// arguments.
    ArgumentJson { pointer: String, text: Arc<str> },
    /// The string at `pointer` of the call's own arguments, as it is.
    Argument { pointer: String, text: Arc<str> },
    /// Text the tool read from configuration or a store. `what` names what
    /// was read.
    Read { what: String, text: Arc<str> },
    /// The whole text of something the host already holds a source for.
    Source { source: SourceRef, text: Arc<str> },
}

impl OutcomePart {
    pub(crate) fn text(&self) -> &str {
        match self {
            OutcomePart::Fixed(text)
            | OutcomePart::ArgumentJson { text, .. }
            | OutcomePart::Argument { text, .. }
            | OutcomePart::Read { text, .. }
            | OutcomePart::Source { text, .. } => text,
        }
    }
}

impl SourcePart {
    pub fn fixed(text: Arc<str>) -> Self {
        Self {
            text,
            from: PartFrom::Fixed,
        }
    }

    pub fn read(what: impl Into<String>, text: Arc<str>) -> Self {
        Self {
            text,
            from: PartFrom::Read { what: what.into() },
        }
    }

    /// The whole text of something the host already holds `source` for.
    pub fn of_source(source: SourceRef, text: Arc<str>) -> Self {
        Self {
            text,
            from: PartFrom::Source { source },
        }
    }
}

/// The text `parts` join to.
pub fn joined(parts: &[SourcePart]) -> String {
    let mut text = String::new();
    for part in parts {
        text.push_str(&part.text);
    }
    text
}

impl fmt::Debug for SourcePart {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SourcePart([opaque])")
    }
}

/// The most sections one round may add, and the most parts and bytes of one
/// section.
pub const MAX_SYSTEM_ADDITIONS: usize = 64;
pub const MAX_ADDITION_PARTS: usize = 1024;
pub const MAX_ADDITION_BYTES: usize = 1_048_576;

/// The application's own text around a summary: the summarizer's system
/// text, the instruction that ends its request, and what opens and closes
/// the summary message.
pub struct SummaryText {
    pub system: SourcePart,
    pub ask: SourcePart,
    pub open: SourcePart,
    pub close: SourcePart,
}

/// The most parts and bytes of one message that arrives with parts.
pub const MAX_ENVELOPE_PARTS: usize = 64;
pub const MAX_ENVELOPE_BYTES: usize = 1_048_576;

/// What the agent keeps beside one recorded message.
#[derive(Clone)]
pub(crate) enum Slot {
    Bound(SourceRef, ActualRetainedCommit),
    /// Appended by the agent itself; the boundary has not been asked yet.
    Pending(ActualRetainedCommit),
    /// Exists only beside the working state until an actual commit.
    StagedSummary(SourceRef, ActualStagedSummary),
    StagedHostMessage(SourceRef, ActualStagedHostMessage),
}

impl Slot {
    pub(crate) fn bound_input(&self) -> Option<(SourceRef, ActualModelInput)> {
        match self {
            Self::Bound(source, commit) => {
                Some((source.clone(), ActualModelInput::Retained(commit.clone())))
            }
            Self::StagedSummary(source, stage) => Some((
                source.clone(),
                ActualModelInput::StagedSummary(stage.clone()),
            )),
            Self::StagedHostMessage(source, stage) => Some((
                source.clone(),
                ActualModelInput::StagedHostMessage(stage.clone()),
            )),
            Self::Pending(_) => None,
        }
    }
}

/// What a middleware with a tracked prepare sees on a selected agent: the
/// state, read-only, one way to add system text, and one way to replace
/// recorded messages with a summary of them.
pub struct TrackedPrepare<'a> {
    pub(crate) state: &'a mut crate::AgentState,
    pub(crate) todos: Option<&'a [PartFrom]>,
    pub(crate) additions: &'a mut Vec<Vec<SourcePart>>,
    pub(crate) boundary: &'a dyn ModelViewBoundary,
    pub(crate) owner: &'a ActualAgentOwner,
    pub(crate) facts: &'a Arc<std::sync::Mutex<ActualFacts>>,
    /// The slots beside the recorded messages as this preparation sees them.
    pub(crate) slots: &'a mut Vec<Slot>,
    /// Whether this preparation replaced recorded messages.
    pub(crate) replaced: &'a mut bool,
}

impl TrackedPrepare<'_> {
    pub fn state(&self) -> &crate::AgentState {
        self.state
    }

    /// Where the text of the stored todo at `index` came from. `None` when
    /// the agent did not see a tool store that todo from its own arguments.
    pub fn todo_origin(&self, index: usize) -> Option<PartFrom> {
        self.todos?.get(index).cloned()
    }

    /// Adds one section to this round's system text, as parts in order.
    pub fn add_system(&mut self, parts: Vec<SourcePart>) -> Result<(), AgentError> {
        let bytes: usize = parts.iter().map(|part| part.text.len()).sum();
        if self.additions.len() >= MAX_SYSTEM_ADDITIONS
            || parts.len() > MAX_ADDITION_PARTS
            || bytes > MAX_ADDITION_BYTES
        {
            return Err(AgentError::State(
                "A system addition of this conversation exceeds its bound.".into(),
            ));
        }
        self.additions.push(parts);
        Ok(())
    }
}

/// What a tool or a middleware announces during one dispatch on a selected
/// agent. Only the tool may change recorded state; anything else that does
/// must say so here, or the turn stops.
#[derive(Clone, Default)]
pub struct TrackedDispatch {
    pub(crate) notes: Arc<std::sync::Mutex<DispatchNotes>>,
    pub(crate) origin: Option<Arc<DispatchOrigin>>,
}

/// Where the running call came from, as the agent recorded it.
pub(crate) struct DispatchOrigin {
    pub(crate) boundary: Arc<dyn ModelViewBoundary>,
    /// The source beside the committed reply that asked for the call.
    pub(crate) reply: SourceRef,
    pub(crate) call_id: String,
    pub(crate) invocation: ActualToolInvocation,
    pub(crate) facts: Arc<std::sync::Mutex<ActualFacts>>,
}

#[derive(Default)]
pub(crate) struct DispatchNotes {
    pub(crate) custom_keys: Vec<String>,
    pub(crate) appended: Vec<SourcePart>,
    pub(crate) todos: Option<Vec<String>>,
    pub(crate) outcome: Option<Vec<OutcomePart>>,
}

impl TrackedDispatch {
    /// Keeps a genuine child return before formatting, source or result
    /// projection. Its parent witness is retained, never guessed from a task
    /// name, call id, registry entry or equal text. This is data, not a grant.
    pub fn retain_task_return(&self, returned: &ActualTaskReturn) {
        if let Some(origin) = &self.origin {
            origin.facts.lock().unwrap().tasks.push(returned.clone());
        }
    }
    /// The actual post-chain invocation of the registered tool, when this
    /// handle was issued at its terminal. No caller call-id can issue one.
    pub fn invocation(&self) -> Option<&ActualToolInvocation> {
        self.origin.as_ref().map(|origin| &origin.invocation)
    }
    /// A middleware keeps bookkeeping of its own under `key` of the custom
    /// state and changes it during this dispatch.
    pub fn owns_custom_key(&self, key: &str) {
        let mut notes = self.notes.lock().unwrap();
        if notes.custom_keys.len() < 16 && !notes.custom_keys.iter().any(|known| known == key) {
            notes.custom_keys.push(key.to_owned());
        }
    }

    /// A middleware appended this fixed text to the end of the outcome it
    /// returns.
    pub fn appended_to_outcome(&self, part: SourcePart) {
        let mut notes = self.notes.lock().unwrap();
        if notes.appended.len() < 16 {
            notes.appended.push(part);
        }
    }

    /// The tool replaced the stored todo list with strings of its own call
    /// arguments: one JSON pointer into those arguments per stored todo, in
    /// order.
    pub fn todos_from_arguments(&self, pointers: Vec<String>) {
        self.notes.lock().unwrap().todos = Some(pointers);
    }

    /// The tool's own outcome content is exactly these pieces, in order.
    /// The agent uses them only if they join to the content the tool
    /// returned; the last statement of a dispatch counts.
    pub fn outcome_is(&self, parts: Vec<OutcomePart>) {
        if parts.len() <= MAX_ADDITION_PARTS {
            self.notes.lock().unwrap().outcome = Some(parts);
        }
    }

    /// The boundary of the conversation the call runs in. `None` when the
    /// agent holds no source for the reply that asked for the call.
    pub fn boundary(&self) -> Option<Arc<dyn ModelViewBoundary>> {
        self.origin.as_ref().map(|origin| origin.boundary.clone())
    }

    /// `text` as the string the running call carries at `pointer` of its
    /// arguments: a part that names the committed reply that asked for the
    /// call. A tool that hands such text to another agent resolves the part
    /// with [`ModelViewBoundary::argument_source`] while the call runs.
    /// `None` when the agent holds no source for that reply.
    pub fn argument(&self, pointer: &str, text: Arc<str>) -> Option<SourcePart> {
        let origin = self.origin.as_ref()?;
        Some(SourcePart {
            text,
            from: PartFrom::Call {
                reply: origin.reply.clone(),
                call_id: origin.call_id.clone(),
                pointer: pointer.to_owned(),
            },
        })
    }
}

/// One change a middleware made to a received reply during its tracked
/// review. Recorded by the agent when the change is made.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct ReplyEdit {
    /// The middleware that made the change.
    pub by: String,
    /// The position of the call among the reply's tool calls.
    pub call: usize,
    /// The arguments as they were before this change.
    pub received: serde_json::Value,
    /// The arguments that replaced them.
    pub replaced: serde_json::Value,
}

/// The reply under review on a selected agent, for a middleware that stated
/// [`ViewSupport::Tracked`]. It can be read freely. It can be changed
/// only through the operations here, each of which is recorded.
pub struct TrackedReply<'a> {
    pub(crate) message: &'a mut Msg,
    pub(crate) edits: &'a mut Vec<ReplyEdit>,
    pub(crate) by: &'a str,
}

impl TrackedReply<'_> {
    /// The reply as it stands, with earlier recorded changes applied.
    pub fn message(&self) -> &Msg {
        self.message
    }

    /// Replaces the arguments of one tool call of the reply. Its text, the
    /// call's id and name and every other call stay as they are.
    pub fn replace_call_arguments(
        &mut self,
        call: usize,
        args: serde_json::Value,
    ) -> Result<(), AgentError> {
        let Msg::Assistant { tool_calls, .. } = &mut *self.message else {
            return Err(AgentError::State(
                "The reply under review is not a reply.".into(),
            ));
        };
        let target = tool_calls.get_mut(call).ok_or_else(|| {
            AgentError::State("The reply under review has no such tool call.".into())
        })?;
        let received = std::mem::replace(&mut target.args, args.clone());
        self.edits.push(ReplyEdit {
            by: self.by.to_owned(),
            call,
            received,
            replaced: args,
        });
        Ok(())
    }
}

impl fmt::Debug for TrackedReply<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TrackedReply([opaque])")
    }
}

/// What a boundary method written without `async` returns.
pub type BoxedBoundaryFuture<'f, T> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<T, AgentError>> + Send + 'f>>;

fn refused<'f, T: Send + 'f>(why: &'static str) -> BoxedBoundaryFuture<'f, T> {
    Box::pin(std::future::ready(Err(AgentError::State(why.into()))))
}

/// Implemented by the host for one conversation. Every method is called by
/// the agent at an actual event; none can change what the agent records.
#[async_trait]
pub trait ModelViewBoundary: Send + Sync {
    /// A real host-note stage; application boundaries validate actual owners
    /// and source parts. The default compatibility bridge grants no authority.
    async fn staged_host_message_source(
        &self,
        stage: &ActualStagedHostMessage,
    ) -> Result<SourceRef, AgentError> {
        self.observe_envelope(FrameworkCommit::HostNote, stage.parts(), stage.message())
            .await
    }

    /// The child builder carries the actual delegating terminal witness.
    /// A host must match it to the private parent that created the boundary.
    fn bind_task_parent(&self, _parent: &ActualToolInvocation) -> Result<(), AgentError> {
        Ok(())
    }
    /// Exact terminal source for a task/message argument. The compatibility
    /// bridge grants no application authority; app boundaries check the
    /// immutable invocation against their actual live operation.
    fn argument_source_for<'a, 'b, 'c, 'f>(
        &'a self,
        invocation: &'b ActualToolInvocation,
        part: &'c SourcePart,
    ) -> BoxedBoundaryFuture<'f, SourceRef>
    where
        'a: 'f,
        'b: 'f,
        'c: 'f,
        Self: 'f,
    {
        let _ = invocation;
        self.argument_source(part)
    }

    /// Creates one child boundary only at its actual delegating invocation.
    fn child_boundary_for<'a, 'b, 'c, 'f>(
        &'a self,
        invocation: &'b ActualToolInvocation,
        system: &'c [SourcePart],
    ) -> BoxedBoundaryFuture<'f, Arc<dyn ModelViewBoundary>>
    where
        'a: 'f,
        'b: 'f,
        'c: 'f,
        Self: 'f,
    {
        let _ = invocation;
        self.child_boundary(system)
    }

    /// A real working-state summary stage, before canonical retention. The
    /// default bridge is for boundaries that supply no application authority.
    async fn staged_summary_source(
        &self,
        stage: &ActualStagedSummary,
    ) -> Result<SourceRef, AgentError> {
        self.summary_source(
            stage.received().transport_receipt().clone(),
            stage.open(),
            stage.close(),
            stage.message(),
        )
        .await
    }

    /// A staged summary actually became the retained conversation and went
    /// through the agent's real save path. This is distinct from staging.
    async fn observe_retained_summary(
        &self,
        commit: &ActualRetainedCommit,
    ) -> Result<SourceRef, AgentError> {
        let stage = commit
            .summary()
            .ok_or_else(|| AgentError::State("The retained summary has no actual stage.".into()))?;
        self.staged_summary_source(stage).await
    }

    /// Checks whether these exact sourced inputs can be projected to a
    /// summarizer. A host must refuse representations it cannot preserve.
    fn validate_summary_inputs(&self, _request: &TrackedModelRequest) -> Result<(), AgentError> {
        Ok(())
    }

    async fn accept_parked_decision(
        &self,
        parked: &ActualParkedCall,
        action: &HitlAction,
        source: &SourceRef,
    ) -> Result<(), AgentError> {
        self.accept_decision(parked.call(), action, source).await
    }

    /// Called by the actual builder for its one selected agent owner. A host
    /// must bind this event owner before accepting the later witnesses.
    fn bind_agent(&self, _owner: &ActualAgentOwner) -> Result<(), AgentError> {
        Ok(())
    }

    /// Closes future required view work after an actual selected failure.
    fn close(&self) {}

    async fn commit_retained_model(
        &self,
        commit: &ActualRetainedCommit,
    ) -> Result<SourceRef, AgentError> {
        let received = commit.received().ok_or_else(|| {
            AgentError::State("The retained model event has no received reply.".into())
        })?;
        self.commit_reviewed_model(
            received.transport_receipt().clone(),
            commit.message(),
            commit.edits(),
        )
        .await
    }

    async fn observe_retained_tool(&self, fact: &ActualToolFact) -> Result<SourceRef, AgentError> {
        let commit = fact.commit().ok_or_else(|| {
            AgentError::State("The chain outcome has no retained tool result.".into())
        })?;
        let Msg::Tool {
            tool_call_id,
            content,
            is_error,
        } = commit.message()
        else {
            return Err(AgentError::State(
                "The retained tool event is not a tool result.".into(),
            ));
        };
        let reached = fact
            .terminals()
            .iter()
            .map(|terminal| terminal.reached().clone())
            .collect::<Vec<_>>();
        let tool = &fact.proposed_call().name;
        self.observe_tool_result(ObservedToolResult {
            call_id: tool_call_id,
            tool,
            content,
            is_error: *is_error,
            reached: &reached,
            returned: fact.returned(),
            appended: fact.appended(),
            parts: fact.parts(),
        })
        .await
    }

    async fn observe_retained_framework(
        &self,
        commit: &ActualRetainedCommit,
    ) -> Result<SourceRef, AgentError> {
        let RetainedEvent::Framework(event) = commit.event() else {
            return Err(AgentError::State(
                "The retained event is not a framework message.".into(),
            ));
        };
        match commit.parts() {
            Some(parts) => self.observe_envelope(event, parts, commit.message()).await,
            None => self.observe_framework_commit(event, commit.message()).await,
        }
    }

    /// Exact post-chain invocation, before any tool effect. Returning a source
    /// gives no permission: the original tool approval rules still apply.
    async fn before_terminal(
        &self,
        _invocation: &ActualToolInvocation,
    ) -> Result<Option<SourceRef>, AgentError> {
        Ok(None)
    }

    /// A final synchronous source-liveness check immediately before polling
    /// the actual tool. It makes no OS containment or rollback guarantee.
    fn terminal_live(&self, _invocation: &ActualToolInvocation) -> Result<(), AgentError> {
        Ok(())
    }
    /// The sources of the system channel for the next request, in order.
    async fn system_sources(&self) -> Result<Vec<SourceRef>, AgentError>;

    /// The source for one registered tool definition.
    async fn tool_source(&self, definition: &ToolDef) -> Result<SourceRef, AgentError>;

    /// A reply was received through `receipt`, reviewed, committed, journaled
    /// and saved. Returns the source to keep beside the committed message.
    async fn commit_model(
        &self,
        receipt: SourceRef,
        committed: &Msg,
    ) -> Result<SourceRef, AgentError>;

    /// A tool result was journaled and committed, and saved if the agent
    /// saves after each result. Returns the source to keep beside it. An
    /// error stops the turn; the result stays recorded.
    async fn observe_tool_result(
        &self,
        result: ObservedToolResult<'_>,
    ) -> Result<SourceRef, AgentError>;

    /// As [`commit_model`](Self::commit_model), for a reply that a middleware
    /// changed during its tracked review. `edits` are those changes, in the
    /// order they were made. With no change this is `commit_model`. The
    /// default refuses a changed reply: a boundary that does not know the
    /// changes cannot say what the committed reply derives from.
    async fn commit_reviewed_model(
        &self,
        receipt: SourceRef,
        committed: &Msg,
        edits: &[ReplyEdit],
    ) -> Result<SourceRef, AgentError> {
        if edits.is_empty() {
            return self.commit_model(receipt, committed).await;
        }
        Err(AgentError::State(
            "This conversation has no source for a reply that was changed in review.".into(),
        ))
    }

    /// One section a middleware adds to this round's system text, as parts.
    /// Returns the source of the section. The default refuses: a boundary
    /// that does not know parts has no view of them.
    async fn system_addition(&self, parts: &[SourcePart]) -> Result<SourceRef, AgentError> {
        let _ = parts;
        Err(AgentError::State(
            "This conversation has no source for a system addition.".into(),
        ))
    }

    /// The source of one message the host itself writes into a request and
    /// does not record: the instruction that ends a summarizer's request.
    /// The default refuses.
    async fn host_message(&self, part: &SourcePart) -> Result<SourceRef, AgentError> {
        let _ = part;
        Err(AgentError::State(
            "This conversation has no source for a host message.".into(),
        ))
    }

    /// A summarizer's reply was received through `receipt`, and `summary` is
    /// the message that replaces the summarized ones: `open`, the reply's
    /// text and `close`, joined. Returns the source to keep beside it. The
    /// default refuses.
    async fn summary_source(
        &self,
        receipt: SourceRef,
        open: &SourcePart,
        close: &SourcePart,
        summary: &Msg,
    ) -> Result<SourceRef, AgentError> {
        let _ = (receipt, open, close, summary);
        Err(AgentError::State(
            "This conversation has no source for a summary.".into(),
        ))
    }

    /// The agent itself appended `committed` at `event`, and it is recorded.
    /// Returns the source to keep beside it. The default refuses: a boundary
    /// that does not know such a message has no view of it.
    async fn observe_framework_commit(
        &self,
        event: FrameworkCommit,
        committed: &Msg,
    ) -> Result<SourceRef, AgentError> {
        let _ = (event, committed);
        Err(AgentError::State(
            "This conversation has no source for a message its agent appended.".into(),
        ))
    }

    /// Called immediately before a call runs: before the dispatch chain for
    /// an ordinary call, and before the tool itself for a call that a human
    /// approved (`approved`). `call` is exactly what will run. An error means
    /// nothing ran, and the call is left without a result.
    async fn before_dispatch(&self, call: &ToolCall, approved: bool) -> Result<(), AgentError> {
        let _ = (call, approved);
        Ok(())
    }

    /// The host's own evidence of which conversation this boundary is for.
    /// The agent keeps it with every request it composes through the
    /// boundary. The default has none.
    fn conversation(&self) -> Option<SourceRef> {
        None
    }

    /// The source of text that the call running now carries in its
    /// arguments, for a tool that hands that text to another agent. `part`
    /// is what [`TrackedDispatch::argument`] returned for it. Called while
    /// that call runs. The default refuses.
    ///
    /// This default and the two after it are written without `async`, in the
    /// shape `async_trait` gives an `async fn`, so that a boundary that does
    /// not override them shares one ready future.
    fn argument_source<'a, 'b, 'f>(
        &'a self,
        part: &'b SourcePart,
    ) -> BoxedBoundaryFuture<'f, SourceRef>
    where
        'a: 'f,
        'b: 'f,
        Self: 'f,
    {
        let _ = part;
        refused("This conversation has no source for text of a call's arguments.")
    }

    /// A boundary for one agent that this conversation's agent delegates a
    /// task to. `system` is that agent's system text, as parts in order. The
    /// new boundary belongs to the same conversation: what one of them holds
    /// a source for, the other can be given as a part. The default refuses.
    fn child_boundary<'a, 'b, 'f>(
        &'a self,
        system: &'b [SourcePart],
    ) -> BoxedBoundaryFuture<'f, Arc<dyn ModelViewBoundary>>
    where
        'a: 'f,
        'b: 'f,
        Self: 'f,
    {
        let _ = system;
        refused("This conversation has no source view for a delegated agent.")
    }

    /// The agent itself appended `committed` at `event` from `parts`, which
    /// join to the message's text: the application's own framing around what
    /// another agent of the conversation sent. It is recorded. Returns the
    /// source to keep beside it. The default refuses.
    fn observe_envelope<'a, 'b, 'c, 'f>(
        &'a self,
        event: FrameworkCommit,
        parts: &'b [SourcePart],
        committed: &'c Msg,
    ) -> BoxedBoundaryFuture<'f, SourceRef>
    where
        'a: 'f,
        'b: 'f,
        'c: 'f,
        Self: 'f,
    {
        let _ = (event, parts, committed);
        refused("This conversation has no source for a message that arrived with parts.")
    }

    /// A human decided the parked `call`. `action` is what is about to be
    /// applied and `decision` is the host's own evidence of that decision.
    /// Called before any effect of the decision. The default refuses.
    async fn accept_decision(
        &self,
        call: &ToolCall,
        action: &HitlAction,
        decision: &SourceRef,
    ) -> Result<(), AgentError> {
        let _ = (call, action, decision);
        Err(AgentError::State(
            "This conversation cannot check a human decision.".into(),
        ))
    }
}

/// The recorded request together with the source of every part. It is
/// composed by the agent and cannot be built or changed by a caller.
pub struct TrackedModelRequest {
    pub(crate) request: ModelRequest,
    pub(crate) system: Vec<SourceRef>,
    pub(crate) messages: Vec<SourceRef>,
    pub(crate) tools: Vec<SourceRef>,
    pub(crate) retained: Vec<ActualRetainedCommit>,
    pub(crate) inputs: Vec<ActualModelInput>,
    pub(crate) returned: Option<Arc<ReturnRecorder>>,
    /// What the boundary this request was composed through says its
    /// conversation is.
    pub(crate) conversation: Option<SourceRef>,
    /// Whether the agent composed this request for a summarizer.
    pub(crate) summary: bool,
    pub(crate) owner: Option<ActualAgentOwner>,
}

impl TrackedModelRequest {
    /// Issued by the actual selected agent that constructed this request.
    /// Host conversation labels and SourceRef erasure cannot replace it.
    pub fn agent_owner(&self) -> Option<&ActualAgentOwner> {
        self.owner.as_ref()
    }
    /// Retains the selected provider's actual returned data before its
    /// fallible receipt/projection work. Available only during the actual
    /// provider terminal call; not a source/HTTP/human/commit certificate.
    /// `false` means no active return owner, or one return was already kept.
    pub fn retain_received_data(&self, response: &ModelResponse) -> bool {
        self.returned
            .as_ref()
            .is_some_and(|owner| owner.retain(response))
    }
    /// Immutable actual-retention witnesses in request order. A summarizer
    /// can also have a staged summary or its unrecorded host instruction;
    /// use [`message_inputs`](Self::message_inputs) to check every item.
    /// The host verifies its bound agent owner and the exact message, not an
    /// ordinal or equal source text. Nothing is loaded from a checkpoint.
    pub fn retained_commits(&self) -> &[ActualRetainedCommit] {
        &self.retained
    }
    /// Exactly one immutable actual-event witness beside every message.
    pub fn message_inputs(&self) -> &[ActualModelInput] {
        &self.inputs
    }
    /// The recorded messages and tool definitions. The system text is empty:
    /// a selected session has no system text outside its sources.
    pub fn recorded(&self) -> &ModelRequest {
        &self.request
    }

    pub fn system_sources(&self) -> &[SourceRef] {
        &self.system
    }

    /// One source per recorded message, in the same order.
    pub fn message_sources(&self) -> &[SourceRef] {
        &self.messages
    }

    /// One source per recorded tool definition, in the same order.
    pub fn tool_sources(&self) -> &[SourceRef] {
        &self.tools
    }

    /// The conversation's own evidence, as the boundary this request was
    /// composed through gave it.
    pub fn conversation(&self) -> Option<&SourceRef> {
        self.conversation.as_ref()
    }

    /// Whether the agent composed this request for a summarizer: the
    /// messages it is about to replace with a summary, and the instruction
    /// to summarize them. It is a new request made from those sources, not a
    /// continuation of the conversation.
    pub fn is_summary(&self) -> bool {
        self.summary
    }
}

impl fmt::Debug for TrackedModelRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TrackedModelRequest([opaque])")
    }
}

/// A reply together with the provider's own receipt of having received it.
pub struct ModelDelivery {
    pub(crate) response: ModelResponse,
    pub(crate) receipt: SourceRef,
}

impl ModelDelivery {
    /// Built by the provider that actually received `response`.
    pub fn new(response: ModelResponse, receipt: SourceRef) -> Self {
        Self { response, receipt }
    }
}
