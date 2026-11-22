//! The agent's side of an optional source view boundary.
//!
//! Every function here does something only when a boundary was selected. The
//! recorded conversation keeps its own order of commit, journal and save;
//! a source is recorded beside a message after that message is kept.
//!
//! The agent keeps one slot per recorded message. A slot holds the source
//! the host supplied, or, for a message the agent appended itself, the event
//! that produced it until the boundary has been asked for its source. An
//! event is noted where the message is appended; nothing is worked out
//! afterwards from a role, a position or the text.
//!
//! What a selected agent still refuses, because it has no source-preserving
//! path: a middleware whose author made no statement for this boundary, a
//! boundary policy, a completion check, system text outside the boundary,
//! per-round system additions, an inbox, the untyped turn, steering and
//! resume entries, reopening, transfer and idle compaction.

use super::*;
use crate::middleware::BoxedViewFuture;
use crate::provenance::{
    FrameworkCommit, ModelDelivery, ModelViewBoundary, ObservedToolResult, ReachedTool, ReplyEdit,
    SourceRef, TrackedModelRequest, TrackedReply, ViewSupport,
};

/// Everything the agent records or carries between rounds. A middleware of a
/// selected agent stated that it leaves all of it alone.
pub(super) struct Kept {
    messages: Vec<Msg>,
    todos: Vec<crate::state::Todo>,
    files: crate::state::FileMap,
    custom: serde_json::Map<String, serde_json::Value>,
    system_extras: Vec<String>,
}

impl Kept {
    fn of(state: &AgentState) -> Self {
        Self {
            messages: state.messages.clone(),
            todos: state.todos.clone(),
            files: state.files.clone(),
            custom: state.custom.clone(),
            system_extras: state.system_extras.clone(),
        }
    }

    fn is(&self, state: &AgentState) -> bool {
        self.messages == state.messages
            && self.todos == state.todos
            && self.files == state.files
            && self.custom == state.custom
            && self.system_extras == state.system_extras
    }
}

/// What the agent watches during one dispatch on a selected agent: every run
/// of the tool itself, and whether anything else changed the recorded state.
/// Only the tool may change it.
pub(super) struct DispatchWatch {
    reached: Vec<ReachedTool>,
    /// The state as the agent, and then each run of the tool, left it.
    last: Kept,
    changed: bool,
}

impl DispatchWatch {
    pub(super) fn new(state: &AgentState) -> Self {
        Self {
            reached: Vec::new(),
            last: Kept::of(state),
            changed: false,
        }
    }

    /// The tool is about to run.
    pub(super) fn entering(&mut self, state: &AgentState) {
        self.changed |= !self.last.is(state);
    }

    /// The tool has run: its exact call and outcome, before any middleware
    /// sees the outcome, and the state as it left it.
    pub(super) fn left(
        &mut self,
        call: ToolCall,
        result: &Result<ToolOutcome, AgentError>,
        state: &AgentState,
    ) {
        self.reached.push(ReachedTool::new(call, result));
        self.last = Kept::of(state);
    }

    /// The dispatch chain has returned.
    pub(super) fn finish(&mut self, state: &AgentState) -> (Vec<ReachedTool>, bool) {
        self.changed |= !self.last.is(state);
        (std::mem::take(&mut self.reached), self.changed)
    }
}

pub(super) fn state_changed() -> AgentError {
    AgentError::State(
        "A middleware of this conversation changed recorded state. Nothing further was sent."
            .into(),
    )
}

pub(super) enum Slot {
    Bound(SourceRef),
    /// Appended by the agent itself; the boundary has not been asked yet.
    Pending(FrameworkCommit),
}

/// One queued input of the active turn. `source` is set only by the typed
/// steering entry of an agent with a source view.
pub(super) struct QueuedInput {
    pub(super) text: String,
    pub(super) source: Option<SourceRef>,
}

impl QueuedInput {
    pub(super) fn plain(text: String) -> Self {
        Self { text, source: None }
    }
}

