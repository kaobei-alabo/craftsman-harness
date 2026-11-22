//! The conversation's side of the framework's source view boundary.
//!
//! The framework keeps one opaque source beside each recorded message and
//! hands them back when a request is composed. This module is what those
//! sources are for one privacy conversation, and how a request made of
//! recorded text and sources becomes a request made of views.
//!
//! A source is minted at an actual event and is one of:
//!
//! - an admitted node, for raw text that went through admission;
//! - a committed model node, for a reply that this conversation's own
//!   transport actually received and the agent then committed;
//! - a tool definition, whose description and schema text were admitted;
//! - a recorded tool result, with what actually ran and returned;
//! - a message the agent appended itself, with the event that produced it.
//!
//! A committed model node is model-origin text. It is not an admitted node
//! and gives no right to restore anything. Nothing here decides an origin
//! from a role, a name, a position or equal bytes: a source is used only if
//! it is this conversation's own object and still fits the recorded item it
//! sits beside.

use super::*;
use craftsman_harness::provenance::{
    FrameworkCommit, ModelViewBoundary, ObservedToolResult, PartFrom, ReachedTool, ReplyEdit,
    SourcePart, SourceRef, TrackedModelRequest,
};
use craftsman_harness::{
    AgentError, HitlAction, ModelRequest, Msg, ToolCall, ToolDef, ToolOutcome,
};
use std::ops::Range;

/// What this conversation keeps beside a recorded item.
enum ViewSource {
    Admitted(AdmittedNode),
    Model(CommittedModelNode),
    Definition {
        description: AdmittedNode,
        parameters: AdmittedNode,
    },
    Result {
        view: ResultView,
        actual: Arc<ActualToolResult>,
    },
    Framework {
        node: AdmittedNode,
        event: FrameworkCommit,
    },
    /// One section a middleware added to the system text, part by part.
    Addition(Vec<AdditionPart>),
}

/// How a recorded tool result is seen by a model.
enum ResultView {
    /// The whole recorded content, captured and admitted as raw text.
    Whole(AdmittedNode),
    /// The content piece by piece, as the tool and the middleware said it
    /// was made: host text as its view, and text of the call's own arguments
    /// as the model wrote it.
    Parts(Vec<AdditionPart>),
}

/// One part of a system addition or of a tool result, as this conversation
/// holds it.
enum AdditionPart {
    /// Host text, fixed or read, that was captured and admitted.
    Admitted(AdmittedNode),
    /// Text the model wrote in the arguments of a call of a committed reply.
    /// It is model-origin text and is sent as the model wrote it.
    Model {
        node: CommittedModelNode,
        text: Arc<str>,
    },
}

/// The most owned host parts one conversation keeps admitted at a time.
const OWNED_PARTS: usize = 512;

/// A reply as this conversation's own transport actually received it. Only
/// that transport creates one, at the moment of receipt.
pub struct ReceivedModel {
    owner: Arc<Inner>,
    message: Msg,
    request: RequestContext,
    native: Option<NativeReceipt>,
}

/// The request a reply answered, as the transport that sent it knows it.
#[derive(Clone, Copy)]
struct RequestContext {
    /// The number of messages in the request, which is the position of the
    /// reply.
    index: usize,
    /// SHA-256 of the exact request bytes that were sent.
    sha256: [u8; 32],
}

/// What this conversation's own native transport actually received with one
/// reply. It exists only in memory, from the moment of receipt. A checkpoint
/// or a history cache cannot make one.
struct NativeReceipt {
    /// Endpoint, model, protocol and credential of the receiving client.
    identity: String,
    /// The provider's own items, signed and opaque ones included, exactly as
    /// received.
    items: Vec<serde_json::Value>,
}

/// The receipt of a reply that the Chat transport for `session` has just
/// received.
/// Called only by that transport, with the request it actually sent.
pub(super) fn received_chat_reply(
    session: &PrivacySession,
    message: &Msg,
    index: usize,
    request_sha256: [u8; 32],
) -> Result<ReceivedModel, SessionStop> {
    session.inner.live()?;
    Ok(ReceivedModel {
        owner: session.inner.clone(),
        message: message.clone(),
        request: RequestContext {
            index,
            sha256: request_sha256,
        },
        native: None,
    })
}

/// The receipt of a reply that a native transport for `session` has just
/// received:
/// the client's identity and the provider's items exactly as received.
/// Called only by that transport, with the request it actually sent.
pub(super) fn received_native_reply(
    session: &PrivacySession,
    message: &Msg,
    identity: &str,
    index: usize,
    request_sha256: [u8; 32],
    items: Vec<serde_json::Value>,
) -> Result<ReceivedModel, SessionStop> {
    let mut received = received_chat_reply(session, message, index, request_sha256)?;
    received.native = Some(NativeReceipt {
        identity: identity.to_owned(),
        items,
    });
    Ok(received)
}

struct ModelInner {
    owner: Arc<Inner>,
    /// The reply as the agent committed it.
    message: Msg,
    /// The reply as it was received.
    received: Msg,
    /// The recorded changes that lead from the received reply to the
    /// committed one, in order.
    edits: Vec<ReplyEdit>,
    request: RequestContext,
    native: Option<NativeReceipt>,
    fields: Option<Vec<SourceField>>,
}

