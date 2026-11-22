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
    /// own here.
    Notification,
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

/// What the agent keeps beside one recorded message.
#[derive(Clone)]
pub(crate) enum Slot {
    Bound(SourceRef),
    /// Appended by the agent itself; the boundary has not been asked yet.
    Pending(FrameworkCommit),
}

/// What a middleware with a tracked prepare sees on a selected agent: the
/// state, read-only, one way to add system text, and one way to replace
/// recorded messages with a summary of them.
pub struct TrackedPrepare<'a> {
    pub(crate) state: &'a mut crate::AgentState,
    pub(crate) todos: Option<&'a [PartFrom]>,
    pub(crate) additions: &'a mut Vec<Vec<SourcePart>>,
    pub(crate) boundary: &'a dyn ModelViewBoundary,
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
}

#[derive(Default)]
pub(crate) struct DispatchNotes {
    pub(crate) custom_keys: Vec<String>,
    pub(crate) appended: Vec<SourcePart>,
    pub(crate) todos: Option<Vec<String>>,
    pub(crate) outcome: Option<Vec<OutcomePart>>,
}

impl TrackedDispatch {
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
/// [`ViewSupport::TrackedReply`]. It can be read freely. It can be changed
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

/// Implemented by the host for one conversation. Every method is called by
/// the agent at an actual event; none can change what the agent records.
#[async_trait]
pub trait ModelViewBoundary: Send + Sync {
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
}

impl TrackedModelRequest {
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
