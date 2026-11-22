//! Private provider for a conversation whose requests are composed from
//! views of sources. Nothing selects it: no session builder, frontend or
//! configuration constructs one.
//!
//! It is the existing Chat client and the existing request encoding. What
//! differs is the input: the request it encodes is the conversation's model
//! view, not the recorded text. The client is bound to the conversation's own
//! send guard here, so the final bytes of every attempt still pass that guard
//! exactly as an ordinary bound client's do, with the existing retry policy.
//!
//! Only the Chat wire is supported. A client that speaks a native wire is
//! refused: replayed native items need receipts of their own, which do not
//! exist yet.

use crate::agent::{TokenSink, to_fw_message, to_llm_messages, to_openai_tools};
use crate::llm::LlmClient;
use crate::privacy_session::{PrivacySession, SessionStop};
use craftsman_harness::provenance::{ModelDelivery, SourceRef, TrackedModelRequest};
use craftsman_harness::{AgentError, ModelProvider, ModelRequest, ModelResponse, Usage};
use std::sync::{Arc, Mutex};

pub struct ViewChatProvider {
    client: LlmClient,
    session: Arc<PrivacySession>,
    usage: Arc<Mutex<(u64, u64)>>,
    sink: TokenSink,
}

impl ViewChatProvider {
    /// `client` is an unbound Chat client. It is bound to this conversation's
    /// guard here and nowhere else.
    pub fn new(
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
        Ok(Self {
            client,
            session,
            usage,
            sink,
        })
    }
}

fn fault(stop: SessionStop) -> AgentError {
    AgentError::Model(format!("Privacy stopped this conversation. {stop}"))
}

#[async_trait::async_trait]
impl ModelProvider for ViewChatProvider {
    /// The recorded text is never sent through this provider.
    async fn complete(&self, _: ModelRequest) -> Result<ModelResponse, AgentError> {
        Err(AgentError::Model(
            "This provider delivers only source-bound requests.".into(),
        ))
    }

    async fn complete_with_view(
        &self,
        req: TrackedModelRequest,
    ) -> Result<ModelDelivery, AgentError> {
        let view = self.session.model_view(&req).map_err(fault)?;
        let messages = to_llm_messages(&view.system, &view.messages);
        let tools = to_openai_tools(&view.tools);
        let turn = self
            .client
            .stream_turn(&messages, &tools, |text| (self.sink)(text), |_| {})
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
        // The receipt is made from the reply as it was received, before the
        // agent reviews or commits it.
        let receipt = self.session.received_model(&message).map_err(fault)?;
        Ok(ModelDelivery::new(
            ModelResponse { message, usage },
            SourceRef::new(Arc::new(receipt)),
        ))
    }
}