/// The provider items a native request sends back, by message position.
/// Built for one request from the committed model nodes of the conversation.
pub struct NativeReplay {
    items: Vec<Option<Vec<serde_json::Value>>>,
}

impl NativeReplay {
    /// The items this conversation received for the reply at `index`. They
    /// are given only for the reply they were received with.
    pub fn items(&self, index: usize, message: &Msg) -> Option<Vec<serde_json::Value>> {
        matches!(message, Msg::Assistant { .. })
            .then(|| self.items.get(index).cloned().flatten())
            .flatten()
    }

    /// How many replies of the request carry provider items.
    pub fn replies(&self) -> usize {
        self.items.iter().flatten().count()
    }
}

/// A received reply that the agent committed. Model-origin text: separate
/// from admitted raw text, and no authority to restore or rewrite anything.
#[derive(Clone)]
pub struct CommittedModelNode {
    inner: Arc<ModelInner>,
}

impl CommittedModelNode {
    /// Whether this exact session received and committed the reply.
    pub fn committed_by(&self, session: &PrivacySession) -> bool {
        Arc::ptr_eq(&self.inner.owner, &session.inner)
    }

    /// The committed reply, while the session is live.
    pub fn message(&self) -> Result<&Msg, SessionStop> {
        self.inner.owner.live()?;
        Ok(&self.inner.message)
    }

    /// The reply as it was received, before any recorded change.
    pub fn received_message(&self) -> Result<&Msg, SessionStop> {
        self.inner.owner.live()?;
        Ok(&self.inner.received)
    }

    /// The recorded changes between the received and the committed reply.
    pub fn edits(&self) -> Result<&[ReplyEdit], SessionStop> {
        self.inner.owner.live()?;
        Ok(&self.inner.edits)
    }

    /// The SHA-256 of the exact request bytes this reply answered, as the
    /// transport that sent them recorded it.
    pub fn request_sha256(&self) -> Result<[u8; 32], SessionStop> {
        self.inner.owner.live()?;
        Ok(self.inner.request.sha256)
    }

    /// Whether a native transport received the reply, with provider items.
    pub fn has_provider_items(&self) -> Result<bool, SessionStop> {
        self.inner.owner.live()?;
        Ok(self.inner.native.is_some())
    }

    /// Every occurrence in the reply's text of a placeholder this
    /// conversation issued. `None` when two occurrences overlap, so that no
    /// reading of the text is certain.
    pub fn fields(&self) -> Result<Option<&[SourceField]>, SessionStop> {
        self.inner.owner.live()?;
        Ok(self.inner.fields.as_deref())
    }
}

/// Where an occurrence of an issued placeholder sits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FieldPlace {
    /// In the text of a committed reply.
    ReplyText,
    /// In a string value of a tool call's arguments, at this JSON pointer.
    /// The range is in the decoded string, not in the JSON encoding.
    Argument { tool: String, pointer: String },
}

/// One occurrence, in model-origin text, of a placeholder that a commit of
/// this conversation issued. It ties the place to that commit's own record.
/// It states where a known placeholder was written. It is not a permission
/// to put the original there.
#[derive(Clone)]
pub struct SourceField {
    place: FieldPlace,
    range: Range<usize>,
    masked: Arc<MaskedText>,
    index: usize,
    owner: Arc<Inner>,
}

impl SourceField {
    pub fn place(&self) -> &FieldPlace {
        &self.place
    }

    pub fn range(&self) -> Range<usize> {
        self.range.clone()
    }

    /// The live scope, the records of the commit that issued the
    /// placeholder, and the index of its record there. Held for the callback
    /// only.
    pub fn with_record<T>(
        &self,
        read: impl FnOnce(&MappingSession, &ResolutionRecords, usize) -> T,
    ) -> Result<T, SessionStop> {
        self.owner.live()?;
        let records = self
            .masked
            .resolution_records_for_host_fields()
            .map_err(|_| SessionStop::Closed)?;
        let result = read(&self.owner.session, records, self.index);
        self.owner.live()?;
        Ok(result)
    }
}

/// The exact call the agent was about to run when this conversation was
/// asked, the committed reply that asked for it, and where that call's
/// arguments name issued placeholders.
pub struct ApprovedOperation {
    owner: Arc<Inner>,
    model: CommittedModelNode,
    call: ToolCall,
    edited: bool,
    fields: Option<Vec<SourceField>>,
}

impl ApprovedOperation {
    /// Exactly what runs: tool name, call id and arguments.
    pub fn call(&self) -> Result<&ToolCall, SessionStop> {
        self.owner.live()?;
        Ok(&self.call)
    }

    /// The committed reply that asked for the call.
    pub fn model(&self) -> &CommittedModelNode {
        &self.model
    }

    /// Whether a person replaced the arguments. Edited arguments are raw
    /// text from that person, not model-origin text, and have no fields.
    pub fn edited_by_human(&self) -> bool {
        self.edited
    }

    /// Every occurrence of an issued placeholder in a string value of the
    /// arguments the model wrote. `None` when occurrences overlap.
    pub fn fields(&self) -> Result<Option<&[SourceField]>, SessionStop> {
        self.owner.live()?;
        Ok(self.fields.as_deref())
    }
}

