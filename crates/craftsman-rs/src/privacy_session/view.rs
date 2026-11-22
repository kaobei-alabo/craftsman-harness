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
//! - a message the agent appended itself, with the event that produced it;
//! - text of a call's arguments that a tool handed to a delegated agent;
//! - a message the agent appended from parts: its own framing around what
//!   another agent of the conversation sent.
//!
//! A conversation has one root agent and the agents it delegates tasks to.
//! Each has a lane of its own: its replies and results are its own, and a
//! request carries only those of the lane it was composed for. All lanes
//! share the session, its vault and its placeholders. Text crosses from one
//! lane to another only as a part that names the source it came from.
//!
//! A committed model node is model-origin text. It is not an admitted node
//! and gives no right to restore anything. Nothing here decides an origin
//! from a role, a name, a position or equal bytes: a source is used only if
//! it is this conversation's own object and still fits the recorded item it
//! sits beside.

use super::*;
use craftsman_harness::provenance::{
    ActualAgentOwner, ActualModelInput, ActualRetainedCommit, ActualStagedSummary, ActualToolFact,
    ActualToolInvocation, FrameworkCommit, ModelViewBoundary, ObservedToolResult, PartFrom,
    ReachedTool, ReplyEdit, RetainedEvent, SourcePart, SourceRef, TrackedModelRequest,
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
        commit: ActualRetainedCommit,
    },
    Framework {
        node: AdmittedNode,
        event: FrameworkCommit,
        commit: ActualRetainedCommit,
    },
    /// One section a middleware added to the system text, part by part.
    Addition(Vec<AdditionPart>),
    /// The message that replaced summarized ones: the application's opening
    /// and closing text around what a summarizer of this conversation
    /// returned. That text is model-origin, whatever role the message has.
    Summary {
        stage: ActualStagedSummary,
        commit: Option<ActualRetainedCommit>,
        open: AdmittedNode,
        text: String,
        close: AdmittedNode,
    },
    /// Text that a call of a committed reply carried in its arguments and
    /// that a tool handed to another agent. It is model-origin text.
    Argument {
        invocation: ActualToolInvocation,
        node: CommittedModelNode,
        text: Arc<str>,
    },
    /// A message the agent appended from parts, part by part.
    Envelope {
        parts: Vec<AdditionPart>,
        commit: ActualRetainedCommit,
    },
}

/// One agent's side of a conversation: the root agent's, or that of an
/// agent the conversation delegated a task to. It is this session's own
/// object, made where the agent's boundary is made.
pub(super) struct Lane {
    owner: Arc<Inner>,
    /// The lane of the agent that delegated the task. `None` for the root.
    parent: Option<Arc<Lane>>,
    invocation: Option<ActualToolInvocation>,
    parent_bound: std::sync::atomic::AtomicBool,
    agent: std::sync::Mutex<Option<ActualAgentOwner>>,
}

impl Lane {
    /// Whether `other` is this lane, the lane that delegated to it, or a
    /// lane it delegated to. Text crosses only between such lanes.
    fn related(self: &Arc<Self>, other: &Arc<Lane>) -> bool {
        let parent_of = |child: &Arc<Lane>, parent: &Arc<Lane>| {
            child
                .parent
                .as_ref()
                .is_some_and(|own| Arc::ptr_eq(own, parent))
        };
        Arc::ptr_eq(self, other) || parent_of(self, other) || parent_of(other, self)
    }
}