pub(super) struct ModelView {
    pub(super) boundary: Arc<dyn ModelViewBoundary>,
    /// One slot per recorded message, in the order the messages were
    /// committed. It lives only in memory and is never read from a checkpoint.
    pub(super) sources: Mutex<Vec<Slot>>,
}

impl ModelView {
    pub(super) fn new(boundary: Arc<dyn ModelViewBoundary>) -> Self {
        Self {
            boundary,
            sources: Mutex::new(Vec::new()),
        }
    }

    /// Slots for inputs that were just appended, in the same order. An input
    /// that came without a source is text from another agent.
    pub(super) fn note_inputs(&self, sources: Vec<Option<SourceRef>>) {
        self.sources
            .lock()
            .unwrap()
            .extend(sources.into_iter().map(|source| match source {
                Some(source) => Slot::Bound(source),
                None => Slot::Pending(FrameworkCommit::Notification),
            }));
    }

    /// The durable reconciliation of [`reconcile_recovered_calls`], keeping
    /// the slots in step: wherever it inserts a result, a pending slot is
    /// inserted at the same position.
    pub(super) fn reconcile(&self, state: &mut AgentState) {
        let mut slots = self.sources.lock().unwrap();
        if slots.len() != state.messages.len() {
            // Already out of step, so the next request is refused. The
            // record is still reconciled as it always is.
            reconcile_recovered_calls(state, None);
            return;
        }
        let unrecorded = |slots: &mut Vec<Slot>, added: usize| {
            slots.extend((0..added).map(|_| Slot::Pending(FrameworkCommit::UnrecordedCall)));
        };
        let mut pending: Vec<String> = Vec::new();
        let original = std::mem::take(&mut state.messages);
        let kept = std::mem::take(&mut *slots);
        for (message, slot) in original.into_iter().zip(kept) {
            match &message {
                Msg::Tool { tool_call_id, .. } => {
                    if let Some(index) = pending.iter().position(|id| id == tool_call_id) {
                        pending.remove(index);
                    }
                }
                _ => {
                    let added = append_unknown_results(&mut state.messages, &mut pending, None);
                    unrecorded(&mut slots, added);
                }
            }
            if let Msg::Assistant { tool_calls, .. } = &message {
                pending.extend(tool_calls.iter().map(|call| call.id.clone()));
            }
            state.messages.push(message);
            slots.push(slot);
        }
        let added = append_unknown_results(&mut state.messages, &mut pending, None);
        unrecorded(&mut slots, added);
    }
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

    /// The selected boundary, when every part of this agent has a
    /// source-preserving path. Checked before any effect of a typed entry.
    fn supported_view(&self) -> Result<&Arc<ModelView>, AgentError> {
        let Some(view) = &self.model_view else {
            return Err(AgentError::State(
                "No source view boundary is selected for this agent.".into(),
            ));
        };
        let adapted = self.chain.middlewares().iter().all(|entry| {
            matches!(
                entry.view_support(),
                ViewSupport::Preserving | ViewSupport::TrackedReply
            )
        });
        if !adapted
            || self.boundary_policy.is_some()
            || self.completion_check.is_some()
            || !self.system_prompt.is_empty()
            || self.inbox.lock().unwrap().is_some()
        {
            return Err(unavailable());
        }
        Ok(view)
    }

    /// Runs one turn on an agent that selected a model view boundary.
    ///
    /// `user_input` is recorded exactly as [`run_turn`](Agent::run_turn)
    /// records it. `source` is the host's evidence for that input and is kept
    /// beside the recorded message; it never replaces it. Refused before any
    /// effect when no boundary is selected, when the agent has a part with no
    /// source-preserving path, or when a recorded message has no slot.
    pub async fn run_turn_with_view(
        &self,
        user_input: impl Into<String>,
        source: SourceRef,
    ) -> Result<TurnResult, AgentError> {
        let view = self.supported_view()?;
        if view.sources.lock().unwrap().len() != self.state.lock().unwrap().messages.len() {
            return Err(unsourced());
        }
        let _turn_activity = self.begin_turn()?;
        let user_input = user_input.into();
        let result = self
            .in_turn_scope(self.run_turn_inner(user_input, Some(source)))
            .await;
        self.settled(view, result).await
    }