/// What actually happened for one recorded tool result.
pub struct ActualToolResult {
    owner: Arc<Inner>,
    call_id: String,
    tool: String,
    operation: Option<Arc<ApprovedOperation>>,
    reached: Vec<ReachedTool>,
    returned: Option<ToolOutcome>,
    appended: Vec<SourcePart>,
}

impl ActualToolResult {
    /// Fixed text that middleware announced it appended to the outcome,
    /// after the tool's own content, in order.
    pub fn appended(&self) -> &[SourcePart] {
        &self.appended
    }

    pub fn call_id(&self) -> &str {
        &self.call_id
    }

    pub fn tool(&self) -> &str {
        &self.tool
    }

    /// The operation that was checked before this call ran.
    pub fn operation(&self) -> Option<&Arc<ApprovedOperation>> {
        self.operation.as_ref()
    }

    /// Every run of the tool itself: the exact call and its own outcome with
    /// metadata, before any middleware. Empty when no tool ran.
    pub fn reached(&self) -> &[ReachedTool] {
        &self.reached
    }

    /// What the dispatch chain returned, with its metadata.
    pub fn returned(&self) -> Option<&ToolOutcome> {
        self.returned.as_ref()
    }
}

/// The host's record of what a person decided about a call it showed them.
struct HumanDecision {
    owner: Arc<Inner>,
    call: ToolCall,
    action: HitlAction,
}

/// Why a request could not be composed from its sources. The session is
/// stopped: a recorded item of the conversation has no usable view.
fn unbound(inner: &Inner) -> SessionStop {
    inner.stop();
    SessionStop::Unbound
}

fn fault(stop: SessionStop) -> AgentError {
    AgentError::State(format!("Privacy stopped this conversation. {stop}"))
}

fn refused(error: AdmitError) -> AgentError {
    match error {
        AdmitError::Stopped(stop) => fault(stop),
        AdmitError::Refused(refusal) => fault(SessionStop::Projection(refusal)),
    }
}

impl AdmittedNode {
    fn captured_len(&self) -> usize {
        self.inner.raw.inner.text.len()
    }

    pub(crate) fn captured_text_is(&self, text: &str) -> bool {
        self.inner.raw.inner.text.as_str() == text
    }

    fn is_accepted_fact(&self) -> bool {
        self.inner.raw.is_accepted_fact()
    }
}

impl Inner {
    /// Every occurrence in `text` of a placeholder this conversation's own
    /// commits issued. `None` when two occurrences overlap.
    async fn fields_in(
        self: &Arc<Self>,
        place: &FieldPlace,
        text: &str,
    ) -> Option<Vec<SourceField>> {
        let ledger = self.operation.lock().await;
        let mut found = Vec::new();
        for known in &ledger.known {
            for (start, matched) in text.match_indices(known.placeholder.as_str()) {
                found.push((start, start + matched.len(), known));
            }
        }
        found.sort_by_key(|(start, end, _)| (*start, *end));
        let mut fields = Vec::with_capacity(found.len());
        let mut covered = 0;
        for (start, end, known) in found {
            if start < covered {
                return None;
            }
            covered = end;
            fields.push(SourceField {
                place: place.clone(),
                range: start..end,
                masked: known.masked.clone(),
                index: known.index,
                owner: self.clone(),
            });
        }
        Some(fields)
    }

    /// The fields of every string value in `args`, by JSON pointer.
    async fn argument_fields(
        self: &Arc<Self>,
        tool: &str,
        args: &serde_json::Value,
    ) -> Option<Vec<SourceField>> {
        let mut leaves = Vec::new();
        string_leaves(args, String::new(), &mut leaves);
        let mut fields = Vec::new();
        for (pointer, text) in leaves {
            let place = FieldPlace::Argument {
                tool: tool.to_owned(),
                pointer,
            };
            fields.extend(self.fields_in(&place, text).await?);
        }
        Some(fields)
    }
}

fn string_leaves<'a>(
    value: &'a serde_json::Value,
    pointer: String,
    leaves: &mut Vec<(String, &'a str)>,
) {
    match value {
        serde_json::Value::String(text) => leaves.push((pointer, text)),
        serde_json::Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                string_leaves(item, format!("{pointer}/{index}"), leaves);
            }
        }
        serde_json::Value::Object(members) => {
            for (name, member) in members {
                let name = name.replace('~', "~0").replace('/', "~1");
                string_leaves(member, format!("{pointer}/{name}"), leaves);
            }
        }
        _ => {}
    }
}

/// A reply with the calls removed, as the agent commits it when the round
/// ends without running them.
fn without_calls(message: &Msg) -> Option<Msg> {
    match message {
        Msg::Assistant { content, .. } => Some(Msg::Assistant {
            content: content.clone(),
            tool_calls: Vec::new(),
        }),
        _ => None,
    }
}

impl PrivacySession {
    /// The source to keep beside the recorded item that `node` was admitted
    /// for. Only a node this session admitted is accepted.
    pub fn source(&self, node: &AdmittedNode) -> Result<SourceRef, SessionStop> {
        self.inner.live()?;
        if !node.admitted_by(self) {
            return Err(SessionStop::Unbound);
        }
        Ok(SourceRef::new(Arc::new(ViewSource::Admitted(node.clone()))))
    }

