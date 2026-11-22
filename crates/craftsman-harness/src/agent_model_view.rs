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
//! per-round system additions, the ordinary inbox, the untyped turn, steering
//! and resume entries, reopening, transfer and the untyped idle compaction.
//!
//! A delegated agent of such an agent is an agent with a boundary of the
//! same conversation. It takes its task through [`Agent::run_task_with_view`]
//! and reads an inbox that carries parts.

use super::*;
use crate::middleware::BoxedViewFuture;
use crate::provenance::{
    FrameworkCommit, ModelDelivery, ModelViewBoundary, ObservedToolResult, OutcomePart, PartFrom,
    ReachedTool, ReplyEdit, SourcePart, SourceRef, TrackedDispatch, TrackedModelRequest,
    TrackedPrepare, TrackedReply, ViewSupport,
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
    ///
    /// `reply` is the source beside the committed reply that asked for the
    /// call, when the call ran exactly as that reply wrote it. A piece that
    /// names the call's arguments has no source without it.
    pub(super) fn outcome_parts(
        &self,
        recorded: &str,
        reply: Option<&SourceRef>,
        call_id: &str,
    ) -> Vec<SourcePart> {
        let Some(outcome) = &self.outcome else {
            return Vec::new();
        };
        let own: String = outcome.iter().map(OutcomePart::text).collect();
        let appended: String = self.appended.iter().map(|part| &*part.text).collect();
        let returned = matches!(self.reached.last().map(|run| &run.result),
            Some(Ok(returned)) if returned.content == own);
        if !returned || recorded != format!("{own}{appended}") {
            return Vec::new();
        }
        let mut parts = Vec::with_capacity(outcome.len());
        for part in outcome {
            let (text, from) = match part {
                OutcomePart::Fixed(text) => (text, PartFrom::Fixed),
                OutcomePart::Read { what, text } => (text, PartFrom::Read { what: what.clone() }),
                OutcomePart::Source { source, text } => (
                    text,
                    PartFrom::Source {
                        source: source.clone(),
                    },
                ),
                OutcomePart::ArgumentJson { pointer, text }
                | OutcomePart::Argument { pointer, text } => {
                    // Text of the call's arguments has a source only when
                    // the call ran as its reply wrote it.
                    let Some(reply) = reply else {
                        return Vec::new();
                    };
                    let (reply, call_id, pointer) =
                        (reply.clone(), call_id.to_owned(), pointer.clone());
                    let from = match part {
                        OutcomePart::ArgumentJson { .. } => PartFrom::CallJson {
                            reply,
                            call_id,
                            pointer,
                        },
                        _ => PartFrom::Call {
                            reply,
                            call_id,
                            pointer,
                        },
                    };
                    (text, from)
                }
            };
            parts.push(SourcePart {
                text: text.clone(),
                from,
            });
        }
        parts
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
    /// `origin` is the boundary of the conversation and the source beside
    /// the committed reply that asked for the call, when there is one.
    pub(super) fn new(
        state: &AgentState,
        origin: Option<(Arc<dyn ModelViewBoundary>, SourceRef, String)>,
    ) -> Self {
        Self {
            reached: Vec::new(),
            last: Kept::of(state),
            changed: false,
            todos_before: state.todos.clone(),
            tracked: TrackedDispatch {
                notes: Default::default(),
                origin: origin.map(|(boundary, reply, call_id)| {
                    Arc::new(crate::provenance::DispatchOrigin {
                        boundary,
                        reply,
                        call_id,
                    })
                }),
            },
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
    /// Set for text that arrived with parts, which join to the text.
    pub(super) parts: Option<(FrameworkCommit, Arc<[SourcePart]>)>,
}

impl QueuedInput {
    pub(super) fn plain(text: String) -> Self {
        Self {
            text,
            source: None,
            parts: None,
        }
    }

    /// Text that arrived with the parts it is made of. Parts that do not
    /// join to exactly the text, or exceed the bound, are dropped: the text
    /// is then recorded without a source.
    pub(super) fn enveloped(text: String, event: FrameworkCommit, parts: Vec<SourcePart>) -> Self {
        let fits = parts.len() <= crate::provenance::MAX_ENVELOPE_PARTS
            && text.len() <= crate::provenance::MAX_ENVELOPE_BYTES
            && crate::provenance::joined(&parts) == text;
        Self {
            text,
            source: None,
            parts: match fits {
                true => Some((event, parts.into())),
                false => None,
            },
        }
    }

    /// What is kept beside the message this input is recorded as. An input
    /// that came with neither a source nor parts is text from another agent.
    pub(super) fn slot(&self) -> Slot {
        match (&self.source, &self.parts) {
            (Some(source), _) => Slot::Bound(source.clone()),
            (None, Some((event, parts))) => Slot::Envelope(*event, parts.clone()),
            (None, None) => Slot::Pending(FrameworkCommit::Notification),
        }
    }
}

fn fixed_piece(cell: &'static std::sync::OnceLock<Arc<str>>, text: &'static str) -> SourcePart {
    SourcePart::fixed(cell.get_or_init(|| Arc::from(text)).clone())
}

/// The parts of the message a notification is recorded as: the agent's own
/// framing around the sender's name and what the sender said its text is
/// made of.
pub(super) fn notification_envelope(
    notification: &crate::subagent::Notification,
    payload: &[SourcePart],
) -> Vec<SourcePart> {
    use crate::subagent::NotificationKind;
    use std::sync::OnceLock;
    static BACKGROUND: OnceLock<Arc<str>> = OnceLock::new();
    static COMPLETED: OnceLock<Arc<str>> = OnceLock::new();
    static MESSAGE: OnceLock<Arc<str>> = OnceLock::new();
    static MESSAGE_END: OnceLock<Arc<str>> = OnceLock::new();
    static WAITING: OnceLock<Arc<str>> = OnceLock::new();
    let (open, close) = match notification.kind {
        NotificationKind::Completed(_) => (
            fixed_piece(&BACKGROUND, "[background agent '"),
            fixed_piece(&COMPLETED, "' completed]\n"),
        ),
        NotificationKind::Message(_) => (
            fixed_piece(&MESSAGE, "[message from agent '"),
            fixed_piece(&MESSAGE_END, "']\n"),
        ),
        NotificationKind::NeedsApproval(_) => (
            fixed_piece(&BACKGROUND, "[background agent '"),
            fixed_piece(&WAITING, "' is waiting for approval]\n"),
        ),
    };
    let mut parts = Vec::with_capacity(payload.len() + 3);
    parts.push(open);
    parts.push(SourcePart::read(
        "agent name",
        Arc::from(notification.from.as_str()),
    ));
    parts.push(close);
    parts.extend(payload.iter().cloned());
    parts
}

pub(super) struct ModelView {
    pub(super) boundary: Arc<dyn ModelViewBoundary>,
    /// One slot per recorded message, in the order the messages were
    /// committed. It lives only in memory and is never read from a checkpoint.
    pub(super) sources: Mutex<Vec<Slot>>,
    /// Where the text of each stored todo came from, in the order of the
    /// stored list. `None` when the list was changed by something that did
    /// not say where its text came from; it then has no source.
    todos: Mutex<Option<Vec<PartFrom>>>,
}

impl ModelView {
    pub(super) fn new(boundary: Arc<dyn ModelViewBoundary>) -> Self {
        Self {
            boundary,
            sources: Mutex::new(Vec::new()),
            todos: Mutex::new(Some(Vec::new())),
        }
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
    pub(super) fn note_inputs(&self, sources: Vec<Option<SourceRef>>) {
        self.note_slots(
            sources
                .into_iter()
                .map(|source| match source {
                    Some(source) => Slot::Bound(source),
                    None => Slot::Pending(FrameworkCommit::Notification),
                })
                .collect(),
        );
    }

    /// Slots for messages that were just appended, in the same order.
    pub(super) fn note_slots(&self, slots: Vec<Slot>) {
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
                    Slot::Pending(_) | Slot::Envelope(..) => None,
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
            parts: None,
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
                        Slot::Pending(event) => Some((index, *event, None)),
                        Slot::Envelope(event, parts) => Some((index, *event, Some(parts.clone()))),
                        Slot::Bound(_) => None,
                    },
                );
            let Some((index, event, parts)) = next else {
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
            let source = match &parts {
                Some(parts) => {
                    view.boundary
                        .observe_envelope(event, parts, &message)
                        .await?
                }
                None => {
                    view.boundary
                        .observe_framework_commit(event, &message)
                        .await?
                }
            };
            match view.sources.lock().unwrap().get_mut(index) {
                Some(slot @ (Slot::Pending(_) | Slot::Envelope(..))) => *slot = Slot::Bound(source),
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
    pub(super) fn commit_slots(&self, view: &ModelView, staged: Option<Vec<Slot>>) {
        if let Some(staged) = staged {
            *view.sources.lock().unwrap() = staged;
        }
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
            Some(Slot::Bound(source)) => Some(source.clone()),
            _ => None,
        }
    }

    /// Whether `call` is exactly a call of the last recorded reply that
    /// holds its id. A call a person changed before it ran is not.
    pub(super) fn asked_as_written(&self, call: &ToolCall) -> bool {
        self.state
            .lock()
            .unwrap()
            .messages
            .iter()
            .rev()
            .find_map(|message| match message {
                Msg::Assistant { tool_calls, .. } => tool_calls
                    .iter()
                    .find(|asked| asked.id == call.id)
                    .map(|asked| asked == call),
                _ => None,
            })
            .unwrap_or(false)
    }

    /// What a dispatch on a selected agent starts from: the boundary and the
    /// source beside the committed reply that asked for the call.
    pub(super) fn dispatch_origin(
        &self,
        call_id: &str,
    ) -> Option<(Arc<dyn ModelViewBoundary>, SourceRef, String)> {
        let view = self.model_view.as_ref()?;
        let reply = self.reply_source(view, call_id)?;
        Some((view.boundary.clone(), reply, call_id.to_owned()))
    }

    /// Records what other agents sent to this one's inbox with parts, in
    /// the order it arrived, where the ordinary inbox is read: before the
    /// round's request. Returns whether anything was recorded.
    pub(super) fn drain_view_inbox_into_state(&self) -> bool {
        let inbox = self.view_inbox.lock().unwrap().clone();
        let received = match inbox {
            Some(inbox) => std::mem::take(&mut *inbox.lock().unwrap()),
            None => return false,
        };
        if received.is_empty() || self.model_view.is_none() {
            return false;
        }
        let mut messages = Vec::with_capacity(received.len());
        for parts in received {
            let text = crate::provenance::joined(&parts);
            messages.push(QueuedInput::enveloped(
                text,
                FrameworkCommit::InboxMessage,
                parts,
            ));
        }
        self.append_steering_messages(messages);
        true
    }

    /// Runs one delegated task on an agent that selected a source view
    /// boundary: [`run_task_with_inbox`] with the typed turn. The agent is
    /// seeded from `inherited` exactly as the ordinary task is, `prompt` is
    /// recorded as its input and `source` is kept beside it. `inbox` carries
    /// what other agents send while the task runs, as parts.
    ///
    /// [`run_task_with_inbox`]: crate::subagent::AgentHandle::run_task_with_inbox
    pub async fn run_task_with_view(
        &self,
        prompt: &str,
        source: SourceRef,
        inherited: AgentState,
        inbox: Option<crate::subagent::ViewInbox>,
    ) -> Result<TrackedTask, AgentError> {
        let view = self.supported_view()?;
        let mut seed = inherited;
        seed.messages.clear();
        seed.todos.clear();
        seed.system_extras.clear();
        *self.state.lock().unwrap() = seed;
        view.sources.lock().unwrap().clear();
        *view.todos.lock().unwrap() = Some(Vec::new());
        self.reset_task_plumbing(None);
        *self.view_inbox.lock().unwrap() = inbox;
        let result = match self.run_turn_with_view(prompt, source).await? {
            TurnResult::Done(out) => {
                crate::subagent::TaskResult::Done(crate::subagent::AgentOutcome {
                    final_text: out.final_text().to_string(),
                    state_updates: self.state.lock().unwrap().clone(),
                })
            }
            TurnResult::Suspended(suspension) => crate::subagent::TaskResult::Suspended(suspension),
        };
        let reply = match &result {
            // The final text is the text of the last recorded message, a
            // reply or the agent's own closing note; its slot is the source.
            crate::subagent::TaskResult::Done(outcome) => {
                let state = self.state.lock().unwrap();
                let last = state.messages.len().checked_sub(1);
                let is_reply = matches!(state.messages.last(),
                    Some(Msg::Assistant { content, .. }) if *content == outcome.final_text);
                drop(state);
                match (is_reply, last) {
                    (true, Some(last)) => match view.sources.lock().unwrap().get(last) {
                        Some(Slot::Bound(source)) => Some(source.clone()),
                        _ => None,
                    },
                    _ => None,
                }
            }
            crate::subagent::TaskResult::Suspended(suspension) => {
                self.reply_source(view, &suspension.request.tool_call.id)
            }
        };
        Ok(TrackedTask { result, reply })
    }
}

/// What a delegated task of an agent with a source view ended with.
pub struct TrackedTask {
    pub result: crate::subagent::TaskResult,
    /// For a finished task, the source beside the message it ended with: a
    /// committed reply, or the agent's own closing note. Its text is the
    /// final text. For a suspended task, the source beside the committed
    /// reply that asked for the parked call. `None` when the agent holds no
    /// such source.
    pub reply: Option<SourceRef>,
}

impl Agent {
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
        self.settle_sources(view).await?;
        let current = match staged {
            Some(staged) => staged.to_vec(),
            None => view.sources.lock().unwrap().clone(),
        };
        let messages = current
            .iter()
            .map(|slot| match slot {
                Slot::Bound(source) => Some(source.clone()),
                Slot::Pending(_) | Slot::Envelope(..) => None,
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
            conversation: view.boundary.conversation(),
            summary: false,
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
        appended: &[SourcePart],
        parts: &[SourcePart],
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
                appended,
                parts,
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

impl TrackedPrepare<'_> {
    /// Appends one message of the host's own before this round's request, as
    /// the parts it is made of. It is recorded with the user role, where the
    /// ordinary hook of the middleware records it, and is committed with the
    /// round. The boundary gives its source from the parts, so nothing here
    /// can pass for text of the model or of a person.
    pub async fn append_host_message(&mut self, parts: Vec<SourcePart>) -> Result<(), AgentError> {
        let text = crate::provenance::joined(&parts);
        if parts.len() > crate::provenance::MAX_ENVELOPE_PARTS
            || text.len() > crate::provenance::MAX_ENVELOPE_BYTES
        {
            return Err(AgentError::State(
                "A host message of this conversation exceeds its bound.".into(),
            ));
        }
        if self.slots.len() != self.state.messages.len() {
            return Err(unsourced());
        }
        let message = Msg::user(text);
        let source = self
            .boundary
            .observe_envelope(FrameworkCommit::HostNote, &parts, &message)
            .await?;
        self.state.messages.push(message);
        self.slots.push(Slot::Bound(source));
        *self.replaced = true;
        Ok(())
    }

    /// Sets a key of the custom state that the middleware keeps as its own
    /// bookkeeping. Nothing the model sees is read from it.
    pub fn set_own_custom(&mut self, key: &str, value: serde_json::Value) {
        self.state.custom.insert(key.to_owned(), value);
    }

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
        if self.slots.len() != self.state.messages.len() {
            return Err(unsourced());
        }
        let bound = |slots: &[Slot], index: usize| match slots.get(index) {
            Some(Slot::Bound(source)) => Ok(source.clone()),
            _ => Err(unsourced()),
        };
        let mut messages = Vec::with_capacity(summarized.len() + 1);
        let mut sources = Vec::with_capacity(summarized.len() + 1);
        for index in summarized {
            messages.push(
                self.state
                    .messages
                    .get(*index)
                    .cloned()
                    .ok_or_else(unsourced)?,
            );
            sources.push(bound(self.slots, *index)?);
        }
        messages.push(Msg::user(&*ask.text));
        sources.push(self.boundary.host_message(&ask).await?);
        let request = TrackedModelRequest {
            system: vec![self.boundary.system_addition(&[system]).await?],
            request: ModelRequest {
                system: String::new(),
                messages,
                tools: Vec::new(),
            },
            messages: sources,
            tools: Vec::new(),
            conversation: self.boundary.conversation(),
            summary: true,
        };
        let delivery = match crate::TurnCancellation::current() {
            Some(turn) => tokio::select! {
                biased;
                _ = turn.cancelled() => return Err(AgentError::Cancelled),
                delivery = summarizer.complete_with_view(request) => delivery?,
            },
            None => summarizer.complete_with_view(request).await?,
        };
        let text = match &delivery.response.message {
            Msg::Assistant { content, .. }
            | Msg::User { content }
            | Msg::System { content }
            | Msg::Tool { content, .. } => content.clone(),
        };
        let summary = Msg::user(format!("{}{text}{}", open.text, close.text));
        let source = self
            .boundary
            .summary_source(delivery.receipt, &open, &close, &summary)
            .await?;
        let mut replaced = Vec::with_capacity(kept.len() + 1);
        let mut slots = Vec::with_capacity(kept.len() + 1);
        replaced.push(summary);
        slots.push(Slot::Bound(source));
        for index in kept {
            replaced.push(
                self.state
                    .messages
                    .get(*index)
                    .cloned()
                    .ok_or_else(unsourced)?,
            );
            slots.push(Slot::Bound(bound(self.slots, *index)?));
        }
        self.state.messages = replaced;
        self.state.last_usage = None;
        self.state.last_usage_msg_count = 0;
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
        let cancellation = self.turn_input.lock().unwrap().cancellation.clone();
        match cancellation {
            Some(turn) => {
                turn.scope(self.compact_idle_with_view_inner(view, compaction))
                    .await
            }
            None => {
                crate::cancellation::unscoped(self.compact_idle_with_view_inner(view, compaction))
                    .await
            }
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
                *view.sources.lock().unwrap() = slots;
                counts
            });
            input.finishing = true;
            counts
        };
        self.save_progress_checkpoint(None).await?;
        Ok(counts)
    }
}