/// The lane a request was composed for, if it is one of `inner`'s.
fn lane_of(inner: &Arc<Inner>, request: &TrackedModelRequest) -> Option<Arc<Lane>> {
    let owner = request.agent_owner()?;
    inner
        .lanes
        .lock()
        .unwrap()
        .iter()
        .filter_map(std::sync::Weak::upgrade)
        .find(|lane| {
            Arc::ptr_eq(&lane.owner, inner)
                && lane
                    .agent
                    .lock()
                    .unwrap()
                    .as_ref()
                    .is_some_and(|actor| actor.same_agent(owner))
        })
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
    /// The lane the answered request was composed for.
    lane: Arc<Lane>,
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
    request: &TrackedModelRequest,
    message: &Msg,
    index: usize,
    request_sha256: [u8; 32],
) -> Result<ReceivedModel, SessionStop> {
    session.inner.live()?;
    let lane = lane_of(&session.inner, request).ok_or(SessionStop::Unbound)?;
    Ok(ReceivedModel {
        owner: session.inner.clone(),
        lane,
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
    request: &TrackedModelRequest,
    message: &Msg,
    identity: &str,
    index: usize,
    request_sha256: [u8; 32],
    items: Vec<serde_json::Value>,
) -> Result<ReceivedModel, SessionStop> {
    let mut received = received_chat_reply(session, request, message, index, request_sha256)?;
    received.native = Some(NativeReceipt {
        identity: identity.to_owned(),
        items,
    });
    Ok(received)
}

struct ModelInner {
    owner: Arc<Inner>,
    commit: ActualRetainedCommit,
    /// The lane of the agent that committed the reply.
    lane: Arc<Lane>,
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

    /// The position the reply was received for: the number of messages in
    /// the request it answered. A compaction may leave the reply at another
    /// position; what it was received for does not change.
    pub fn request_position(&self) -> Result<usize, SessionStop> {
        self.inner.owner.live()?;
        Ok(self.inner.request.index)
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
    invocation: ActualToolInvocation,
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
    /// The lane of the agent that recorded the result.
    lane: Arc<Lane>,
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

fn retained_input_is(input: &ActualModelInput, commit: &ActualRetainedCommit) -> bool {
    matches!(input, ActualModelInput::Retained(actual) if actual.same_commit(commit))
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

    /// Replays original native items for this actual lane and client, even
    /// when compaction moved their unchanged committed message. A changed
    /// native reply refuses; no position/portable fallback grants authority.
    pub fn native_view(
        &self,
        request: &TrackedModelRequest,
        identity: &str,
    ) -> Result<(ModelRequest, NativeReplay), SessionStop> {
        if request.is_summary() {
            return Err(unbound(&self.inner));
        }
        let view = self.model_view(request)?;
        let mut items = Vec::with_capacity(view.messages.len());
        for source in request.message_sources() {
            let own = source.downcast::<ViewSource>();
            let Some(ViewSource::Model(node)) = own.as_deref() else {
                items.push(None);
                continue;
            };
            let receipt = node
                .inner
                .native
                .as_ref()
                .filter(|receipt| receipt.identity == identity)
                .ok_or_else(|| unbound(&self.inner))?;
            if node.inner.received != node.inner.message {
                return Err(unbound(&self.inner));
            }
            items.push(Some(receipt.items.clone()));
        }
        Ok((view, NativeReplay { items }))
    }

    /// A new explicit summary request, composed by the framework from actual
    /// source/event inputs. It projects committed text/calls to the summarizer;
    /// original opaque items stay retained with their private source receipts.
    pub fn summary_view(&self, request: &TrackedModelRequest) -> Result<ModelRequest, SessionStop> {
        if !request.is_summary() {
            return Err(unbound(&self.inner));
        }
        self.model_view(request)
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
        let actor = request.agent_owner().ok_or(SessionStop::Unbound)?;
        if request.message_inputs().len() != recorded.messages.len()
            || request
                .message_inputs()
                .iter()
                .zip(&recorded.messages)
                .any(|(input, message)| {
                    !input.owner().same_agent(&actor) || input.message() != message
                })
        {
            return Err(unbound(inner));
        }

        if !recorded.system.is_empty()
            || recorded.messages.len() != request.message_sources().len()
            || recorded.tools.len() != request.tool_sources().len()
        {
            return Err(unbound(inner));
        }
        // The lane the agent composed this request for. Replies and results
        // are a request's own only if that lane's agent committed them.
        let Some(lane) = lane_of(inner, request) else {
            return Err(unbound(inner));
        };
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
            // Model text inside a part is a reply's of this lane, of the
            // lane that delegated to it, or of a lane it delegated to.
            let written = |node: &CommittedModelNode| {
                node.committed_by(self) && lane.related(&node.inner.lane)
            };
            // Parts that are, in order, exactly `content`: host text goes
            // out as its view, and model text goes out as the model wrote it.
            let pieces = |parts: &[AdditionPart], content: &str| -> Result<String, SessionStop> {
                let mut at = 0;
                let mut seen = String::new();
                for part in parts {
                    let (length, piece) = match part {
                        AdditionPart::Admitted(node) => (node.captured_len(), host(node)?),
                        AdditionPart::Model { node, text } => {
                            if !written(node) {
                                return Err(SessionStop::Unbound);
                            }
                            (text.len(), text.to_string())
                        }
                    };
                    let fits = content
                        .get(at..at + length)
                        .is_some_and(|recorded| match part {
                            AdditionPart::Admitted(node) => node.captured_text_is(recorded),
                            AdditionPart::Model { text, .. } => **text == *recorded,
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
                Ok(seen)
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
                                    if !written(node) {
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
            for ((message, source), input) in recorded
                .messages
                .iter()
                .zip(request.message_sources())
                .zip(request.message_inputs())
            {
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
                            commit,
                        },
                    ) if actual.call_id == *tool_call_id
                        && commit.message() == message
                        && retained_input_is(input, commit)
                        && Arc::ptr_eq(&actual.lane, &lane) =>
                    {
                        Msg::Tool {
                            tool_call_id: tool_call_id.clone(),
                            content: match result {
                                ResultView::Whole(node) => view(node, content)?,
                                // The parts, in order, must be exactly the
                                // recorded content.
                                ResultView::Parts(parts) => pieces(parts, content)?,
                            },
                            is_error: *is_error,
                        }
                    }
                    // What the agent appended from parts: its own framing
                    // around what another agent of the conversation sent.
                    (Msg::User { content }, ViewSource::Envelope { parts, commit })
                        if retained_input_is(input, commit) && commit.message() == message =>
                    {
                        Msg::User {
                            content: pieces(parts, content)?,
                        }
                    }
                    // The input of a delegated agent: text that a call of the
                    // delegating agent's committed reply wrote. It goes out
                    // as the model wrote it.
                    (
                        Msg::User { content },
                        ViewSource::Argument {
                            invocation,
                            node,
                            text,
                        },
                    ) if node.committed_by(self)
                        && lane
                            .parent
                            .as_ref()
                            .is_some_and(|parent| Arc::ptr_eq(parent, &node.inner.lane))
                        && lane
                            .invocation
                            .as_ref()
                            .is_some_and(|parent| parent.same_invocation(invocation))
                        && **text == **content =>
                    {
                        Msg::User {
                            content: content.clone(),
                        }
                    }
                    (
                        Msg::Tool {
                            tool_call_id,
                            content,
                            is_error,
                        },
                        ViewSource::Framework {
                            node,
                            event,
                            commit,
                        },
                    ) if commit.message() == message
                        && retained_input_is(input, commit)
                        && matches!(
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
                            commit,
                        },
                    ) if tool_calls.is_empty()
                        && commit.message() == message
                        && retained_input_is(input, commit) =>
                    {
                        Msg::Assistant {
                            content: view(node, content)?,
                            tool_calls: Vec::new(),
                        }
                    }
                    (Msg::Assistant { .. }, ViewSource::Model(node))
                        if node.committed_by(self)
                            && retained_input_is(input, &node.inner.commit)
                            && node.inner.commit.message() == message
                            && Arc::ptr_eq(&node.inner.lane, &lane)
                            && node.inner.message == *message =>
                    {
                        message.clone()
                    }
                    (
                        Msg::User { content },
                        ViewSource::Summary {
                            stage,
                            commit,
                            open,
                            text,
                            close,
                        },
                    ) => {
                        let bound = match (input, commit) {
                            (ActualModelInput::StagedSummary(actual), None) => {
                                actual.same_stage(stage)
                            }
                            (ActualModelInput::Retained(actual), Some(commit)) => {
                                actual.same_commit(commit)
                            }
                            _ => false,
                        };
                        if !bound || stage.message() != message {
                            return Err(SessionStop::Unbound);
                        }
                        // The recorded summary is the opening text, the
                        // summarizer's text and the closing text, exactly.
                        let (first, last) = (open.captured_len(), content.len());
                        let fits = last.checked_sub(close.captured_len()).is_some_and(|tail| {
                            first <= tail
                                && content.is_char_boundary(first)
                                && content.is_char_boundary(tail)
                                && open.captured_text_is(&content[..first])
                                && content[first..tail] == **text
                                && close.captured_text_is(&content[tail..])
                        });
                        if !fits {
                            return Err(SessionStop::Unbound);
                        }
                        Msg::User {
                            content: format!("{}{text}{}", host(open)?, host(close)?),
                        }
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
            lane: Arc::new(Lane {
                owner: self.inner.clone(),
                parent: None,
                invocation: None,
                parent_bound: std::sync::atomic::AtomicBool::new(true),
                agent: std::sync::Mutex::new(None),
            }),
            system: sources,
            definitions: tokio::sync::Mutex::new(Vec::new()),
            owned_parts: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            dispatch: std::sync::Mutex::new(Dispatch::default()),
            agent: std::sync::Mutex::new(None),
            lanes: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }))
    }
}

/// The most agents one conversation delegates tasks to over its life.
const MAX_LANES: usize = 4096;

/// What the boundary remembers between a reply and the calls it asked for.
struct Decided {
    call: ToolCall,
    action: HitlAction,
    source: SourceRef,
}
#[derive(Default)]
struct Dispatch {
    /// The last reply the agent committed.
    model: Option<CommittedModelNode>,
    /// The exact call a checked human decision allows to run next.
    decided: Option<Decided>,
    /// The operation checked for the call that is running now.
    in_flight: Option<Arc<ApprovedOperation>>,
}

/// The framework's boundary for one agent of a privacy conversation: the
/// root agent's, which the session hands out once, or one that a boundary of
/// the conversation made for an agent it delegated a task to.
pub struct ConversationBoundary {
    session: Arc<PrivacySession>,
    /// This agent's lane.
    lane: Arc<Lane>,
    system: Vec<SourceRef>,
    /// The admitted text of each tool definition seen so far, by name. An
    /// entry is reused only while the definition's text is the captured text.
    definitions: tokio::sync::Mutex<Vec<(String, SourceRef)>>,
    /// Host parts that were admitted, each with the owned piece it was
    /// admitted for. A piece is found again only as the same owned object;
    /// equal text in another piece is admitted on its own.
    /// Shared by every boundary of the conversation, so that a piece is
    /// admitted once whichever agent hands it over.
    owned_parts: Arc<tokio::sync::Mutex<Vec<(Arc<str>, AdmittedNode)>>>,
    dispatch: std::sync::Mutex<Dispatch>,
    agent: std::sync::Mutex<Option<ActualAgentOwner>>,
    /// How many lanes the conversation has made for delegated agents.
    lanes: Arc<std::sync::atomic::AtomicUsize>,
}

impl ConversationBoundary {
    pub(crate) fn require_agent(&self, owner: &ActualAgentOwner) -> Result<(), AgentError> {
        self.session.check().map_err(fault)?;
        if !self
            .agent
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|held| held.same_agent(owner))
        {
            return Err(fault(SessionStop::Unbound));
        }
        Ok(())
    }

    async fn summary_node(
        &self,
        stage: &ActualStagedSummary,
        commit: Option<ActualRetainedCommit>,
    ) -> Result<SourceRef, AgentError> {
        self.require_agent(stage.owner())?;
        self.require_agent(stage.received().owner())?;
        let inner = &self.session.inner;
        let received = stage
            .received()
            .transport_receipt()
            .downcast::<ReceivedModel>()
            .filter(|received| Arc::ptr_eq(&received.owner, inner))
            .filter(|received| Arc::ptr_eq(&received.lane, &self.lane))
            .ok_or_else(|| fault(unbound(inner)))?;
        if received.message != *stage.received().message() {
            return Err(fault(unbound(inner)));
        }
        let text = match &received.message {
            Msg::Assistant { content, .. }
            | Msg::User { content }
            | Msg::System { content }
            | Msg::Tool { content, .. } => content.clone(),
        };
        let (open, close) = (stage.open(), stage.close());
        let fits = matches!(stage.message(), Msg::User { content }
            if *content == format!("{}{text}{}", open.text, close.text));
        if !fits || !matches!(open.from, PartFrom::Fixed) || !matches!(close.from, PartFrom::Fixed)
        {
            return Err(fault(unbound(inner)));
        }
        let open = self.owned_part(RawOrigin::HostStatic, &open.text).await?;
        let close = self.owned_part(RawOrigin::HostStatic, &close.text).await?;
        Ok(SourceRef::new(Arc::new(ViewSource::Summary {
            stage: stage.clone(),
            commit,
            open,
            text,
            close,
        })))
    }

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
                        .filter(|node| self.lane.related(&node.inner.lane))
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
                // The part says its text is the whole text of something this
                // conversation already holds a source for. That source is
                // checked itself: it is this session's, it belongs to this
                // agent or to one it delegates to or was delegated by, and
                // its text is exactly the part's.
                PartFrom::Source { source } => {
                    let held = source.downcast::<ViewSource>();
                    let near = |node: &CommittedModelNode| {
                        node.committed_by(&self.session) && self.lane.related(&node.inner.lane)
                    };
                    match held.as_deref() {
                        Some(ViewSource::Model(node))
                            if near(node)
                                && matches!(&node.inner.message,
                                    Msg::Assistant { content, .. } if **content == *part.text) =>
                        {
                            AdditionPart::Model {
                                node: node.clone(),
                                text: part.text.clone(),
                            }
                        }
                        Some(ViewSource::Argument { node, text, .. })
                            if near(node) && **text == *part.text =>
                        {
                            AdditionPart::Model {
                                node: node.clone(),
                                text: part.text.clone(),
                            }
                        }
                        Some(ViewSource::Admitted(node) | ViewSource::Framework { node, .. })
                            if node.admitted_by(&self.session)
                                && node.is_accepted_fact()
                                && node.captured_text_is(&part.text) =>
                        {
                            AdditionPart::Admitted(node.clone())
                        }
                        _ => return Err(fault(unbound(inner))),
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
    fn bind_task_parent(&self, parent: &ActualToolInvocation) -> Result<(), AgentError> {
        self.session.check().map_err(fault)?;
        if !self
            .lane
            .invocation
            .as_ref()
            .is_some_and(|expected| expected.same_invocation(parent))
        {
            return Err(fault(SessionStop::Unbound));
        }
        self.lane.parent_bound.store(true, Ordering::Release);
        Ok(())
    }

    fn bind_agent(&self, owner: &ActualAgentOwner) -> Result<(), AgentError> {
        self.session.check().map_err(fault)?;
        if !self.lane.parent_bound.load(Ordering::Acquire) {
            return Err(fault(SessionStop::Unbound));
        }
        let mut agent = self.agent.lock().unwrap();
        if agent.is_some() {
            return Err(fault(SessionStop::Unbound));
        }
        // Registry -> lane is the one internal order, also used by request
        // lookup. Only this actual builder event registers an agent owner.
        let mut registered = self.session.inner.lanes.lock().unwrap();
        if registered
            .iter()
            .filter_map(std::sync::Weak::upgrade)
            .any(|lane| {
                lane.agent
                    .lock()
                    .unwrap()
                    .as_ref()
                    .is_some_and(|held| held.same_agent(owner))
            })
        {
            return Err(fault(SessionStop::Unbound));
        }
        let mut lane_owner = self.lane.agent.lock().unwrap();
        if lane_owner.is_some() {
            return Err(fault(SessionStop::Unbound));
        }
        if self.lane.parent.is_none() {
            let mut root_owner = self.session.inner.agent_owner.lock().unwrap();
            if root_owner.is_some() {
                return Err(fault(SessionStop::Unbound));
            }
            *root_owner = Some(owner.clone());
        }
        *agent = Some(owner.clone());
        *lane_owner = Some(owner.clone());
        registered.push(Arc::downgrade(&self.lane));
        Ok(())
    }
    fn close(&self) {
        self.session.stop();
    }

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

    async fn host_message(&self, part: &SourcePart) -> Result<SourceRef, AgentError> {
        let inner = &self.session.inner;
        inner.live().map_err(fault)?;
        let origin = match &part.from {
            PartFrom::Fixed => RawOrigin::HostStatic,
            PartFrom::Read { .. } => RawOrigin::HostFile,
            _ => return Err(fault(unbound(inner))),
        };
        let node = self.owned_part(origin, &part.text).await?;
        Ok(SourceRef::new(Arc::new(ViewSource::Admitted(node))))
    }

    fn validate_summary_inputs(&self, request: &TrackedModelRequest) -> Result<(), AgentError> {
        self.session
            .summary_view(request)
            .map(|_| ())
            .map_err(fault)
    }

    fn conversation(&self) -> Option<SourceRef> {
        Some(SourceRef::new(self.lane.clone()))
    }

    async fn argument_source(&self, _: &SourcePart) -> Result<SourceRef, AgentError> {
        Err(fault(unbound(&self.session.inner)))
    }

    async fn argument_source_for(
        &self,
        invocation: &ActualToolInvocation,
        part: &SourcePart,
    ) -> Result<SourceRef, AgentError> {
        let inner = &self.session.inner;
        inner.live().map_err(fault)?;
        let PartFrom::Call {
            reply,
            call_id,
            pointer,
        } = &part.from
        else {
            return Err(fault(unbound(inner)));
        };
        self.require_agent(invocation.owner())?;
        // The call that is running now, exactly as it was checked before it
        // ran, carries this text at this place of its arguments.
        let operation = self
            .operation_in_flight()
            .filter(|operation| {
                operation.invocation.same_invocation(invocation)
                    && operation.call == *invocation.call()
                    && operation.call.id == *call_id
            })
            .ok_or_else(|| fault(unbound(inner)))?;
        let carried = matches!(operation.call.args.pointer(pointer),
            Some(serde_json::Value::String(text)) if **text == *part.text);
        if !carried {
            return Err(fault(unbound(inner)));
        }
        if operation.edited {
            // A person replaced the arguments. The text is raw text from
            // that person, and is admitted like any other.
            let node = self.admit(RawOrigin::HumanReply, &part.text).await?;
            return Ok(SourceRef::new(Arc::new(ViewSource::Admitted(node))));
        }
        // The reply the part names is the committed reply of this agent that
        // asked for the call.
        let node = self
            .session
            .committed_model(reply)
            .filter(|node| {
                Arc::ptr_eq(&node.inner, &operation.model.inner)
                    && Arc::ptr_eq(&node.inner.lane, &self.lane)
            })
            .ok_or_else(|| fault(unbound(inner)))?;
        Ok(SourceRef::new(Arc::new(ViewSource::Argument {
            invocation: invocation.clone(),
            node,
            text: part.text.clone(),
        })))
    }

    async fn child_boundary(
        &self,
        _: &[SourcePart],
    ) -> Result<Arc<dyn ModelViewBoundary>, AgentError> {
        Err(fault(unbound(&self.session.inner)))
    }

    async fn child_boundary_for(
        &self,
        invocation: &ActualToolInvocation,
        system: &[SourcePart],
    ) -> Result<Arc<dyn ModelViewBoundary>, AgentError> {
        self.require_agent(invocation.owner())?;
        if !self
            .operation_in_flight()
            .is_some_and(|operation| operation.invocation.same_invocation(invocation))
        {
            return Err(fault(unbound(&self.session.inner)));
        }
        let inner = &self.session.inner;
        inner.live().map_err(fault)?;
        if self.lanes.fetch_add(1, Ordering::AcqRel) >= MAX_LANES {
            return Err(AgentError::State(
                "This conversation has delegated as many tasks as it can.".into(),
            ));
        }
        // The delegated agent's system text is host text, kept apart from
        // this agent's: each part is admitted, and they are held as one
        // section.
        let host = system
            .iter()
            .all(|part| matches!(part.from, PartFrom::Fixed | PartFrom::Read { .. }));
        if !host {
            return Err(fault(unbound(inner)));
        }
        let sources = match system.is_empty() {
            true => Vec::new(),
            false => vec![SourceRef::new(Arc::new(ViewSource::Addition(
                self.held_parts(system).await?,
            )))],
        };
        // The same session, vault and placeholders, and a lane of its own.
        Ok(Arc::new(ConversationBoundary {
            session: self.session.clone(),
            lane: Arc::new(Lane {
                owner: inner.clone(),
                parent: Some(self.lane.clone()),
                invocation: Some(invocation.clone()),
                parent_bound: std::sync::atomic::AtomicBool::new(false),
                agent: std::sync::Mutex::new(None),
            }),
            system: sources,
            definitions: tokio::sync::Mutex::new(Vec::new()),
            owned_parts: self.owned_parts.clone(),
            dispatch: std::sync::Mutex::new(Dispatch::default()),
            lanes: self.lanes.clone(),
            agent: std::sync::Mutex::new(None),
        }))
    }

    async fn observe_envelope(
        &self,
        _: FrameworkCommit,
        _: &[SourcePart],
        _: &Msg,
    ) -> Result<SourceRef, AgentError> {
        Err(fault(unbound(&self.session.inner)))
    }

    async fn summary_source(
        &self,
        _: SourceRef,
        _: &SourcePart,
        _: &SourcePart,
        _: &Msg,
    ) -> Result<SourceRef, AgentError> {
        // Caller DTOs and receipt/message equality cannot issue a stage.
        Err(fault(unbound(&self.session.inner)))
    }

    async fn staged_summary_source(
        &self,
        stage: &ActualStagedSummary,
    ) -> Result<SourceRef, AgentError> {
        self.summary_node(stage, None).await
    }

    async fn observe_retained_summary(
        &self,
        commit: &ActualRetainedCommit,
    ) -> Result<SourceRef, AgentError> {
        self.require_agent(commit.owner())?;
        let stage = commit
            .summary()
            .ok_or_else(|| fault(SessionStop::Unbound))?;
        if commit.event() != RetainedEvent::Summary || commit.message() != stage.message() {
            return Err(fault(unbound(&self.session.inner)));
        }
        self.summary_node(stage, Some(commit.clone())).await
    }

    async fn commit_model(&self, _: SourceRef, _: &Msg) -> Result<SourceRef, AgentError> {
        Err(fault(unbound(&self.session.inner)))
    }
    async fn commit_reviewed_model(
        &self,
        _: SourceRef,
        _: &Msg,
        _: &[ReplyEdit],
    ) -> Result<SourceRef, AgentError> {
        Err(fault(unbound(&self.session.inner)))
    }
    async fn commit_retained_model(
        &self,
        commit: &ActualRetainedCommit,
    ) -> Result<SourceRef, AgentError> {
        self.require_agent(commit.owner())
            .map_err(|_| fault(unbound(&self.session.inner)))?;
        if commit.event() != RetainedEvent::ModelReply {
            return Err(fault(SessionStop::Unbound));
        }
        let actual = commit
            .received()
            .ok_or_else(|| fault(SessionStop::Unbound))?;
        self.require_agent(actual.owner())?;
        let receipt = actual.transport_receipt().clone();
        let committed = commit.message();
        let edits = commit.edits();
        let inner = &self.session.inner;
        inner.live().map_err(fault)?;
        // The receipt is this conversation's own. What was committed is what
        // was received with the recorded changes applied in order, or that
        // reply with its calls removed because the round ended without
        // running them. The changes are the agent's own record of what a
        // tracked review did; nothing is worked out from a comparison.
        // The request it answered was composed for this agent's lane.
        let Some(received) = receipt
            .downcast::<ReceivedModel>()
            .filter(|received| Arc::ptr_eq(&received.owner, inner))
            .filter(|received| Arc::ptr_eq(&received.lane, &self.lane))
        else {
            return Err(fault(unbound(inner)));
        };
        if received.message != *actual.message() {
            return Err(fault(unbound(inner)));
        }
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
                commit: commit.clone(),
                lane: self.lane.clone(),
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
        _: ObservedToolResult<'_>,
    ) -> Result<SourceRef, AgentError> {
        Err(fault(SessionStop::Unbound))
    }
    async fn observe_retained_tool(&self, fact: &ActualToolFact) -> Result<SourceRef, AgentError> {
        self.require_agent(fact.owner())?;
        let commit = fact.commit().ok_or_else(|| fault(SessionStop::Unbound))?;
        self.require_agent(commit.owner())?;
        let Msg::Tool {
            tool_call_id,
            content,
            ..
        } = commit.message()
        else {
            return Err(fault(SessionStop::Unbound));
        };
        if commit.event() != RetainedEvent::ToolResult {
            return Err(fault(SessionStop::Unbound));
        }
        let mut operations = Vec::new();
        for terminal in fact.terminals() {
            self.require_agent(terminal.invocation().owner())?;
            let operation = terminal
                .operation_source()
                .and_then(|source| source.downcast::<ApprovedOperation>())
                .ok_or_else(|| fault(SessionStop::Unbound))?;
            if !operation.invocation.same_invocation(terminal.invocation())
                || operation.call != *terminal.invocation().call()
            {
                return Err(fault(SessionStop::Unbound));
            }
            operations.push(operation);
        }
        let actual = Arc::new(ActualToolResult {
            owner: self.session.inner.clone(),
            lane: self.lane.clone(),
            call_id: tool_call_id.clone(),
            tool: fact.proposed_call().name.clone(),
            operation: operations.first().cloned(),
            reached: fact
                .terminals()
                .iter()
                .map(|terminal| terminal.reached().clone())
                .collect(),
            returned: fact.returned().cloned(),
            appended: fact.appended().to_vec(),
        });
        // This label records that a checked actual terminal also returned this
        // value. It grants no source/restore authority; the immutable invocation,
        // outcome and retained message above supply the event binding.
        let observed_tool_value = fact.terminals().iter().any(|terminal| {
            matches!(&terminal.reached().result, Ok(outcome) if outcome.content == *content
                && Some(outcome.is_error) == fact.returned().map(|returned| returned.is_error))
        });
        let origin = if observed_tool_value {
            RawOrigin::ToolResult
        } else {
            RawOrigin::ChainResult
        };
        let view = if fact.parts().is_empty() {
            ResultView::Whole(self.admit(origin, content).await?)
        } else {
            let mut parts = self.held_parts(fact.parts()).await?;
            parts.extend(self.held_parts(fact.appended()).await?);
            ResultView::Parts(parts)
        };
        let mut dispatch = self.dispatch.lock().unwrap();
        if dispatch.in_flight.as_ref().is_some_and(|pending| {
            operations
                .iter()
                .any(|operation| pending.invocation.same_invocation(&operation.invocation))
        }) {
            dispatch.in_flight = None;
        }
        Ok(SourceRef::new(Arc::new(ViewSource::Result {
            view,
            actual,
            commit: commit.clone(),
        })))
    }

    async fn observe_framework_commit(
        &self,
        _: FrameworkCommit,
        _: &Msg,
    ) -> Result<SourceRef, AgentError> {
        Err(fault(unbound(&self.session.inner)))
    }
    async fn observe_retained_framework(
        &self,
        commit: &ActualRetainedCommit,
    ) -> Result<SourceRef, AgentError> {
        self.require_agent(commit.owner())?;
        let RetainedEvent::Framework(event) = commit.event() else {
            return Err(fault(SessionStop::Unbound));
        };
        if let Some(parts) = commit.parts() {
            let joined = craftsman_harness::provenance::joined(parts);
            if !matches!(
                event,
                FrameworkCommit::Notification | FrameworkCommit::InboxMessage
            ) || !matches!(commit.message(), Msg::User { content } if *content == joined)
            {
                return Err(fault(unbound(&self.session.inner)));
            }
            return Ok(SourceRef::new(Arc::new(ViewSource::Envelope {
                parts: self.held_parts(parts).await?,
                commit: commit.clone(),
            })));
        }
        let committed = commit.message();
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
            commit: commit.clone(),
        })))
    }

    async fn before_dispatch(&self, call: &ToolCall, approved: bool) -> Result<(), AgentError> {
        self.session.check().map_err(fault)?;
        let allowed = if approved {
            self.dispatch
                .lock()
                .unwrap()
                .decided
                .as_ref()
                .is_some_and(|decided| decided.call == *call)
        } else {
            self.asked_for(call).is_some()
        };
        if !allowed {
            return Err(fault(unbound(&self.session.inner)));
        }
        Ok(())
    }
    async fn before_terminal(
        &self,
        invocation: &ActualToolInvocation,
    ) -> Result<Option<SourceRef>, AgentError> {
        self.require_agent(invocation.owner())?;
        let inner = &self.session.inner;
        inner.live().map_err(fault)?;
        let call = invocation.call();
        let (model, edited) = if invocation.resumed() {
            let decided = self.dispatch.lock().unwrap().decided.take();
            let model = self.dispatch.lock().unwrap().model.clone();
            match (decided, model) {
                (Some(decided), Some(model))
                    if decided.call == *call
                        && invocation.decision_action() == Some(&decided.action)
                        && invocation
                            .decision_source()
                            .is_some_and(|source| source.is(&decided.source)) =>
                {
                    (
                        model,
                        matches!(decided.action, HitlAction::ApproveEdited { .. }),
                    )
                }
                _ => return Err(fault(unbound(inner))),
            }
        } else {
            (
                self.asked_for(call).ok_or_else(|| fault(unbound(inner)))?,
                false,
            )
        };
        let fields = if edited {
            Some(Vec::new())
        } else {
            inner.argument_fields(&call.name, &call.args).await
        };
        inner.live().map_err(fault)?;
        let operation = Arc::new(ApprovedOperation {
            owner: inner.clone(),
            invocation: invocation.clone(),
            model,
            call: call.clone(),
            edited,
            fields,
        });
        self.dispatch.lock().unwrap().in_flight = Some(operation.clone());
        Ok(Some(SourceRef::new(operation)))
    }
    fn terminal_live(&self, invocation: &ActualToolInvocation) -> Result<(), AgentError> {
        self.require_agent(invocation.owner())?;
        self.session.check().map_err(fault)
    }

    async fn accept_decision(
        &self,
        _: &ToolCall,
        _: &HitlAction,
        _: &SourceRef,
    ) -> Result<(), AgentError> {
        Err(fault(SessionStop::Unbound))
    }
    async fn accept_parked_decision(
        &self,
        parked: &craftsman_harness::provenance::ActualParkedCall,
        action: &HitlAction,
        decision: &SourceRef,
    ) -> Result<(), AgentError> {
        self.require_agent(parked.owner())?;
        let call = parked.call();
        let inner = &self.session.inner;
        inner.live().map_err(fault)?;
        // A decision that is not this conversation's, or is about another
        // call or action, is turned away. Nothing has happened yet, so the
        // call stays parked and the conversation continues.
        let received = decision.downcast::<crate::privacy_entry::HumanDecisionInput>();
        let fits = received
            .as_ref()
            .is_some_and(|input| input.binds(&self.session, parked, action));
        if !fits || self.asked_for(call).is_none() {
            return Err(AgentError::State(
                "This decision does not belong to the call that is waiting.".into(),
            ));
        }
        if let Some(raw) = received.as_ref().and_then(|input| input.edited_raw()) {
            self.session.admit_raw(raw).await.map_err(refused)?;
        }
        if let HitlAction::Deny { feedback } = action {
            self.admit(RawOrigin::HumanReply, feedback).await?;
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
        self.dispatch.lock().unwrap().decided = run.map(|call| Decided {
            call,
            action: action.clone(),
            source: decision.clone(),
        });
        if let Some(input) = received {
            input.apply_accepted_effect();
        }
        Ok(())
    }
}

/// A receipt for `message` as an agent's own transport would make it, for
/// the agent whose boundary gave `conversation`. For tests of what a
/// boundary accepts.
#[cfg(test)]
pub(super) fn receipt_for_test(conversation: &SourceRef, message: &Msg) -> SourceRef {
    let lane = conversation
        .downcast::<Lane>()
        .expect("a lane of a session");
    SourceRef::new(Arc::new(ReceivedModel {
        owner: lane.owner.clone(),
        lane,
        message: message.clone(),
        request: RequestContext {
            index: 0,
            sha256: [0; 32],
        },
        native: None,
    }))
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