    /// The request a model may see on the Chat wire. Refused, and the
    /// session stopped, when a reply of the conversation was received by a
    /// native transport: its items are not sent on another wire.
    pub fn chat_view(&self, request: &TrackedModelRequest) -> Result<ModelRequest, SessionStop> {
        let view = self.model_view(request)?;
        let foreign = request.message_sources().iter().any(|source| {
            matches!(source.downcast::<ViewSource>().as_deref(),
                Some(ViewSource::Model(node)) if node.inner.native.is_some())
        });
        if foreign {
            return Err(unbound(&self.inner));
        }
        Ok(view)
    }

    /// The request a model may see on a native wire, with the provider items
    /// to send back. `identity` is the sending client's. Every reply of the
    /// conversation must have been received by a client of exactly that
    /// identity, at exactly its position. A reply without such a receipt, or
    /// with another client's, stops the session before a request exists.
    /// Items are sent back only for a reply that was committed as received.
    pub fn native_view(
        &self,
        request: &TrackedModelRequest,
        identity: &str,
    ) -> Result<(ModelRequest, NativeReplay), SessionStop> {
        let view = self.model_view(request)?;
        let mut items = Vec::with_capacity(view.messages.len());
        for (index, source) in request.message_sources().iter().enumerate() {
            let own = source.downcast::<ViewSource>();
            let Some(ViewSource::Model(node)) = own.as_deref() else {
                items.push(None);
                continue;
            };
            let Some(receipt) = node.inner.native.as_ref().filter(|receipt| {
                receipt.identity == identity && node.inner.request.index == index
            }) else {
                return Err(unbound(&self.inner));
            };
            items.push((node.inner.received == node.inner.message).then(|| receipt.items.clone()));
        }
        Ok((view, NativeReplay { items }))
    }

    /// Called by the interface that actually showed `call` to a person and
    /// took `action` from them. The result is the evidence for
    /// `Agent::resume_with_view`.
    pub fn human_decision(
        &self,
        call: &ToolCall,
        action: &HitlAction,
    ) -> Result<SourceRef, SessionStop> {
        self.inner.live()?;
        Ok(SourceRef::new(Arc::new(HumanDecision {
            owner: self.inner.clone(),
            call: call.clone(),
            action: action.clone(),
        })))
    }

    /// The committed model node behind `source`, if it is one of this
    /// session's.
    pub fn committed_model(&self, source: &SourceRef) -> Option<CommittedModelNode> {
        match &*source.downcast::<ViewSource>()? {
            ViewSource::Model(node) if node.committed_by(self) => Some(node.clone()),
            _ => None,
        }
    }

    /// The admitted node behind `source`, if it is one of this session's.
    pub fn admitted(&self, source: &SourceRef) -> Option<AdmittedNode> {
        match &*source.downcast::<ViewSource>()? {
            ViewSource::Admitted(node)
            | ViewSource::Result {
                view: ResultView::Whole(node),
                ..
            }
            | ViewSource::Framework { node, .. }
                if node.admitted_by(self) =>
            {
                Some(node.clone())
            }
            _ => None,
        }
    }

    /// What actually ran and returned for the tool result behind `source`,
    /// if it is one of this session's.
    pub fn actual_result(&self, source: &SourceRef) -> Option<Arc<ActualToolResult>> {
        match &*source.downcast::<ViewSource>()? {
            ViewSource::Result { actual, .. } if Arc::ptr_eq(&actual.owner, &self.inner) => {
                Some(actual.clone())
            }
            _ => None,
        }
    }

