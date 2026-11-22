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
//! untracked system additions, an inbox, the untyped turn, steering and
//! resume entries, reopening and transfer. Tracked Chat compaction has its
//! own actual stage/retention witnesses; unsupported native inputs refuse.

use super::*;
use crate::ModelResponse;
use crate::middleware::BoxedViewFuture;
use crate::provenance::{
    ActualAgentOwner, ActualFacts, ActualModelInput, ActualParkedCall, ActualProviderReturn,
    ActualReceivedReply, ActualRequestInput, ActualRetainedCommit, ActualStagedSummary,
    ActualTerminalOutcome, ActualToolFact, FrameworkCommit, ModelDelivery, ModelViewBoundary,
    OutcomePart, PartFrom, ReachedTool, ReplyEdit, RetainedEvent, ReturnRecorder, SourcePart,
    SourceRef, TrackedDispatch, TrackedModelRequest, TrackedPrepare, TrackedReply, ViewSupport,
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
        self.is_except(state, &[])
    }

    /// As [`is`](Self::is), leaving out the custom-state keys a middleware
    /// announced as its own bookkeeping.
    fn is_except(&self, state: &AgentState, custom_keys: &[String]) -> bool {
        let custom = custom_keys.is_empty() && self.custom == state.custom
            || !custom_keys.is_empty() && {
                let without = |map: &serde_json::Map<String, serde_json::Value>| {
                    let mut map = map.clone();
                    for key in custom_keys {
                        map.remove(key);
                    }
                    map
                };
                without(&self.custom) == without(&state.custom)
            };
        self.messages == state.messages
            && self.todos == state.todos
            && self.files == state.files
            && custom
            && self.system_extras == state.system_extras
            && self.last_usage == state.last_usage
            && self.last_usage_msg_count == state.last_usage_msg_count
    }
}

/// What one dispatch on a selected agent left behind for the agent.
#[derive(Default)]
pub(super) struct Dispatched {
    pub(super) reached: Vec<ReachedTool>,
    /// Whether anything other than the tool changed recorded state without
    /// announcing it.
    pub(super) changed: bool,
    pub(super) appended: Vec<SourcePart>,
    /// The argument places the tool stored as the todo list, if it said so.
    pub(super) todos: Option<Vec<String>>,
    /// Whether the stored todo list differs from the one before the dispatch.
    pub(super) todos_changed: bool,
    /// The pieces the tool said its own outcome is made of.
    pub(super) outcome: Option<Vec<OutcomePart>>,
}

impl Dispatched {
    /// The tool's own content as parts, if the tool said what it is made of
    /// and the recorded content is exactly those pieces followed by what
    /// middleware announced it appended. Anything else gives no parts.
    pub(super) fn outcome_parts(
        &self,
        recorded: &str,
        reply: Option<&SourceRef>,
        call_id: &str,
    ) -> Vec<SourcePart> {
        let (Some(outcome), Some(reply)) = (&self.outcome, reply) else {
            return Vec::new();
        };
        let own: String = outcome
            .iter()
            .map(|part| match part {
                OutcomePart::Fixed(text) | OutcomePart::ArgumentJson { text, .. } => &**text,
            })
            .collect();
        let appended: String = self.appended.iter().map(|part| &*part.text).collect();
        let returned = matches!(self.reached.last().map(|run| &run.result),
            Some(Ok(returned)) if returned.content == own);
        if !returned || recorded != format!("{own}{appended}") {
            return Vec::new();
        }
        outcome
            .iter()
            .map(|part| match part {
                OutcomePart::Fixed(text) => SourcePart::fixed(text.clone()),
                OutcomePart::ArgumentJson { pointer, text } => SourcePart {
                    text: text.clone(),
                    from: PartFrom::CallJson {
                        reply: reply.clone(),
                        call_id: call_id.to_owned(),
                        pointer: pointer.clone(),
                    },
                },
            })
            .collect()
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
    todos_before: Vec<crate::state::Todo>,
    /// What the tool and the middleware announce during this dispatch.
    pub(super) tracked: TrackedDispatch,
}

impl DispatchWatch {
    pub(super) fn new(state: &AgentState) -> Self {
        Self {
            reached: Vec::new(),
            last: Kept::of(state),
            changed: false,
            todos_before: state.todos.clone(),
            tracked: TrackedDispatch::default(),
        }
    }

