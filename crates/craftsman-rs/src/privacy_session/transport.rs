//! The transports of a conversation whose requests are composed from views
//! of sources. The session builder constructs one only for a private
//! selection, which no frontend, configuration or CLI route can make.
//!
//! This module is the only producer of a reply's receipt. The functions that
//! make one are private to the conversation module, and they are called here
//! and nowhere else, at the moment a client returns a reply, with the digest
//! of the exact request bytes that client sent. No other code can make a
//! receipt from a message it holds.
//!
//! They are the existing clients and the existing request encodings. What
//! differs is the input: the request they encode is the conversation's model
//! view, not the recorded text. The client is bound to the conversation's own
//! send guard here, so the final bytes of every attempt still pass that guard
//! exactly as an ordinary bound client's do, under the existing retry policy.
//!
//! [`ViewChatProvider`] speaks the Chat wire. [`ViewNativeProvider`] speaks
//! the three native wires through the existing native client: endpoint,
//! headers, limits, stream handling and usage are that client's. Provider
//! items, signed and opaque ones included, are kept exactly as received in a
//! receipt of this conversation and sent back exactly. The client's history
//! cache and its checkpoint copy are not used.
//!
//! What a client adds to every request by itself, the model id, the fixed
//! fields of its wire and the configured generation settings, is captured
//! once as host configuration and admitted. A setting that holds a supported
//! value cannot be replaced without changing what it means, so such a client
//! is refused.

use super::view::{received_chat_reply, received_native_reply};
use super::{AdmitError, AdmittedNode, CapturedRaw, PrivacySession, RawOrigin, SessionStop};
use crate::agent::{
    ActivitySink, RoundSink, TokenSink, to_fw_message, to_llm_messages, to_openai_tools,
};
use crate::llm::LlmClient;
use craftsman_harness::provenance::{ModelDelivery, SourceRef, TrackedModelRequest};
use craftsman_harness::{AgentError, ModelProvider, ModelRequest, ModelResponse, Usage};
use std::sync::{Arc, Mutex};

/// The most replies with provider items one native conversation keeps, as
/// the native history does.
const NATIVE_REPLIES: usize = 256;

fn fault(stop: SessionStop) -> AgentError {
    AgentError::Model(format!("Privacy stopped this conversation. {stop}"))
}

fn recorded_text_refused() -> AgentError {
    AgentError::Model("This provider delivers only source-bound requests.".into())
}

/// Captures what a client adds to every request and admits it. Nothing in it
/// may need a placeholder.
async fn admitted_defaults(
    session: &PrivacySession,
    defaults: &str,
) -> Result<AdmittedNode, SessionStop> {
    let raw = CapturedRaw::from_producer(RawOrigin::HostConfig, defaults);
    let node = session.admit_raw(&raw).await.map_err(|error| match error {
        AdmitError::Stopped(stop) => stop,
        AdmitError::Refused(refusal) => SessionStop::Projection(refusal),
    })?;
    if node.replacements() != 0 {
        session.stop();
        return Err(SessionStop::Unbound);
    }
    Ok(node)
}

/// The captured defaults are still what the client adds, and their source is
/// still this live conversation's.
fn defaults_hold(
    session: &PrivacySession,
    defaults: &AdmittedNode,
    current: &str,
) -> Result<(), AgentError> {
    if !defaults.admitted_by(session) || !defaults.captured_text_is(current) {
        session.stop();
        return Err(fault(SessionStop::Unbound));
    }
    defaults.model_view().map(|_| ()).map_err(fault)
}

pub struct ViewChatProvider {
    client: LlmClient,
    session: Arc<PrivacySession>,
    usage: Arc<Mutex<(u64, u64)>>,
    sink: TokenSink,
    round_sink: Option<RoundSink>,
    status_sink: Option<ActivitySink>,
    defaults: AdmittedNode,
}

impl ViewChatProvider {
    /// `client` is an unbound Chat client. It is bound to this conversation's
    /// guard here and nowhere else.
    pub async fn new(
        client: LlmClient,
        session: Arc<PrivacySession>,
        usage: Arc<Mutex<(u64, u64)>>,
        sink: TokenSink,
    ) -> Result<Self, SessionStop> {
        session.check()?;
        if client.native.is_some() {
            return Err(SessionStop::Unbound);
        }
        let client = client
            .with_send_guard(session.send_guard())
            .map_err(SessionStop::Guard)?;
        let defaults = admitted_defaults(&session, &client.chat_request_defaults()).await?;
        Ok(Self {
            client,
            session,
            usage,
            sink,
            round_sink: None,
            status_sink: None,
            defaults,
        })
    }

    /// The frontend's notifications, as the ordinary provider has them: one
    /// before each request, and the transport's retry and warning lines.
    pub fn with_sinks(
        mut self,
        round_sink: Option<RoundSink>,
        status_sink: Option<ActivitySink>,
    ) -> Self {
        self.round_sink = round_sink;
        self.status_sink = status_sink;
        self
    }
}

#[async_trait::async_trait]
impl ModelProvider for ViewChatProvider {
    /// The recorded text is never sent through this provider.
    async fn complete(&self, _: ModelRequest) -> Result<ModelResponse, AgentError> {
        Err(recorded_text_refused())
    }