    /// The request a model may see: the recorded request with each text
    /// replaced by the view of its own source. Names, ids, roles, order,
    /// model-origin text and tool-call arguments are the recorded ones.
    ///
    /// Refused, and the session stopped, when any part has no source of this
    /// session, when a source does not fit the item it sits beside, or when
    /// the request has a part this composer does not support.
    pub fn model_view(&self, request: &TrackedModelRequest) -> Result<ModelRequest, SessionStop> {
        let inner = &self.inner;
        inner.live()?;
        let recorded = request.recorded();
        if !recorded.system.is_empty()
            || recorded.messages.len() != request.message_sources().len()
            || recorded.tools.len() != request.tool_sources().len()
        {
            return Err(unbound(inner));
        }
        let own = |source: &SourceRef| source.downcast::<ViewSource>().ok_or(SessionStop::Unbound);
        let view = |node: &AdmittedNode, text: &str| -> Result<String, SessionStop> {
            if !node.admitted_by(self) || !node.captured_text_is(text) {
                return Err(SessionStop::Unbound);
            }
            node.model_view().map(str::to_owned)
        };
        let composed = (|| {
            let mut system = String::new();
            let host = |node: &AdmittedNode| -> Result<String, SessionStop> {
                if !node.admitted_by(self) || !node.is_accepted_fact() {
                    return Err(SessionStop::Unbound);
                }
                node.model_view().map(str::to_owned)
            };
            for (index, source) in request.system_sources().iter().enumerate() {
                if index > 0 {
                    system.push_str("\n\n");
                }
                match &*own(source)? {
                    ViewSource::Admitted(node) => system.push_str(&host(node)?),
                    // A section keeps its parts: host text goes out as its
                    // view, and text a committed call wrote goes out as the
                    // model wrote it.
                    ViewSource::Addition(parts) => {
                        for part in parts {
                            match part {
                                AdditionPart::Admitted(node) => system.push_str(&host(node)?),
                                AdditionPart::Model { node, text } => {
                                    if !node.committed_by(self) {
                                        return Err(SessionStop::Unbound);
                                    }
                                    system.push_str(text);
                                }
                            }
                        }
                    }
                    _ => return Err(SessionStop::Unbound),
                }
            }
            let mut messages = Vec::with_capacity(recorded.messages.len());
            for (message, source) in recorded.messages.iter().zip(request.message_sources()) {
                messages.push(match (message, &*own(source)?) {
                    (Msg::User { content }, ViewSource::Admitted(node)) => Msg::User {
                        content: view(node, content)?,
                    },
                    (
                        Msg::Tool {
                            tool_call_id,
                            content,
                            is_error,
                        },
                        ViewSource::Result {
                            view: result,
                            actual,
                        },
                    ) if actual.call_id == *tool_call_id => Msg::Tool {
                        tool_call_id: tool_call_id.clone(),
                        content: match result {
                            ResultView::Whole(node) => view(node, content)?,
                            ResultView::Parts(parts) => {
                                // The parts, in order, must be exactly the
                                // recorded content.
                                let mut at = 0;
                                let mut seen = String::new();
                                for part in parts {
                                    let (length, piece) = match part {
                                        AdditionPart::Admitted(node) => {
                                            (node.captured_len(), host(node)?)
                                        }
                                        AdditionPart::Model { node, text } => {
                                            if !node.committed_by(self) {
                                                return Err(SessionStop::Unbound);
                                            }
                                            (text.len(), text.to_string())
                                        }
                                    };
                                    let fits =
                                        content.get(at..at + length).is_some_and(|recorded| {
                                            match part {
                                                AdditionPart::Admitted(node) => {
                                                    node.captured_text_is(recorded)
                                                }
                                                AdditionPart::Model { text, .. } => {
                                                    **text == *recorded
                                                }
                                            }
                                        });
                                    if !fits {
                                        return Err(SessionStop::Unbound);
                                    }
                                    at += length;
                                    seen.push_str(&piece);
                                }
                                if at != content.len() {
                                    return Err(SessionStop::Unbound);
                                }
                                seen
                            }
                        },
                        is_error: *is_error,
                    },
                    (
                        Msg::Tool {
                            tool_call_id,
                            content,
                            is_error,
                        },
                        ViewSource::Framework { node, event },
                    ) if matches!(
                        event,
                        FrameworkCommit::CancelledCall
                            | FrameworkCommit::UnansweredCall
                            | FrameworkCommit::UnrecordedCall
                            | FrameworkCommit::Denial
                    ) =>
                    {
                        Msg::Tool {
                            tool_call_id: tool_call_id.clone(),
                            content: view(node, content)?,
                            is_error: *is_error,
                        }
                    }
                    (
                        Msg::Assistant {
                            content,
                            tool_calls,
                        },
                        ViewSource::Framework {
                            node,
                            event: FrameworkCommit::CancelNote,
                        },
                    ) if tool_calls.is_empty() => Msg::Assistant {
                        content: view(node, content)?,
                        tool_calls: Vec::new(),
                    },
                    (Msg::Assistant { .. }, ViewSource::Model(node))
                        if node.committed_by(self) && node.inner.message == *message =>
                    {
                        message.clone()
                    }
                    _ => return Err(SessionStop::Unbound),
                });
            }
            let mut tools = Vec::with_capacity(recorded.tools.len());
            for (definition, source) in recorded.tools.iter().zip(request.tool_sources()) {
                let ViewSource::Definition {
                    description,
                    parameters,
                } = &*own(source)?
                else {
                    return Err(SessionStop::Unbound);
                };
                // A schema is structure: it is sent as recorded, and only if
                // admission found nothing in it.
                if !parameters.admitted_by(self)
                    || !parameters.captured_text_is(&definition.parameters.to_string())
                    || parameters.replacements() != 0
                {
                    return Err(SessionStop::Unbound);
                }
                tools.push(ToolDef {
                    name: definition.name.clone(),
                    description: view(description, &definition.description)?,
                    parameters: definition.parameters.clone(),
                });
            }
            Ok(ModelRequest {
                system,
                messages,
                tools,
            })
        })();
        composed.map_err(|stop| match stop {
            SessionStop::Unbound => unbound(inner),
            other => other,
        })
    }

