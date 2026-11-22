//! The conversation's side of the framework's source view boundary.
//!
//! The framework keeps one opaque source beside each recorded message and
//! hands them back when a request is composed. This module is what those
//! sources are for one privacy conversation, and how a request made of
//! recorded text and sources becomes a request made of views.
//!
//! A source is one of three things, each minted at an actual event:
//!
//! - an admitted node, for raw text that went through admission;
//! - a committed model node, for a reply that this conversation's own
//!   transport actually received and the agent then committed;
//! - a tool definition, whose description and schema text were admitted.
//!
//! A committed model node is model-origin text. It is not an admitted node
//! and gives no right to restore anything. Nothing here decides an origin
//! from a role, a name, a position or equal bytes: a source is used only if
//! it is this conversation's own object and still fits the recorded item it
//! sits beside.

use super::*;
use craftsman_harness::provenance::{
    ModelViewBoundary, ObservedToolResult, SourceRef, TrackedModelRequest,
};
use craftsman_harness::{AgentError, ModelRequest, Msg, ToolDef};

/// What this conversation keeps beside a recorded item.
enum ViewSource {
    Admitted(AdmittedNode),
    Model(CommittedModelNode),
    Definition {
        description: AdmittedNode,
        parameters: AdmittedNode,
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

impl AdmittedNode {
    fn captured_text_is(&self, text: &str) -> bool {
        self.inner.raw.inner.text.as_str() == text
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
            ViewSource::Admitted(node) if node.admitted_by(self) => Some(node.clone()),
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
                        ViewSource::Admitted(node),
                    ) if node.origin() == RawOrigin::ToolResult => Msg::Tool {
                        tool_call_id: tool_call_id.clone(),
                        content: view(node, content)?,
                        is_error: *is_error,
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
        }))
    }
}

impl AdmittedNode {
    fn is_accepted_fact(&self) -> bool {
        self.inner.raw.is_accepted_fact()
    }
}

/// The framework's boundary for one privacy conversation.
pub struct ConversationBoundary {
    session: Arc<PrivacySession>,
    system: Vec<SourceRef>,
    /// The admitted text of each tool definition seen so far, by name. An
    /// entry is reused only while the definition's text is the captured text.
    definitions: tokio::sync::Mutex<Vec<(String, SourceRef)>>,
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
        let admit = |text: &str| {
            let raw = CapturedRaw::from_producer(RawOrigin::HostStatic, text);
            async move { self.session.admit_raw(&raw).await }
        };
        let stopped = |error: AdmitError| match error {
            AdmitError::Stopped(stop) => fault(stop),
            AdmitError::Refused(refusal) => fault(SessionStop::Projection(refusal)),
        };
        let description = admit(&definition.description).await.map_err(stopped)?;
        let parameters = admit(&schema).await.map_err(stopped)?;
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
        // what was received.
        let received = receipt
            .downcast::<ReceivedModel>()
            .filter(|received| {
                Arc::ptr_eq(&received.owner, inner) && received.message == *committed
            })
            .ok_or_else(|| fault(unbound(inner)))?;
        Ok(SourceRef::new(Arc::new(ViewSource::Model(
            CommittedModelNode {
                inner: Arc::new(ModelInner {
                    owner: inner.clone(),
                    message: received.message.clone(),
                }),
            },
        ))))
    }

    async fn observe_tool_result(
        &self,
        result: ObservedToolResult<'_>,
    ) -> Result<SourceRef, AgentError> {
        // The returned content is an accepted fact: it is admitted or the
        // session stops. It is never replaced.
        let raw = CapturedRaw::from_producer(RawOrigin::ToolResult, result.content);
        let node = self
            .session
            .admit_raw(&raw)
            .await
            .map_err(|error| match error {
                AdmitError::Stopped(stop) => fault(stop),
                AdmitError::Refused(refusal) => fault(SessionStop::Projection(refusal)),
            })?;
        self.session.source(&node).map_err(fault)
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