    fn announced_keys(&self) -> Vec<String> {
        self.tracked.notes.lock().unwrap().custom_keys.clone()
    }

    /// The tool is about to run.
    pub(super) fn entering(&mut self, state: &AgentState) {
        let keys = self.announced_keys();
        self.changed |= !self.last.is_except(state, &keys);
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
    pub(super) fn finish(&mut self, state: &AgentState) -> Dispatched {
        let keys = self.announced_keys();
        self.changed |= !self.last.is_except(state, &keys);
        let mut notes = self.tracked.notes.lock().unwrap();
        Dispatched {
            reached: std::mem::take(&mut self.reached),
            changed: self.changed,
            appended: std::mem::take(&mut notes.appended),
            todos: notes.todos.take(),
            todos_changed: self.todos_before != state.todos,
            outcome: notes.outcome.take(),
        }
    }
}

pub(super) fn state_changed() -> AgentError {
    AgentError::State(
        "A middleware of this conversation changed recorded state. Nothing further was sent."
            .into(),
    )
}

pub(super) use crate::provenance::Slot;

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

pub(super) struct AppliedDecision {
    pub(super) source: SourceRef,
    pub(super) action: crate::HitlAction,
}

pub(super) struct ModelView {
    pub(super) boundary: Arc<dyn ModelViewBoundary>,
    /// One slot per recorded message, in the order the messages were
    /// committed. It lives only in memory and is never read from a checkpoint.
    pub(super) sources: Mutex<Vec<Slot>>,
    pub(super) owner: ActualAgentOwner,
    closed: AtomicBool,
    pub(super) decision: Mutex<Option<AppliedDecision>>,
    pub(super) parked: Mutex<Option<ActualParkedCall>>,
    facts: Arc<Mutex<ActualFacts>>,
    todos: Mutex<Option<Vec<PartFrom>>>,
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
            facts: Arc::new(Mutex::new(ActualFacts::default())),
            todos: Mutex::new(Some(Vec::new())),
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
                    Slot::StagedSummary(_, _) => return false,
                };
                commit.owner().same_agent(&self.owner) && commit.message() == message
            })
    }

    /// After a dispatch: the stored todo list keeps its origins only if the
    /// tool that replaced it said which of its arguments it stored.
    pub(super) fn note_todos(
        &self,
        dispatched: &Dispatched,
        reply: Option<SourceRef>,
        call_id: &str,
        stored: usize,
    ) {
        if !dispatched.todos_changed && dispatched.todos.is_none() {
            return;
        }
        *self.todos.lock().unwrap() = match (&dispatched.todos, reply) {
            (Some(pointers), Some(reply)) if pointers.len() == stored => Some(
                pointers
                    .iter()
                    .map(|pointer| PartFrom::Call {
                        reply: reply.clone(),
                        call_id: call_id.to_owned(),
                        pointer: pointer.clone(),
                    })
                    .collect(),
            ),
            _ => None,
        };
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

/// One real selected provider terminal. Return data is kept here before
/// an around-model wrapper can change/fail it. App transports can retain
/// earlier, before their own receipt/usage/projection checks, through the
/// private per-call recorder carried only by this terminal's request.
async fn owned_model_return(
    model: &dyn ModelProvider,
    mut request: TrackedModelRequest,
    owner: &ActualAgentOwner,
    facts: Arc<Mutex<ActualFacts>>,
) -> Result<ModelDelivery, AgentError> {
    let returns = ReturnRecorder::new(owner, facts);
    let _ownership = returns.owned();
    request.returned = Some(returns.clone());
    let result = model.complete_with_view(request).await;
    if let Ok(delivery) = &result {
        returns.retain(&delivery.response);
    }
    result
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
                ViewSupport::Preserving | ViewSupport::Tracked
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
                    Slot::Pending(_) | Slot::StagedSummary(_, _) => None,
                })
                .collect(),
        )
    }

    /// Actual provider/terminal/retained events of this selected agent, including
    /// facts for which projection failed. These are private-memory history;
    /// no token certifies transport, persistence, restoration or permission.
    /// Returned data of the selected provider call, including returns whose
    /// later receipt/projection checks failed. No record grants app origin.
    pub fn actual_provider_returns(&self) -> Option<Vec<ActualProviderReturn>> {
        Some(
            self.model_view
                .as_ref()?
                .facts
                .lock()
                .unwrap()
                .provider_returns
                .clone(),
        )
    }

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
        let consistent = {
            let state = self.state.lock().unwrap();
            view.matches(&state.messages)
        };
        if !consistent {
            view.close();
            return Err(unsourced());
        }
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
        *view.decision.lock().unwrap() = Some(AppliedDecision {
            source: decision,
            action: action.clone(),
        });
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
                        Slot::StagedSummary(_, _) => None,
                    },
                );
            let Some((index, commit)) = next else {
                return Ok(());
            };
            let source = if commit.event() == RetainedEvent::Summary {
                view.boundary.observe_retained_summary(&commit).await?
            } else {
                view.boundary.observe_retained_framework(&commit).await?
            };
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

    /// The per-round preparation on a selected agent, in the order of the
    /// ordinary one. A middleware with a tracked prepare sees the state
    /// read-only and adds system text as parts. Every other middleware must
    /// leave the state alone. Returns this round's sections.
    pub(super) async fn prepare_with_view(
        &self,
        view: &ModelView,
        working: &mut AgentState,
    ) -> Result<(Vec<Vec<SourcePart>>, Option<Vec<Slot>>), AgentError> {
        // A preparation may replace recorded messages with a summary. It
        // works on a copy of the slots, which takes the place of the kept
        // ones when the working state is committed.
        self.settle_sources(view).await?;
        let mut slots = view.sources.lock().unwrap().clone();
        let mut replaced = false;
        let mut additions = Vec::new();
        // The stored todos have origins only while there is one per todo.
        let todos = view
            .todos
            .lock()
            .unwrap()
            .clone()
            .filter(|origins| origins.len() == working.todos.len());
        for entry in self.chain.middlewares() {
            if entry.view_support() == ViewSupport::Tracked {
                let mut prepare = TrackedPrepare {
                    state: working,
                    todos: todos.as_deref(),
                    additions: &mut additions,
                    boundary: view.boundary.as_ref(),
                    owner: &view.owner,
                    facts: &view.facts,
                    slots: &mut slots,
                    replaced: &mut replaced,
                };
                entry.prepare_model_with_view(&mut prepare).await?;
            } else {
                let kept = Kept::of(working);
                entry.prepare_model(working).await?;
                if !kept.is(working) {
                    return Err(state_changed());
                }
            }
        }
        Ok((additions, replaced.then_some(slots)))
    }

    /// The working state of a round was committed. Slots that a preparation
    /// of that round replaced take the place of the kept ones.
    pub(super) fn commit_slots(
        &self,
        view: &ModelView,
        staged: Option<Vec<Slot>>,
    ) -> Result<(), AgentError> {
        let Some(staged) = staged else {
            return Ok(());
        };
        // Canonical state is already committed. Hold it while checking the
        // actual items and issuing summary retention events; never issue a
        // retained token from a candidate, a label or a checkpoint image.
        let state = self.state.lock().unwrap();
        if staged.len() > state.messages.len()
            || staged
                .iter()
                .zip(&state.messages)
                .any(|(slot, message)| match slot {
                    Slot::Bound(_, commit) | Slot::Pending(commit) => commit.message() != message,
                    Slot::StagedSummary(_, stage) => stage.message() != message,
                })
        {
            return Err(unsourced());
        }
        let committed = staged
            .into_iter()
            .map(|slot| match slot {
                Slot::StagedSummary(_, stage) => {
                    let commit = ActualRetainedCommit::summary_issued(stage);
                    view.facts.lock().unwrap().retained.push(commit.clone());
                    Slot::Pending(commit)
                }
                slot => slot,
            })
            .collect();
        *view.sources.lock().unwrap() = committed;
        Ok(())
    }

    /// The source of the committed reply that asked for the call `call_id`:
    /// the slot beside the last recorded reply that holds that call.
    pub(super) fn reply_source(&self, view: &ModelView, call_id: &str) -> Option<SourceRef> {
        let index = self
            .state
            .lock()
            .unwrap()
            .messages
            .iter()
            .rposition(|message| {
                matches!(message, Msg::Assistant { tool_calls, .. }
                if tool_calls.iter().any(|call| call.id == call_id))
            })?;
        match view.sources.lock().unwrap().get(index) {
            Some(Slot::Bound(source, _)) => Some(source.clone()),
            _ => None,
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
            let verdict = if entry.view_support() == ViewSupport::Tracked {
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
        additions: Vec<Vec<SourcePart>>,
        staged: Option<&[Slot]>,
    ) -> Result<Option<ModelDelivery>, AgentError> {
        if !working.system_extras.is_empty() {
            return Err(unavailable());
        }
        view.live()?;
        self.settle_sources(view).await?;
        let current = match staged {
            Some(staged) => staged.to_vec(),
            None => view.sources.lock().unwrap().clone(),
        };
        let pairs = current
            .iter()
            .map(Slot::bound_input)
            .collect::<Option<Vec<_>>>()
            .ok_or_else(unsourced)?;
        let (messages, inputs): (Vec<_>, Vec<_>) = pairs.into_iter().unzip();
        if inputs.len() != working.messages.len()
            || inputs
                .iter()
                .zip(&working.messages)
                .any(|(input, message)| {
                    !input.owner().same_agent(&view.owner) || input.message() != message
                })
        {
            return Err(unsourced());
        }
        let definitions = self.tool_defs();
        let mut tools = Vec::with_capacity(definitions.len());
        for definition in &definitions {
            tools.push(view.boundary.tool_source(definition).await?);
        }
        let mut system = view.boundary.system_sources().await?;
        for parts in &additions {
            system.push(view.boundary.system_addition(parts).await?);
        }
        let request = TrackedModelRequest {
            system,
            request: ModelRequest {
                system: String::new(),
                messages: working.messages.clone(),
                tools: definitions,
            },
            messages,
            tools,
            retained: inputs
                .iter()
                .filter_map(|input| match input {
                    ActualModelInput::Retained(commit) => Some(commit.clone()),
                    _ => None,
                })
                .collect(),
            inputs,
            returned: None,
        };
        let model = self.model.clone();
        let owner = view.owner.clone();
        let facts = view.facts.clone();
        let terminal = move |request: TrackedModelRequest| -> BoxedViewFuture<'_> {
            let model = model.clone();
            let owner = owner.clone();
            let facts = facts.clone();
            Box::pin(
                async move { owned_model_return(model.as_ref(), request, &owner, facts).await },
            )
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

impl TrackedPrepare<'_> {
    /// Replaces recorded messages with a summary of them, made by
    /// `summarizer` through its view path.
    ///
    /// The summarizer is given `text.system` as its system text, the recorded
    /// messages at `summarized` with their own sources, and `text.ask` as a
    /// final input. The summary message is `text.open`, the reply's text and
    /// `text.close`, joined, recorded with the user role as the ordinary
    /// compaction records it. After it come the recorded messages at `kept`,
    /// with their slots. The usage anchor is reset as the ordinary compaction
    /// resets it.
    ///
    /// Nothing is replaced when the summarizer or the boundary fails, or when
    /// the turn is cancelled while the summarizer runs.
    pub async fn replace_with_summary(
        &mut self,
        summarizer: &dyn ModelProvider,
        text: crate::provenance::SummaryText,
        summarized: &[usize],
        kept: &[usize],
    ) -> Result<(), AgentError> {
        let crate::provenance::SummaryText {
            system,
            ask,
            open,
            close,
        } = text;
        let length = self.state.messages.len();
        if self.slots.len() != length {
            return Err(unsourced());
        }
        // The replacement must account for every original exactly once and
        // preserve order within both groups. An index is never a source.
        let mut covered = vec![false; length];
        for group in [summarized, kept] {
            if group.windows(2).any(|pair| pair[0] >= pair[1]) {
                return Err(unsourced());
            }
            for &index in group {
                let seen = covered.get_mut(index).ok_or_else(unsourced)?;
                if *seen {
                    return Err(unsourced());
                }
                *seen = true;
            }
        }
        if covered.iter().any(|seen| !seen) {
            return Err(unsourced());
        }
        for (slot, message) in self.slots.iter().zip(&self.state.messages) {
            let (_, input) = slot.bound_input().ok_or_else(unsourced)?;
            if !input.owner().same_agent(self.owner) || input.message() != message {
                return Err(unsourced());
            }
        }
        let mut messages = Vec::with_capacity(summarized.len() + 1);
        let mut sources = Vec::with_capacity(summarized.len() + 1);
        let mut inputs = Vec::with_capacity(summarized.len() + 1);
        for &index in summarized {
            let (source, input) = self.slots[index].bound_input().ok_or_else(unsourced)?;
            messages.push(self.state.messages[index].clone());
            sources.push(source);
            inputs.push(input);
        }
        let ask_message = Msg::user(&*ask.text);
        let ask_source = self.boundary.host_message(&ask).await?;
        let ask_input = ActualRequestInput::issued(self.owner, ask_message.clone());
        self.facts
            .lock()
            .unwrap()
            .request_inputs
            .push(ask_input.clone());
        messages.push(ask_message);
        sources.push(ask_source);
        inputs.push(ActualModelInput::RequestInput(ask_input));
        let retained = inputs
            .iter()
            .filter_map(|input| match input {
                ActualModelInput::Retained(commit) => Some(commit.clone()),
                _ => None,
            })
            .collect();
        let request = TrackedModelRequest {
            system: vec![self.boundary.system_addition(&[system]).await?],
            request: ModelRequest {
                system: String::new(),
                messages,
                tools: Vec::new(),
            },
            messages: sources,
            tools: Vec::new(),
            retained,
            inputs,
            returned: None,
        };
        self.boundary.validate_summary_inputs(&request)?;
        let delivery = match crate::TurnCancellation::current() {
            Some(turn) => tokio::select! {
                biased;
                _ = turn.cancelled() => return Err(AgentError::Cancelled),
                delivery = owned_model_return(summarizer, request, self.owner, self.facts.clone()) => delivery?,
            },
            None => owned_model_return(summarizer, request, self.owner, self.facts.clone()).await?,
        };
        // What actually came back belongs to the fact owner before any
        // projection, framing, cancellation or source check can fail.
        let received = ActualReceivedReply::issued(
            self.owner,
            delivery.response.message.clone(),
            delivery.response.usage.clone(),
            delivery.receipt,
        );
        self.facts.lock().unwrap().received.push(received.clone());
        if crate::TurnCancellation::current().is_some_and(|turn| turn.is_cancelled()) {
            return Err(AgentError::Cancelled);
        }
        let reply_text = match received.message() {
            Msg::Assistant { content, .. }
            | Msg::User { content }
            | Msg::System { content }
            | Msg::Tool { content, .. } => content,
        };
        let summary = Msg::user(format!("{}{reply_text}{}", open.text, close.text));
        let mut replaced = Vec::with_capacity(kept.len() + 1);
        let tail_slots = kept
            .iter()
            .map(|&index| self.slots[index].clone())
            .collect::<Vec<_>>();
        replaced.push(summary.clone());
        replaced.extend(kept.iter().map(|&index| self.state.messages[index].clone()));
        self.state.messages = replaced;
        self.state.last_usage = None;
        self.state.last_usage_msg_count = 0;
        // Issued at this actual working-state replacement, not at receipt
        // validation and not as a canonical retained commit.
        let stage = ActualStagedSummary::issued(self.owner, summary, received, open, close);
        self.facts
            .lock()
            .unwrap()
            .staged_summaries
            .push(stage.clone());
        let source = self.boundary.staged_summary_source(&stage).await?;
        let mut slots = Vec::with_capacity(tail_slots.len() + 1);
        slots.push(Slot::StagedSummary(source, stage));
        slots.extend(tail_slots);
        *self.slots = slots;
        *self.replaced = true;
        Ok(())
    }
}

impl Agent {
    /// The idle compaction of a selected agent: [`compact_idle`] with the
    /// tracked path. The same conditions, the same exclusive activity, the
    /// same saves before and after, and the same refusals when an approval is
    /// pending, when the turn is cancelled or when the conversation changed
    /// while the summarizer ran. The summarizer is asked through its view
    /// path, and the summary and the kept messages keep their sources.
    ///
    /// [`compact_idle`]: Agent::compact_idle
    pub async fn compact_idle_with_view(
        &self,
        compaction: &crate::builtin::CompactionMiddleware,
    ) -> Result<Option<(usize, usize)>, AgentError> {
        let view = self.supported_view()?;
        if !self.durable_turn_checkpoints {
            return Err(AgentError::State(
                "Durable idle compaction is not enabled for this agent.".into(),
            ));
        }
        let _activity = self.begin_activity(true)?;
        let consistent = {
            let state = self.state.lock().unwrap();
            view.matches(&state.messages)
        };
        if !consistent {
            view.close();
            return Err(unsourced());
        }
        let cancellation = self.turn_input.lock().unwrap().cancellation.clone();
        let result = match cancellation {
            Some(turn) => {
                turn.scope(self.compact_idle_with_view_inner(view, compaction))
                    .await
            }
            None => {
                crate::cancellation::unscoped(self.compact_idle_with_view_inner(view, compaction))
                    .await
            }
        };
        match result {
            Ok(counts) => Ok(counts),
            Err(error) => self.view_failed(view, error).await,
        }
    }

    async fn compact_idle_with_view_inner(
        &self,
        view: &ModelView,
        compaction: &crate::builtin::CompactionMiddleware,
    ) -> Result<Option<(usize, usize)>, AgentError> {
        if self.pending.lock().unwrap().is_some()
            || self.parked_suspension.lock().unwrap().is_some()
        {
            return Err(AgentError::State(
                "Resolve the pending approval before compacting the conversation.".into(),
            ));
        }
        if self.take_cancel_request() {
            return Err(AgentError::Cancelled);
        }
        view.reconcile(&mut self.state.lock().unwrap());
        self.save_progress_checkpoint(None).await?;
        self.settle_sources(view).await?;
        let mut working = self.snapshot();
        let messages = working.messages.clone();
        let mut slots = view.sources.lock().unwrap().clone();
        let mut replaced = false;
        let mut additions = Vec::new();
        {
            let mut prepare = TrackedPrepare {
                state: &mut working,
                todos: None,
                additions: &mut additions,
                boundary: view.boundary.as_ref(),
                owner: &view.owner,
                facts: &view.facts,
                slots: &mut slots,
                replaced: &mut replaced,
            };
            compaction.compact_now_with_view(&mut prepare).await?;
        }
        let counts = {
            let mut input = self.turn_input.lock().unwrap();
            if input.cancel_requested
                || input
                    .cancellation
                    .as_ref()
                    .is_some_and(|turn| turn.is_cancelled())
            {
                return Err(AgentError::Cancelled);
            }
            let mut state = self.state.lock().unwrap();
            if state.messages != messages {
                return Err(AgentError::State(
                    "Conversation changed during compaction; the summary was discarded.".into(),
                ));
            }
            let counts = replaced.then(|| {
                let counts = (messages.len(), working.messages.len());
                state.messages = working.messages;
                state.last_usage = None;
                state.last_usage_msg_count = 0;
                let committed_slots = slots
                    .into_iter()
                    .map(|slot| match slot {
                        Slot::StagedSummary(_, stage) => {
                            let commit = ActualRetainedCommit::summary_issued(stage);
                            view.facts.lock().unwrap().retained.push(commit.clone());
                            Slot::Pending(commit)
                        }
                        slot => slot,
                    })
                    .collect();
                *view.sources.lock().unwrap() = committed_slots;
                counts
            });
            input.finishing = true;
            counts
        };
        self.save_progress_checkpoint(None).await?;
        self.settle_sources(view).await?;
        Ok(counts)
    }
}