    /// The boundary to select on the one agent of this conversation. The
    /// system channel is made of `system`, in order: raw host text that was
    /// captured and admitted before the first request. A conversation has one
    /// agent, so the boundary is handed out once; a second request for it is
    /// refused.
    pub fn boundary(
        self: &Arc<Self>,
        system: &[AdmittedNode],
    ) -> Result<Arc<ConversationBoundary>, SessionStop> {
        let mut sources = Vec::with_capacity(system.len());
        for node in system {
            if !node.is_accepted_fact() {
                return Err(SessionStop::Unbound);
            }
            sources.push(self.source(node)?);
        }
        if self.inner.bound.swap(true, Ordering::AcqRel) {
            return Err(SessionStop::Unbound);
        }
        Ok(Arc::new(ConversationBoundary {
            session: self.clone(),
            system: sources,
            definitions: tokio::sync::Mutex::new(Vec::new()),
            owned_parts: tokio::sync::Mutex::new(Vec::new()),
            dispatch: std::sync::Mutex::new(Dispatch::default()),
        }))
    }
}

/// What the boundary remembers between a reply and the calls it asked for.
#[derive(Default)]
struct Dispatch {
    /// The last reply the agent committed.
    model: Option<CommittedModelNode>,
    /// The exact call a checked human decision allows to run next.
    decided: Option<ToolCall>,
    /// The operation checked for the call that is running now.
    in_flight: Option<Arc<ApprovedOperation>>,
}

/// The framework's boundary for one privacy conversation.
pub struct ConversationBoundary {
    session: Arc<PrivacySession>,
    system: Vec<SourceRef>,
    /// The admitted text of each tool definition seen so far, by name. An
    /// entry is reused only while the definition's text is the captured text.
    definitions: tokio::sync::Mutex<Vec<(String, SourceRef)>>,
    /// Host parts that were admitted, each with the owned piece it was
    /// admitted for. A piece is found again only as the same owned object;
    /// equal text in another piece is admitted on its own.
    owned_parts: tokio::sync::Mutex<Vec<(Arc<str>, AdmittedNode)>>,
    dispatch: std::sync::Mutex<Dispatch>,
}

impl ConversationBoundary {
    /// The operation that was checked for the call running now, for a host
    /// component at the tool that needs to know exactly what was approved.
    pub fn operation_in_flight(&self) -> Option<Arc<ApprovedOperation>> {
        self.dispatch.lock().unwrap().in_flight.clone()
    }

    /// The last committed reply, if it asked for exactly this call.
    fn asked_for(&self, call: &ToolCall) -> Option<CommittedModelNode> {
        let model = self.dispatch.lock().unwrap().model.clone()?;
        let asked = matches!(&model.inner.message, Msg::Assistant { tool_calls, .. }
            if tool_calls.iter().any(|asked| asked == call));
        asked.then_some(model)
    }

    async fn admit(&self, origin: RawOrigin, text: &str) -> Result<AdmittedNode, AgentError> {
        let raw = CapturedRaw::from_producer(origin, text);
        self.session.admit_raw(&raw).await.map_err(refused)
    }

    /// The admitted node of one owned host part. The same owned piece is
    /// admitted once; it is recognized by being the same object.
    async fn owned_part(
        &self,
        origin: RawOrigin,
        piece: &Arc<str>,
    ) -> Result<AdmittedNode, AgentError> {
        let mut owned = self.owned_parts.lock().await;
        if let Some((_, node)) = owned.iter().find(|(known, _)| Arc::ptr_eq(known, piece)) {
            return Ok(node.clone());
        }
        let node = self.admit(origin, piece).await?;
        if owned.len() >= OWNED_PARTS {
            owned.drain(..OWNED_PARTS / 2);
        }
        owned.push((piece.clone(), node.clone()));
        Ok(node)
    }

    /// The parts of a system addition or of a tool result, as this
    /// conversation holds them. Host text is admitted. Text a part says a
    /// call of a committed reply wrote is checked against that reply.
    async fn held_parts(&self, parts: &[SourcePart]) -> Result<Vec<AdditionPart>, AgentError> {
        let inner = &self.session.inner;
        inner.live().map_err(fault)?;
        let mut composed = Vec::with_capacity(parts.len());
        for part in parts {
            composed.push(match &part.from {
                PartFrom::Fixed => AdditionPart::Admitted(
                    self.owned_part(RawOrigin::HostStatic, &part.text).await?,
                ),
                PartFrom::Read { .. } => {
                    AdditionPart::Admitted(self.owned_part(RawOrigin::HostFile, &part.text).await?)
                }
                // The part says a call of a committed reply wrote this text
                // at this place of its arguments. That is checked against the
                // committed reply itself.
                PartFrom::Call {
                    reply,
                    call_id,
                    pointer,
                }
                | PartFrom::CallJson {
                    reply,
                    call_id,
                    pointer,
                } => {
                    let node = self
                        .session
                        .committed_model(reply)
                        .ok_or_else(|| fault(unbound(inner)))?;
                    let value = match &node.inner.message {
                        Msg::Assistant { tool_calls, .. } => tool_calls
                            .iter()
                            .find(|call| call.id == *call_id)
                            .and_then(|call| call.args.pointer(pointer)),
                        _ => None,
                    };
                    // Either the string itself, or its JSON encoding, as the
                    // part says.
                    let written = match (&part.from, value) {
                        (PartFrom::Call { .. }, Some(serde_json::Value::String(text))) => {
                            Some(text.clone())
                        }
                        (PartFrom::CallJson { .. }, Some(value)) => {
                            serde_json::to_string(value).ok()
                        }
                        _ => None,
                    };
                    if written.as_deref() != Some(&*part.text) {
                        return Err(fault(unbound(inner)));
                    }
                    AdditionPart::Model {
                        node,
                        text: part.text.clone(),
                    }
                }
                _ => return Err(fault(unbound(inner))),
            });
        }
        Ok(composed)
    }
}