    async fn complete_with_view(
        &self,
        req: TrackedModelRequest,
    ) -> Result<ModelDelivery, AgentError> {
        if let Some(round_sink) = &self.round_sink {
            round_sink();
        }
        defaults_hold(
            &self.session,
            &self.defaults,
            &self.client.chat_request_defaults(),
        )?;
        let view = self.session.chat_view(&req).map_err(fault)?;
        let messages = to_llm_messages(&view.system, &view.messages);
        let tools = to_openai_tools(&view.tools);
        let (turn, request_sha256) = self
            .client
            .stream_view_turn(
                &messages,
                &tools,
                |text| (self.sink)(text),
                |status| {
                    if let Some(sink) = &self.status_sink {
                        sink(status);
                    }
                },
            )
            .await
            .map_err(|error| AgentError::Model(error.to_string()))?;
        {
            let mut usage = self.usage.lock().unwrap();
            usage.0 += turn.usage.input_tokens;
            usage.1 += turn.usage.output_tokens;
        }
        let usage = Usage {
            input_tokens: turn.usage.input_tokens,
            output_tokens: turn.usage.output_tokens,
        };
        let message = to_fw_message(turn);
        // The receipt is made here, from the reply as it was received and
        // the request it answered, before the agent reviews or commits it.
        let receipt =
            received_chat_reply(&self.session, &message, view.messages.len(), request_sha256)
                .map_err(fault)?;
        Ok(ModelDelivery::new(
            ModelResponse { message, usage },
            SourceRef::new(Arc::new(receipt)),
        ))
    }
}

pub struct ViewNativeProvider {
    client: LlmClient,
    session: Arc<PrivacySession>,
    usage: Arc<Mutex<(u64, u64)>>,
    sink: TokenSink,
    round_sink: Option<RoundSink>,
    defaults: AdmittedNode,
    /// Whether this provider serves the summarizer of the conversation.
    summaries: bool,
}

impl ViewNativeProvider {
    /// `client` is an unbound client with a native transport. It is bound to
    /// this conversation's guard here and nowhere else.
    pub async fn new(
        client: LlmClient,
        session: Arc<PrivacySession>,
        usage: Arc<Mutex<(u64, u64)>>,
        sink: TokenSink,
    ) -> Result<Self, SessionStop> {
        session.check()?;
        if client.native.is_none() {
            return Err(SessionStop::Unbound);
        }
        let client = client
            .with_send_guard(session.send_guard())
            .map_err(SessionStop::Guard)?;
        let native = client.native.as_ref().ok_or(SessionStop::Unbound)?;
        let defaults = native
            .request_defaults()
            .map_err(|_| SessionStop::Unbound)?;
        let defaults = admitted_defaults(&session, &defaults).await?;
        Ok(Self {
            client,
            session,
            usage,
            sink,
            round_sink: None,
            defaults,
            summaries: false,
        })
    }

    /// For the summarizer of the conversation. Its requests carry earlier
    /// replies as their text and calls, never with provider items, as the
    /// ordinary summarizer's do: its own client never received those replies.
    pub fn for_summaries(mut self) -> Self {
        self.summaries = true;
        self
    }

    /// The frontend's notification before each request, as the ordinary
    /// provider has it.
    pub fn with_round_sink(mut self, round_sink: Option<RoundSink>) -> Self {
        self.round_sink = round_sink;
        self
    }
}

#[async_trait::async_trait]
impl ModelProvider for ViewNativeProvider {
    /// The recorded text is never sent through this provider.
    async fn complete(&self, _: ModelRequest) -> Result<ModelResponse, AgentError> {
        Err(recorded_text_refused())
    }

    async fn complete_with_view(
        &self,
        req: TrackedModelRequest,
    ) -> Result<ModelDelivery, AgentError> {
        if let Some(round_sink) = &self.round_sink {
            round_sink();
        }
        let native = self
            .client
            .native
            .as_ref()
            .ok_or_else(|| fault(SessionStop::Unbound))?;
        let current = native
            .request_defaults()
            .map_err(|error| AgentError::Model(error.to_string()))?;
        defaults_hold(&self.session, &self.defaults, &current)?;
        // The view, and the provider items this conversation itself received
        // from a client of exactly this identity. The summarizer sends none.
        let (view, replay) = if self.summaries {
            (self.session.model_view(&req).map_err(fault)?, None)
        } else {
            let (view, replay) = self
                .session
                .native_view(&req, native.identity())
                .map_err(fault)?;
            if replay.replies() >= NATIVE_REPLIES {
                return Err(AgentError::Model(
                    "Native model history exceeds its checkpoint limit.".into(),
                ));
            }
            (view, Some(replay))
        };
        let exchange = native
            .complete_view(
                &view,
                &|index, message| {
                    replay
                        .as_ref()
                        .and_then(|replay| replay.items(index, message))
                },
                |text| (self.sink)(text),
            )
            .await
            .map_err(|error| AgentError::Model(error.to_string()))?;
        {
            let mut usage = self.usage.lock().unwrap();
            let input = usage
                .0
                .checked_add(exchange.response.usage.input_tokens)
                .ok_or_else(|| AgentError::Model("Model input usage overflow.".into()))?;
            let output = usage
                .1
                .checked_add(exchange.response.usage.output_tokens)
                .ok_or_else(|| AgentError::Model("Model output usage overflow.".into()))?;
            *usage = (input, output);
        }
        // The receipt is made from the reply and its items as received,
        // before the agent reviews or commits the reply.
        let receipt = received_native_reply(
            &self.session,
            &exchange.response.message,
            native.identity(),
            view.messages.len(),
            exchange.request_sha256,
            exchange.items,
        )
        .map_err(fault)?;
        Ok(ModelDelivery::new(
            exchange.response,
            SourceRef::new(Arc::new(receipt)),
        ))
    }
}
