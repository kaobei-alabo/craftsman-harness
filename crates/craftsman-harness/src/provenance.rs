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

use crate::{AgentError, ModelRequest, ModelResponse, Msg, ToolDef};
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

/// One tool result exactly as it was committed to the conversation.
#[non_exhaustive]
pub struct ObservedToolResult<'a> {
    pub call_id: &'a str,
    pub tool: &'a str,
    pub content: &'a str,
    pub is_error: bool,
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