#[async_trait::async_trait]
impl ModelViewBoundary for ConversationBoundary {
    async fn system_sources(&self) -> Result<Vec<SourceRef>, AgentError> {
        self.session.check().map_err(fault)?;
        Ok(self.system.clone())
    }

    async fn tool_source(&self, definition: &ToolDef) -> Result<SourceRef, AgentError> {
        let schema = definition.parameters.to_string();
        let mut definitions = self.definitions.lock().await;
        if let Some((_, source)) = definitions
            .iter()
            .find(|(name, _)| *name == definition.name)
            && let Some(ViewSource::Definition {
                description,
                parameters,
            }) = source.downcast::<ViewSource>().as_deref()
            && description.captured_text_is(&definition.description)
            && parameters.captured_text_is(&schema)
        {
            return Ok(source.clone());
        }
        // Tool text is fixed host text. It is captured where it is first used
        // and admitted like any other raw source.
        let description = self
            .admit(RawOrigin::HostStatic, &definition.description)
            .await?;
        let parameters = self.admit(RawOrigin::HostStatic, &schema).await?;
        if parameters.replacements() != 0 {
            // A private value inside a schema cannot be replaced without
            // changing what the tool accepts.
            self.session.stop();
            return Err(fault(SessionStop::Unbound));
        }
        let source = SourceRef::new(Arc::new(ViewSource::Definition {
            description,
            parameters,
        }));
        definitions.retain(|(name, _)| *name != definition.name);
        definitions.push((definition.name.clone(), source.clone()));
        Ok(source)
    }

    async fn system_addition(&self, parts: &[SourcePart]) -> Result<SourceRef, AgentError> {
        let composed = self.held_parts(parts).await?;
        Ok(SourceRef::new(Arc::new(ViewSource::Addition(composed))))
    }

    async fn commit_model(
        &self,
        receipt: SourceRef,
        committed: &Msg,
    ) -> Result<SourceRef, AgentError> {
        self.commit_reviewed_model(receipt, committed, &[]).await
    }

    async fn commit_reviewed_model(
        &self,
        receipt: SourceRef,
        committed: &Msg,
        edits: &[ReplyEdit],
    ) -> Result<SourceRef, AgentError> {
        let inner = &self.session.inner;
        inner.live().map_err(fault)?;
        // The receipt is this conversation's own. What was committed is what
        // was received with the recorded changes applied in order, or that
        // reply with its calls removed because the round ended without
        // running them. The changes are the agent's own record of what a
        // tracked review did; nothing is worked out from a comparison.
        let Some(received) = receipt
            .downcast::<ReceivedModel>()
            .filter(|received| Arc::ptr_eq(&received.owner, inner))
        else {
            return Err(fault(unbound(inner)));
        };
        let derived = edits
            .iter()
            .try_fold(received.message.clone(), |mut reply, edit| {
                let Msg::Assistant { tool_calls, .. } = &mut reply else {
                    return None;
                };
                let call = tool_calls.get_mut(edit.call)?;
                (call.args == edit.received).then(|| call.args = edit.replaced.clone())?;
                Some(reply)
            });
        let fits = derived.as_ref().is_some_and(|derived| {
            derived == committed || without_calls(derived).as_ref() == Some(committed)
        });
        if !fits {
            return Err(fault(unbound(inner)));
        }
        if received.native.is_some() && !edits.is_empty() {
            // Provider items were validated against the reply as received.
            // The native transport refuses a reply that was changed after
            // that, and so does this conversation.
            inner.stop();
            return Err(AgentError::State(
                "Native model response was changed after validation; tool dispatch was refused."
                    .into(),
            ));
        }
        let native = received.native.as_ref().map(|receipt| NativeReceipt {
            identity: receipt.identity.clone(),
            items: receipt.items.clone(),
        });
        let fields = match committed {
            Msg::Assistant { content, .. } => {
                inner.fields_in(&FieldPlace::ReplyText, content).await
            }
            _ => None,
        };
        let node = CommittedModelNode {
            inner: Arc::new(ModelInner {
                owner: inner.clone(),
                message: committed.clone(),
                received: received.message.clone(),
                edits: edits.to_vec(),
                request: received.request,
                native,
                fields,
            }),
        };
        let mut dispatch = self.dispatch.lock().unwrap();
        dispatch.model = Some(node.clone());
        dispatch.decided = None;
        dispatch.in_flight = None;
        Ok(SourceRef::new(Arc::new(ViewSource::Model(node))))
    }

