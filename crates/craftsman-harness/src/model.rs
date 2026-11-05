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
}

/// Estimates token counts for a slice of messages. Implemented by the
/// application (e.g. over `tiktoken-rs`); the framework has no tokenizer of
/// its own, just as it has no HTTP client of its own for
/// [`ModelProvider`]. An estimate, not an exact count — see
/// `CompactionMiddleware`, its only consumer.
pub trait TokenCounter: Send + Sync {
    fn count(&self, messages: &[Msg]) -> u64;
}
