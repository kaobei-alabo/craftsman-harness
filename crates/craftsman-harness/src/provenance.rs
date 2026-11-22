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
    /// As `Preserving`, except that its review of a received reply changes
    /// that reply. On a selected agent its `review_model` is not called;
    /// `review_model_with_view` is, and every change it makes goes through
    /// [`TrackedReply`] and is recorded as a derivation of what was received.
    TrackedReply,
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
