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
use crate::ModelResponse;
use crate::middleware::BoxedViewFuture;
use crate::provenance::{
    ActualAgentOwner, ActualParkedCall, ActualReceivedReply, ActualRetainedCommit,
    ActualTerminalOutcome, ActualToolFact, FrameworkCommit, ModelDelivery, ModelViewBoundary,
    ReachedTool, ReplyEdit, RetainedEvent, SourceRef, TrackedModelRequest, TrackedReply,
    ViewSupport,
};
use std::sync::atomic::AtomicBool;

/// Everything the agent records or carries between rounds. A middleware of a
/// selected agent stated that it leaves all of it alone.
pub(super) struct Kept {
    messages: Vec<Msg>,
    todos: Vec<crate::state::Todo>,
    files: crate::state::FileMap,
    custom: serde_json::Map<String, serde_json::Value>,
    system_extras: Vec<String>,
    last_usage: Option<crate::Usage>,
    last_usage_msg_count: usize,
}

impl Kept {
    fn of(state: &AgentState) -> Self {
        Self {
            messages: state.messages.clone(),
            todos: state.todos.clone(),
            files: state.files.clone(),
            custom: state.custom.clone(),
            system_extras: state.system_extras.clone(),
            last_usage: state.last_usage.clone(),
            last_usage_msg_count: state.last_usage_msg_count,
        }
    }

    fn is(&self, state: &AgentState) -> bool {
        self.messages == state.messages
            && self.todos == state.todos
            && self.files == state.files
            && self.custom == state.custom
            && self.system_extras == state.system_extras
            && self.last_usage == state.last_usage
            && self.last_usage_msg_count == state.last_usage_msg_count
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
    Bound(SourceRef, ActualRetainedCommit),
    /// Appended by the agent itself; the boundary has not been asked yet.
    Pending(ActualRetainedCommit),
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
    pub(super) owner: ActualAgentOwner,
    closed: AtomicBool,
    pub(super) decision: Mutex<Option<SourceRef>>,
    pub(super) parked: Mutex<Option<ActualParkedCall>>,
    facts: Mutex<Facts>,
}

#[derive(Default)]
struct Facts {
    received: Vec<ActualReceivedReply>,
    retained: Vec<ActualRetainedCommit>,
    terminals: Vec<ActualTerminalOutcome>,
    chains: Vec<ActualToolFact>,
}

impl ModelView {
    pub(super) fn new(boundary: Arc<dyn ModelViewBoundary>) -> Self {
        let owner = ActualAgentOwner::issued();
        let closed = boundary.bind_agent(&owner).is_err();
        Self {
            boundary,
            sources: Mutex::new(Vec::new()),
            owner,
            closed: AtomicBool::new(closed),
            decision: Mutex::new(None),
            parked: Mutex::new(None),
            facts: Mutex::new(Facts::default()),
        }
    }

    pub(super) fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.boundary.close();
    }

    pub(super) fn live(&self) -> Result<(), AgentError> {
        if self.closed.load(Ordering::Acquire) {
            Err(unavailable())
        } else {
            Ok(())
        }
    }

    pub(super) fn retained(
        &self,
        event: RetainedEvent,
        message: &Msg,
        received: Option<ActualReceivedReply>,
    ) -> ActualRetainedCommit {
        let commit = ActualRetainedCommit::issued(&self.owner, event, message.clone(), received);
        self.facts.lock().unwrap().retained.push(commit.clone());
        commit
    }

    pub(super) fn retained_model(
        &self,
        message: &Msg,
        received: ActualReceivedReply,
        edits: &[ReplyEdit],
    ) -> ActualRetainedCommit {
        let commit = ActualRetainedCommit::model_issued(
            &self.owner,
            message.clone(),
            received,
            edits.to_vec(),
        );
        self.facts.lock().unwrap().retained.push(commit.clone());
        commit
    }

    pub(super) fn received(
        &self,
        response: &ModelResponse,
        receipt: SourceRef,
    ) -> ActualReceivedReply {
        let received = ActualReceivedReply::issued(
            &self.owner,
            response.message.clone(),
            response.usage.clone(),
            receipt,
        );
        self.facts.lock().unwrap().received.push(received.clone());
        received
    }

    pub(super) fn terminal(&self, outcome: ActualTerminalOutcome) {
        self.facts.lock().unwrap().terminals.push(outcome);
    }

    pub(super) fn chain(&self, fact: ActualToolFact) {
        self.facts.lock().unwrap().chains.push(fact);
    }