    async fn observe_tool_result(
        &self,
        result: ObservedToolResult<'_>,
    ) -> Result<SourceRef, AgentError> {
        let operation = {
            let mut dispatch = self.dispatch.lock().unwrap();
            dispatch
                .in_flight
                .take()
                .filter(|operation| operation.call.id == result.call_id)
        };
        // The recorded content is the tool's own only when the last run of
        // the tool returned exactly it. Otherwise the chain wrote it: a
        // refusal or denial on the way, a cut, or the text of a failure.
        let own = matches!(result.reached.last().map(|run| &run.result),
            Some(Ok(outcome)) if outcome.content == result.content);
        let origin = if own {
            RawOrigin::ToolResult
        } else {
            RawOrigin::ChainResult
        };
        // The content is an accepted fact: it is admitted or the session
        // stops. It is never replaced. When the tool said what its own
        // content is made of, each piece is held on its own: host text is
        // admitted, and text of the call's own arguments stays model text.
        let view = if result.parts.is_empty() {
            ResultView::Whole(self.admit(origin, result.content).await?)
        } else {
            let mut parts = self.held_parts(result.parts).await?;
            parts.extend(self.held_parts(result.appended).await?);
            ResultView::Parts(parts)
        };
        let actual = Arc::new(ActualToolResult {
            owner: self.session.inner.clone(),
            call_id: result.call_id.to_owned(),
            tool: result.tool.to_owned(),
            operation,
            reached: result.reached.to_vec(),
            returned: result.returned.cloned(),
            appended: result.appended.to_vec(),
        });
        Ok(SourceRef::new(Arc::new(ViewSource::Result {
            view,
            actual,
        })))
    }

    async fn observe_framework_commit(
        &self,
        event: FrameworkCommit,
        committed: &Msg,
    ) -> Result<SourceRef, AgentError> {
        let inner = &self.session.inner;
        let (origin, text) = match (event, committed) {
            (
                FrameworkCommit::CancelledCall
                | FrameworkCommit::UnansweredCall
                | FrameworkCommit::UnrecordedCall,
                Msg::Tool { content, .. },
            ) => (RawOrigin::HostStatic, content),
            (
                FrameworkCommit::CancelNote,
                Msg::Assistant {
                    content,
                    tool_calls,
                },
            ) if tool_calls.is_empty() => (RawOrigin::HostStatic, content),
            // The host's denial text followed by what the person wrote.
            (FrameworkCommit::Denial, Msg::Tool { content, .. }) => {
                (RawOrigin::HumanReply, content)
            }
            // Text from another agent arrives without a source of its own.
            _ => return Err(fault(unbound(inner))),
        };
        let node = self.admit(origin, text).await?;
        Ok(SourceRef::new(Arc::new(ViewSource::Framework {
            node,
            event,
        })))
    }

    async fn before_dispatch(&self, call: &ToolCall, approved: bool) -> Result<(), AgentError> {
        let inner = &self.session.inner;
        inner.live().map_err(fault)?;
        let decided = self.dispatch.lock().unwrap().decided.take();
        let (model, edited) = if approved {
            // Exactly the call a checked decision allowed, and nothing else.
            let model = self.dispatch.lock().unwrap().model.clone();
            match (decided, model) {
                (Some(decided), Some(model)) if decided == *call => {
                    let edited = self.asked_for(call).is_none();
                    (model, edited)
                }
                _ => return Err(fault(unbound(inner))),
            }
        } else {
            match self.asked_for(call) {
                Some(model) => (model, false),
                None => return Err(fault(unbound(inner))),
            }
        };
        let fields = if edited {
            Some(Vec::new())
        } else {
            inner.argument_fields(&call.name, &call.args).await
        };
        self.dispatch.lock().unwrap().in_flight = Some(Arc::new(ApprovedOperation {
            owner: inner.clone(),
            model,
            call: call.clone(),
            edited,
            fields,
        }));
        Ok(())
    }

    async fn accept_decision(
        &self,
        call: &ToolCall,
        action: &HitlAction,
        decision: &SourceRef,
    ) -> Result<(), AgentError> {
        let inner = &self.session.inner;
        inner.live().map_err(fault)?;
        // A decision that is not this conversation's, or is about another
        // call or action, is turned away. Nothing has happened yet, so the
        // call stays parked and the conversation continues.
        let fits = decision.downcast::<HumanDecision>().is_some_and(|decided| {
            Arc::ptr_eq(&decided.owner, inner) && decided.call == *call && decided.action == *action
        });
        if !fits || self.asked_for(call).is_none() {
            return Err(AgentError::State(
                "This decision does not belong to the call that is waiting.".into(),
            ));
        }
        let run = match action {
            HitlAction::Approve => Some(call.clone()),
            HitlAction::ApproveEdited { args } => Some(ToolCall {
                id: call.id.clone(),
                name: call.name.clone(),
                args: args.clone(),
            }),
            HitlAction::Deny { .. } => None,
        };
        self.dispatch.lock().unwrap().decided = run;
        Ok(())
    }
}

impl std::fmt::Debug for ConversationBoundary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ConversationBoundary([private])")
    }
}

impl std::fmt::Debug for CommittedModelNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CommittedModelNode([private])")
    }
}

impl std::fmt::Debug for ReceivedModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ReceivedModel([private])")
    }
}

impl std::fmt::Debug for SourceField {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SourceField([private])")
    }
}

impl std::fmt::Debug for ApprovedOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ApprovedOperation([private])")
    }
}

impl std::fmt::Debug for ActualToolResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ActualToolResult([private])")
    }
}
