use crate::error::AgentError;
use crate::state::{Msg, ToolCall};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    /// JSON Schema for the arguments object.
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct ModelRequest {
    pub system: String,
    pub messages: Vec<Msg>,
    pub tools: Vec<ToolDef>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Debug, Clone)]
pub struct ModelResponse {
    pub message: Msg,
    pub usage: Usage,
}

impl ModelResponse {
    pub fn tool_calls(&self) -> &[ToolCall] {
        match &self.message {
            Msg::Assistant { tool_calls, .. } => tool_calls,
            _ => &[],
        }
    }
}

/// Implemented by the application (e.g. over an OpenAI-compatible SSE client).
/// The framework never talks HTTP itself.
#[async_trait]
pub trait ModelProvider: Send + Sync {
    async fn complete(&self, req: ModelRequest) -> Result<ModelResponse, AgentError>;

    /// Delivers a request composed from sources, for an agent that selected a
    /// model view boundary. An agent without one never calls this. A provider
    /// that does not implement it refuses: the request is not passed to
    /// `complete`, because that would send the recorded text instead of views.
    ///
    /// The default is written without `async`, in the shape `async_trait`
    /// gives an `async fn`, so that the many providers that do not override
    /// it share one ready future instead of each getting a state machine.
    fn complete_with_view<'a, 'f>(
        &'a self,
        req: crate::provenance::TrackedModelRequest,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<crate::provenance::ModelDelivery, AgentError>>
                + Send
                + 'f,
        >,
    >
    where
        'a: 'f,
        Self: 'f,
    {
        drop(req);
        Box::pin(std::future::ready(Err(AgentError::Model(
            "This provider cannot deliver a source-bound request.".into(),
        ))))
    }
}

/// Estimates token counts for a slice of messages. Implemented by the
/// application (e.g. over `tiktoken-rs`); the framework has no tokenizer of
/// its own, just as it has no HTTP client of its own for
/// [`ModelProvider`]. An estimate, not an exact count — see
/// `CompactionMiddleware`, its only consumer.
pub trait TokenCounter: Send + Sync {
    fn count(&self, messages: &[Msg]) -> u64;
}