    pub(super) fn matches(&self, messages: &[Msg]) -> bool {
        let slots = self.sources.lock().unwrap();
        slots.len() == messages.len()
            && slots.iter().zip(messages).all(|(slot, message)| {
                let commit = match slot {
                    Slot::Bound(_, commit) | Slot::Pending(commit) => commit,
                };
                commit.owner().same_agent(&self.owner) && commit.message() == message
            })
    }

    /// Slots for inputs that were just appended, in the same order. An input
    /// that came without a source is text from another agent.
    pub(super) fn note_inputs(&self, messages: &[Msg], sources: Vec<Option<SourceRef>>) {
        let slots = messages
            .iter()
            .zip(sources)
            .map(|(message, source)| {
                let commit = self.retained(RetainedEvent::Input, message, None);
                match source {
                    Some(source) => Slot::Bound(source, commit),
                    None => Slot::Pending(commit),
                }
            })
            .collect::<Vec<_>>();
        self.sources.lock().unwrap().extend(slots);
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
        let unrecorded = |messages: &mut Vec<Msg>,
                          slots: &mut Vec<Slot>,
                          pending: &mut Vec<String>| {
            for id in pending.drain(..) {
                let message = Msg::Tool {
                    tool_call_id: id,
                    content: "Completion was not recorded before the session was interrupted. This operation may have had effects. Inspect the workspace before retrying it.".into(),
                    is_error: true,
                };
                messages.push(message.clone());
                slots.push(Slot::Pending(self.retained(
                    RetainedEvent::Framework(FrameworkCommit::UnrecordedCall),
                    &message,
                    None,
                )));
            }
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
                    unrecorded(&mut state.messages, &mut slots, &mut pending);
                }
            }
            if let Msg::Assistant { tool_calls, .. } = &message {
                pending.extend(tool_calls.iter().map(|call| call.id.clone()));
            }
            state.messages.push(message);
            slots.push(slot);
        }
        unrecorded(&mut state.messages, &mut slots, &mut pending);
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
        view.live()?;
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
        let _turn_activity = self.begin_turn()?;
        // Activity is owned before inspecting either collection. Do not keep
        // a sources guard while acquiring state: commits acquire state first.
        let consistent = {
            let state = self.state.lock().unwrap();
            view.matches(&state.messages)
        };
        if !consistent {
            view.close();
            return Err(unsourced());
        }
        let user_input = user_input.into();
        let future: std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<TurnResult, AgentError>> + Send + '_>,
        > = Box::pin(self.run_turn_inner(user_input, Some(source)));
        let result = self.in_turn_scope(future).await;
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
                    Slot::Bound(source, _) => Some(source.clone()),
                    Slot::Pending(_) => None,
                })
                .collect(),
        )
    }

    /// Actual provider/terminal/retained events of this selected agent, including
    /// facts for which projection failed. These are private-memory history;
    /// no token certifies transport, persistence, restoration or permission.
    pub fn actual_received_replies(&self) -> Option<Vec<ActualReceivedReply>> {
        Some(
            self.model_view
                .as_ref()?
                .facts
                .lock()
                .unwrap()
                .received
                .clone(),
        )
    }

    pub fn actual_retained_commits(&self) -> Option<Vec<ActualRetainedCommit>> {
        Some(
            self.model_view
                .as_ref()?
                .facts
                .lock()
                .unwrap()
                .retained
                .clone(),
        )
    }

    pub fn actual_tool_facts(&self) -> Option<Vec<ActualToolFact>> {
        Some(
            self.model_view
                .as_ref()?
                .facts
                .lock()
                .unwrap()
                .chains
                .clone(),
        )
    }

    pub fn actual_terminal_outcomes(&self) -> Option<Vec<ActualTerminalOutcome>> {
        Some(
            self.model_view
                .as_ref()?
                .facts
                .lock()
                .unwrap()
                .terminals
                .clone(),
        )
    }

    pub fn actual_parked_call(&self) -> Option<ActualParkedCall> {
        self.model_view.as_ref()?.parked.lock().unwrap().clone()
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
        if !input.active || input.idle_operation || input.cancel_requested || input.finishing {
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
        let parked = view
            .parked
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(unavailable)?;
        if parked.call() != &call {
            return Err(unsourced());
        }
        view.boundary
            .accept_parked_decision(&parked, &action, &decision)
            .await?;
        *view.decision.lock().unwrap() = Some(decision);
        let future: std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<TurnResult, AgentError>> + Send + '_>,
        > = Box::pin(self.resume_inner(action, deny_default));
        let result = self.in_turn_scope(future).await;
        self.settled(view, result).await
    }

    /// Asks the boundary for the source of every message the agent appended
    /// itself and has not asked about yet, in order.
    async fn settle_sources(&self, view: &ModelView) -> Result<(), AgentError> {
        loop {
            let next =
                view.sources.lock().unwrap().iter().enumerate().find_map(
                    |(index, slot)| match slot {
                        Slot::Pending(commit) => Some((index, commit.clone())),
                        Slot::Bound(_, _) => None,
                    },
                );
            let Some((index, commit)) = next else {
                return Ok(());
            };
            let source = view.boundary.observe_retained_framework(&commit).await?;
            match view.sources.lock().unwrap().get_mut(index) {
                Some(slot @ Slot::Pending(_)) => *slot = Slot::Bound(source, commit),
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
        let result = match result {
            Ok(result) => result,
            Err(error) => return self.view_failed(view, error).await,
        };
        if let Err(error) = self.settle_sources(view).await {
            return self.view_failed(view, error).await;
        }
        {
            let mut input = self.turn_input.lock().unwrap();
            input.idle_operation = true;
            input.control_settled = true;
        }
        Ok(result)
    }

    async fn view_failed<T>(&self, view: &ModelView, error: AgentError) -> Result<T, AgentError> {
        let pending = {
            let mut input = self.turn_input.lock().unwrap();
            input.idle_operation = true;
            input.pending.drain(..).collect()
        };
        // Retain originals without invoking a failed source callback again.
        self.append_steering_messages(pending);
        self.drain_agent_notifications_into_state();
        view.close();
        // A failed save is retained as failure, never retried as proof.
        if self.checkpoint_failed() {
            return Err(checkpoint_save_error());
        }
        self.save_checkpoint(None).await?;
        if self.checkpoint_failed() {
            return Err(checkpoint_save_error());
        }
        Err(error)
    }

    /// Notes `count` messages the agent has just appended itself.
    pub(super) fn note_framework_commits(&self, event: FrameworkCommit, messages: &[Msg]) {
        if let Some(view) = &self.model_view {
            let commits = messages
                .iter()
                .map(|message| {
                    Slot::Pending(view.retained(RetainedEvent::Framework(event), message, None))
                })
                .collect::<Vec<_>>();
            view.sources.lock().unwrap().extend(commits);
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
        view.live()?;
        self.settle_sources(view).await?;
        if !view.matches(&working.messages) {
            return Err(unsourced());
        }
        let messages = view
            .sources
            .lock()
            .unwrap()
            .iter()
            .map(|slot| match slot {
                Slot::Bound(source, _) => Some(source.clone()),
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
            retained: view
                .sources
                .lock()
                .unwrap()
                .iter()
                .map(|slot| match slot {
                    Slot::Bound(_, commit) | Slot::Pending(commit) => commit.clone(),
                })
                .collect(),
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

    /// Publishes the source only after the actual model commit/save path.
    pub(super) async fn record_model_source(
        &self,
        view: &ModelView,
        commit: ActualRetainedCommit,
        index: usize,
    ) -> Result<(), AgentError> {
        if !matches!(view.sources.lock().unwrap().get(index),
            Some(Slot::Pending(pending)) if pending.same_commit(&commit))
        {
            return Err(unsourced());
        }
        let source = view.boundary.commit_retained_model(&commit).await?;
        let mut sources = view.sources.lock().unwrap();
        match sources.get_mut(index) {
            Some(slot @ Slot::Pending(_)) => *slot = Slot::Bound(source, commit),
            _ => return Err(unsourced()),
        }
        Ok(())
    }

    /// Actual outcomes already belong to the private fact owner even if this
    /// callback fails. The full immutable retained message binds the source.
    pub(super) async fn record_tool_source(
        &self,
        view: &ModelView,
        fact: &ActualToolFact,
    ) -> Result<(), AgentError> {
        let commit = fact.commit().ok_or_else(unsourced)?;
        let index = {
            let sources = view.sources.lock().unwrap();
            sources.len().checked_sub(1).ok_or_else(unsourced)?
        };
        if !matches!(view.sources.lock().unwrap().get(index),
            Some(Slot::Pending(pending)) if pending.same_commit(commit))
        {
            return Err(unsourced());
        }
        let source = view.boundary.observe_retained_tool(fact).await?;
        let mut sources = view.sources.lock().unwrap();
        match sources.get_mut(index) {
            Some(slot @ Slot::Pending(_)) => *slot = Slot::Bound(source, commit.clone()),
            _ => return Err(unsourced()),
        }
        Ok(())
    }
}
