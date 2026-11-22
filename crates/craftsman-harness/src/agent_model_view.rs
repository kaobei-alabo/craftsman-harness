//! The agent's side of an optional source view boundary.
//!
//! Every function here is reached only when a boundary was selected. The
//! recorded conversation keeps its own order of commit, journal and save;
//! a source is recorded beside a message after that message is kept.
//!
//! This first selected runtime is narrow on purpose. It has no path yet for
//! middleware, a boundary policy, a completion check, system text outside the
//! boundary, steering, resumed approvals, reopening, transfer, compaction or
//! messages that arrive from other agents. Each of those is refused instead
//! of being run without sources.

use super::*;
use crate::provenance::{
    ModelDelivery, ModelViewBoundary, ObservedToolResult, SourceRef, TrackedModelRequest,
};

pub(super) struct ModelView {
    pub(super) boundary: Arc<dyn ModelViewBoundary>,
    /// One source per recorded message, in the order the messages were
    /// committed. It lives only in memory and is never read from a checkpoint.
    pub(super) sources: Mutex<Vec<SourceRef>>,
}

fn unavailable() -> AgentError {
    AgentError::State(
        "This conversation is bound to a source view. This operation is not available for it."
            .into(),
    )
}

fn unsourced() -> AgentError {
    AgentError::State(
        "A recorded message of this conversation has no source. No request was composed.".into(),
    )
}

impl Agent {
    /// Entries that carry no source cannot be used on a selected agent.
    pub(super) fn refuse_untracked_entry(&self) -> Result<(), AgentError> {
        match self.model_view {
            Some(_) => Err(unavailable()),
            None => Ok(()),
        }
    }

    /// Runs one turn on an agent that selected a model view boundary.
    ///
    /// `user_input` is recorded exactly as [`run_turn`](Agent::run_turn)
    /// records it. `source` is the host's evidence for that input and is kept
    /// beside the recorded message; it never replaces it. Refused before any
    /// effect when no boundary is selected, when the agent has a part with no
    /// source-preserving path, or when a recorded message has no source.
    pub async fn run_turn_with_view(
        &self,
        user_input: impl Into<String>,
        source: SourceRef,
    ) -> Result<TurnResult, AgentError> {
        let Some(view) = &self.model_view else {
            return Err(AgentError::State(
                "No source view boundary is selected for this agent.".into(),
            ));
        };
        if !self.chain.middlewares().is_empty()
            || self.boundary_policy.is_some()
            || self.completion_check.is_some()
            || !self.system_prompt.is_empty()
        {
            return Err(unavailable());
        }
        if view.sources.lock().unwrap().len() != self.state.lock().unwrap().messages.len() {
            return Err(unsourced());
        }
        let _turn_activity = self.begin_turn()?;
        let user_input = user_input.into();
        self.in_turn_scope(self.run_turn_inner(user_input, Some(source)))
            .await
    }

    /// Composes the request from sources and hands it to the provider's view
    /// path. `None` means the turn was cancelled while the provider ran.
    pub(super) async fn deliver_with_view(
        &self,
        view: &ModelView,
        working: &AgentState,
    ) -> Result<Option<ModelDelivery>, AgentError> {
        if !working.system_extras.is_empty() {
            return Err(unavailable());
        }
        let messages = view.sources.lock().unwrap().clone();
        if messages.len() != working.messages.len() {
            return Err(unsourced());
        }
        let definitions = self.tool_defs();
        let mut tools = Vec::with_capacity(definitions.len());
        for definition in &definitions {
            tools.push(view.boundary.tool_source(definition).await?);
        }
        let request = TrackedModelRequest {
            system: view.boundary.system_sources().await?,
            request: ModelRequest {
                system: String::new(),
                messages: working.messages.clone(),
                tools: definitions,
            },
            messages,
            tools,
        };
        if self.interrupt_model_on_cancel {
            tokio::select! {
                biased;
                _ = self.model_cancelled() => Ok(None),
                delivery = self.model.complete_with_view(request) => delivery.map(Some),
            }
        } else {
            self.model.complete_with_view(request).await.map(Some)
        }
    }

    /// Records the source of a reply that is already committed and saved.
    pub(super) async fn record_model_source(
        &self,
        view: &ModelView,
        receipt: SourceRef,
        committed: &Msg,
        index: usize,
    ) -> Result<(), AgentError> {
        if view.sources.lock().unwrap().len() != index {
            return Err(unsourced());
        }
        let source = view.boundary.commit_model(receipt, committed).await?;
        let mut sources = view.sources.lock().unwrap();
        if sources.len() != index {
            return Err(unsourced());
        }
        sources.push(source);
        Ok(())
    }

    /// Records the source of a tool result that is already committed and
    /// saved. An error leaves the result recorded and stops the turn.
    pub(super) async fn record_tool_source(
        &self,
        view: &ModelView,
        tool: &str,
        committed: &Msg,
    ) -> Result<(), AgentError> {
        let Msg::Tool {
            tool_call_id,
            content,
            is_error,
        } = committed
        else {
            return Err(unsourced());
        };
        let expected = self.state.lock().unwrap().messages.len();
        if view.sources.lock().unwrap().len() + 1 != expected {
            return Err(unsourced());
        }
        let source = view
            .boundary
            .observe_tool_result(ObservedToolResult {
                call_id: tool_call_id,
                tool,
                content,
                is_error: *is_error,
            })
            .await?;
        let mut sources = view.sources.lock().unwrap();
        if sources.len() + 1 != expected {
            return Err(unsourced());
        }
        sources.push(source);
        Ok(())
    }
}