    /// The source kept beside each recorded message, in order, for the host
    /// that selected the boundary. `None` when no boundary is selected. An
    /// entry is `None` while the boundary has not been asked about a message
    /// the agent appended itself.
    pub fn recorded_sources(&self) -> Option<Vec<Option<SourceRef>>> {
        let view = self.model_view.as_ref()?;
        Some(
            view.sources
                .lock()
                .unwrap()
                .iter()
                .map(|slot| match slot {
                    Slot::Bound(source) => Some(source.clone()),
                    Slot::Pending(_) => None,
                })
                .collect(),
        )
    }

    /// Adds typed input to the active turn of a selected agent, like
    /// [`steer`](Agent::steer). The text is recorded at the next safe round
    /// boundary and `source` is kept beside it. Returns `false` when no turn
    /// is active, when the turn no longer takes input, or when no boundary is
    /// selected.
    pub fn steer_with_view(&self, message: impl Into<String>, source: SourceRef) -> bool {
        let message = message.into();
        if message.trim().is_empty() || self.model_view.is_none() {
            return false;
        }
        let mut input = self.turn_input.lock().unwrap();
        if !input.active || input.idle_operation {
            return false;
        }
        input.pending.push_back(QueuedInput {
            text: message,
            source: Some(source),
        });
        true
    }

    /// Applies a human decision to the parked call of a selected agent, like
    /// [`resume`](Agent::resume). `decision` is the host's evidence of that
    /// decision. The boundary checks it against the parked call and the
    /// action before anything is applied; a refusal leaves the call parked.
    /// An approved call is checked once more, exactly as it will run, before
    /// the tool is reached.
    pub async fn resume_with_view(
        &self,
        action: crate::interrupt::HitlAction,
        deny_default: &str,
        decision: SourceRef,
    ) -> Result<TurnResult, AgentError> {
        let view = self.supported_view()?;
        let _turn_activity = self.begin_turn()?;
        let call = self
            .pending
            .lock()
            .unwrap()
            .as_ref()
            .map(|pending| pending.call.clone())
            .ok_or_else(|| AgentError::State("no suspension to resume".into()))?;
        view.boundary
            .accept_decision(&call, &action, &decision)
            .await?;
        let result = self
            .in_turn_scope(self.resume_inner(action, deny_default))
            .await;
        self.settled(view, result).await
    }

    /// Asks the boundary for the source of every message the agent appended
    /// itself and has not asked about yet, in order.
    async fn settle_sources(&self, view: &ModelView) -> Result<(), AgentError> {
        loop {
            let next =
                view.sources.lock().unwrap().iter().enumerate().find_map(
                    |(index, slot)| match slot {
                        Slot::Pending(event) => Some((index, *event)),
                        Slot::Bound(_) => None,
                    },
                );
            let Some((index, event)) = next else {
                return Ok(());
            };
            let message = self
                .state
                .lock()
                .unwrap()
                .messages
                .get(index)
                .cloned()
                .ok_or_else(unsourced)?;
            let source = view
                .boundary
                .observe_framework_commit(event, &message)
                .await?;
            match view.sources.lock().unwrap().get_mut(index) {
                Some(slot @ Slot::Pending(_)) => *slot = Slot::Bound(source),
                _ => return Err(unsourced()),
            }
        }
    }

    /// The end of a typed entry. Whatever the turn appended itself is already
    /// recorded and saved; this asks for its sources. The turn's own error
    /// comes first.
    async fn settled(
        &self,
        view: &ModelView,
        result: Result<TurnResult, AgentError>,
    ) -> Result<TurnResult, AgentError> {
        let settled = self.settle_sources(view).await;
        let result = result?;
        settled?;
        Ok(result)
    }

    /// Notes `count` messages the agent has just appended itself.
    pub(super) fn note_framework_commits(&self, event: FrameworkCommit, count: usize) {
        if let Some(view) = &self.model_view {
            view.sources
                .lock()
                .unwrap()
                .extend((0..count).map(|_| Slot::Pending(event)));
        }
    }

