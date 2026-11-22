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
    FrameworkCommit, ModelViewBoundary, ObservedToolResult, ReachedTool, SourceRef,
    TrackedModelRequest,
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
        node: AdmittedNode,
        actual: Arc<ActualToolResult>,
    },
    Framework {
        node: AdmittedNode,
        event: FrameworkCommit,
    },
}

/// A reply as this conversation's own transport actually received it. Only
/// that transport creates one, at the moment of receipt.
pub struct ReceivedModel {
    owner: Arc<Inner>,
    message: Msg,
}

struct ModelInner {
    owner: Arc<Inner>,
    message: Msg,
    fields: Option<Vec<SourceField>>,
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
    call_id: String,
    tool: String,
    operation: Option<Arc<ApprovedOperation>>,
    reached: Vec<ReachedTool>,
    returned: Option<ToolOutcome>,
}

impl ActualToolResult {
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
    fn captured_text_is(&self, text: &str) -> bool {
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

    /// Called by this conversation's own transport with the reply it has just
    /// received, before anything else happens to that reply.
    pub fn received_model(&self, message: &Msg) -> Result<ReceivedModel, SessionStop> {
        self.inner.live()?;
        Ok(ReceivedModel {
            owner: self.inner.clone(),
            message: message.clone(),
        })
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
            | ViewSource::Result { node, .. }
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
            ViewSource::Result { node, actual } if node.admitted_by(self) => Some(actual.clone()),
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
            for source in request.system_sources() {
                let ViewSource::Admitted(node) = &*own(source)? else {
                    return Err(SessionStop::Unbound);
                };
                if !node.admitted_by(self) || !node.is_accepted_fact() {
                    return Err(SessionStop::Unbound);
                }
                if !system.is_empty() {
                    system.push_str("\n\n");
                }
                system.push_str(node.model_view()?);
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
                        ViewSource::Result { node, actual },
                    ) if actual.call_id == *tool_call_id => Msg::Tool {
                        tool_call_id: tool_call_id.clone(),
                        content: view(node, content)?,
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
    /// captured and admitted before the first request.
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
        Ok(Arc::new(ConversationBoundary {
            session: self.clone(),
            system: sources,
            definitions: tokio::sync::Mutex::new(Vec::new()),
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

    async fn commit_model(
        &self,
        receipt: SourceRef,
        committed: &Msg,
    ) -> Result<SourceRef, AgentError> {
        let inner = &self.session.inner;
        inner.live().map_err(fault)?;
        // The receipt is this conversation's own, and what was committed is
        // what was received, or that reply with its calls removed because the
        // round ended without running them.
        let fits = receipt.downcast::<ReceivedModel>().is_some_and(|received| {
            Arc::ptr_eq(&received.owner, inner)
                && (received.message == *committed
                    || without_calls(&received.message).as_ref() == Some(committed))
        });
        if !fits {
            return Err(fault(unbound(inner)));
        }
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
        // stops. It is never replaced.
        let node = self.admit(origin, result.content).await?;
        let actual = Arc::new(ActualToolResult {
            call_id: result.call_id.to_owned(),
            tool: result.tool.to_owned(),
            operation,
            reached: result.reached.to_vec(),
            returned: result.returned.cloned(),
        });
        Ok(SourceRef::new(Arc::new(ViewSource::Result {
            node,
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
