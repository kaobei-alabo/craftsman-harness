//! Optional, frontend-selected completion checks. The agent owns their tool dispatch.
use crate::{AgentError, AgentState};
use async_trait::async_trait;
use serde_json::Value;

/// A completion policy selects a normal registered tool or accepts completion.
/// It never supplies tool-call identity or dispatches through a separate runner.
#[non_exhaustive]
pub enum CompletionAction {
    Ready,
    Check { tool: String, args: Value },
    Stop { reason: String },
}

#[non_exhaustive]
pub enum CompletionAfterCheck {
    Continue,
    Stop { reason: String },
}

#[async_trait]
pub trait CompletionCheck: Send + Sync {
    /// The actual configured host source for a selected view adapter. Generic
    /// erasure grants no authority; the app must validate its own producer.
    /// None keeps selected verification unsupported rather than inventing a
    /// model origin for a host-scheduled call.
    fn source_for_view(&self) -> Option<crate::provenance::SourceRef> {
        None
    }
    /// Called after the actual reviewed response has been committed. Default
    /// agents have no policy, so their original finish path remains unchanged.
    async fn before_finish(&self, state: &AgentState) -> Result<CompletionAction, AgentError>;

    /// Actual tool results are committed and durably saved before this hook.
    /// Continue samples the model normally so its answer can use those results.
    async fn after_check(&self, state: &AgentState) -> Result<CompletionAfterCheck, AgentError>;
}