    /// The recorded state before a phase of middleware hooks, on a selected
    /// agent.
    pub(super) fn view_phase_start(&self, working: &AgentState) -> Option<Kept> {
        self.model_view.as_ref().map(|_| Kept::of(working))
    }

    /// Refuses the turn when a hook of the phase changed recorded state.
    /// Every middleware of a selected agent stated that it does not.
    pub(super) fn view_phase_end(
        &self,
        kept: Option<Kept>,
        working: &AgentState,
    ) -> Result<(), AgentError> {
        match kept {
            Some(kept) if !kept.is(working) => Err(state_changed()),
            _ => Ok(()),
        }
    }

    /// The review of a received reply on a selected agent, in the order and
    /// with the verdict rule of the ordinary review. A middleware with a
    /// tracked review changes the reply only through recorded operations and
    /// sees nothing else. Every other middleware must leave the state alone.
    pub(super) async fn review_with_view(
        &self,
        working: &mut AgentState,
        reply: usize,
        edits: &mut Vec<ReplyEdit>,
    ) -> Result<Flow, AgentError> {
        let mut decision = Flow::Proceed;
        for entry in self.chain.middlewares().iter().rev() {
            let verdict = if entry.view_support() == ViewSupport::TrackedReply {
                let message = working.messages.get_mut(reply).ok_or_else(unsourced)?;
                let mut tracked = TrackedReply {
                    message,
                    edits,
                    by: entry.name(),
                };
                entry.review_model_with_view(&mut tracked).await?
            } else {
                let kept = Kept::of(working);
                let verdict = entry.review_model(working).await?;
                if !kept.is(working) {
                    return Err(state_changed());
                }
                verdict
            };
            if decision == Flow::Proceed {
                decision = verdict;
            }
        }
        Ok(decision)
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
        self.settle_sources(view).await?;
        let messages = view
            .sources
            .lock()
            .unwrap()
            .iter()
            .map(|slot| match slot {
                Slot::Bound(source) => Some(source.clone()),
                Slot::Pending(_) => None,
            })
            .collect::<Option<Vec<_>>>()
            .ok_or_else(unsourced)?;
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
        let model = self.model.clone();
        let terminal = move |request: TrackedModelRequest| -> BoxedViewFuture<'_> {
            let model = model.clone();
            Box::pin(async move { model.complete_with_view(request).await })
        };
        if self.interrupt_model_on_cancel {
            tokio::select! {
                biased;
                _ = self.model_cancelled() => Ok(None),
                delivery = self.chain.around_model_with_view(request, &terminal) => delivery.map(Some),
            }
        } else {
            self.chain
                .around_model_with_view(request, &terminal)
                .await
                .map(Some)
        }
    }

    /// Records the source of a reply that is already committed and saved.
    pub(super) async fn record_model_source(
        &self,
        view: &ModelView,
        receipt: SourceRef,
        committed: &Msg,
        index: usize,
        edits: &[ReplyEdit],
    ) -> Result<(), AgentError> {
        if view.sources.lock().unwrap().len() != index {
            return Err(unsourced());
        }
        let source = view
            .boundary
            .commit_reviewed_model(receipt, committed, edits)
            .await?;
        let mut sources = view.sources.lock().unwrap();
        if sources.len() != index {
            return Err(unsourced());
        }
        sources.push(Slot::Bound(source));
        Ok(())
    }

    /// Records the source of a tool result that is already committed and
    /// saved. An error leaves the result recorded and stops the turn.
    pub(super) async fn record_tool_source(
        &self,
        view: &ModelView,
        tool: &str,
        committed: &Msg,
        reached: &[ReachedTool],
        returned: Option<&ToolOutcome>,
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
                reached,
                returned,
            })
            .await?;
        let mut sources = view.sources.lock().unwrap();
        if sources.len() + 1 != expected {
            return Err(unsourced());
        }
        sources.push(Slot::Bound(source));
        Ok(())
    }
}
