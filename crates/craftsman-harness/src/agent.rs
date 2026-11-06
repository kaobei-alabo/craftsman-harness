//! The Agent loop and its builder.
//!
//! `run_turn` drives the agent cycle: push the user message,
//! run `turn_start`, then loop model→tools until the model answers plainly,
//! a middleware `End`s the turn, or the `max_rounds` guard fires.
//!
//! ## State-sharing design (preserve across Tasks 8-11)
//! The default [`StateFs`] backend shares the **committed** state `Arc`
//! (`self.state`), while every hook and tool operates on a **working clone**
//! (`ToolCtx.state`) that is committed back after each round / each tool
//! dispatch.
//!
//! During a tool dispatch a tool can therefore mutate state through **two
//! channels**:
//! 1. `ctx.state` — the working clone (todos, custom, system_extras, and its own
//!    view of `.files`). These are private until commit.
//! 2. `ctx.backend` — the default [`StateFs`] holds the *committed* `Arc`, so a
//!    `backend.write`/`edit` mutates `committed.files` **immediately**, mid-
//!    dispatch, independent of the working clone.
//!
//! A blanket `commit(working)` would overwrite `committed` with the pre-dispatch
//! snapshot and silently discard channel 2's writes. So the dispatch commit runs
//! a **three-way merge** (`merge_dispatch_result`) over the `t0` snapshot,
//! `committed` (post backend writes), and the tool's `working` clone: non-file
//! fields take the working view; `.files` keeps committed's entries (backend
//! writes) then replays the working copy's file deltas vs `t0`. A backend that
//! never touches `.files` (e.g. a real filesystem) sees an identical no-op merge.
//! Plan 2 revisits this with suspension snapshots.

use crate::backend::{Backend, StateFs};
use crate::checkpoint::{Checkpointer, TranscriptEvent, TranscriptSink};
use crate::error::AgentError;
use crate::interrupt::Suspension;
use crate::middleware::{
    AgentMiddleware, BoxedModelFuture, BoxedToolFuture, Flow, MiddlewareChain,
};
use crate::model::{ModelProvider, ModelRequest, ToolDef};
use crate::state::{AgentState, Msg, ToolCall};
use crate::tool::{Tool, ToolCtx, ToolOutcome};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// The result of a single [`Agent::run_turn`]: the final assistant message.
#[derive(Debug, Clone)]
pub struct TurnOutput {
    pub message: Msg,
}

impl TurnOutput {
    /// Text content of the final message (empty for a tool-call-only message).
    pub fn final_text(&self) -> &str {
        match &self.message {
            Msg::Assistant { content, .. } => content,
            Msg::System { content } | Msg::User { content } => content,
            Msg::Tool { content, .. } => content,
        }
    }
}

/// Outcome of a turn: either it ran to completion (`Done`) or a gated tool call
/// paused it awaiting a human decision (`Suspended`). A suspended turn is
/// finished by [`Agent::resume`].
#[derive(Debug)]
pub enum TurnResult {
    Done(TurnOutput),
    Suspended(crate::interrupt::Suspension),
}

/// The parked state of a suspended turn: the gated `call` awaiting a decision,
/// the `remaining` calls in the same round not yet dispatched, and the `round`
/// index so [`Agent::resume`] can continue the model↔tool loop afterward.
struct PendingResume {
    call: ToolCall,
    remaining: Vec<ToolCall>,
    round: usize,
}

#[derive(Default)]
struct TurnInputState {
    active: bool,
    pending: VecDeque<String>,
    cancel_requested: bool,
    cancellation: Option<crate::TurnCancellation>,
    finishing: bool,
}

/// Resets the active-turn bit on every exit path, including model/tool errors
/// and suspensions. Normal completion marks the turn inactive earlier, while
/// atomically checking for a last-moment steer.
struct TurnActivityGuard {
    state: Arc<Mutex<TurnInputState>>,
}

enum TurnBoundaryAction {
    Continue,
    Finish,
    Cancel,
}

impl Drop for TurnActivityGuard {
    fn drop(&mut self) {
        let mut state = self.state.lock().unwrap();
        state.active = false;
        state.cancel_requested = false;
        state.cancellation = None;
        state.finishing = false;
    }
}

/// A Claude-Code-style agent: a model, a tool set, a middleware chain, and a
/// backend, driven by [`run_turn`](Agent::run_turn).
#[derive(Clone)]
pub struct Agent {
    pub state: Arc<Mutex<AgentState>>,
    name: String,
    model: Arc<dyn ModelProvider>,
    tools: HashMap<String, Arc<dyn Tool>>,
    chain: MiddlewareChain,
    backend: Arc<dyn Backend>,
    system_prompt: String,
    max_rounds: usize,
    /// Shared subagent registry: the `task` tool records spawned children here so
    /// the app (and peers) can list/poll them. Every dispatched `ToolCtx` clones
    /// this `Arc`.
    registry: Arc<crate::subagent::AgentRegistry>,
    /// Notification bus: background subagents and `send_message` push here.
    /// The agent keeps one subscription for addressed feedback; applications may
    /// keep another for display/telemetry.
    notifications: crate::subagent::NotificationQueue,
    /// Independent subscription used to feed addressed background-agent
    /// completions/messages back into this agent's model context. The app owns
    /// a second subscription for display, so neither consumer steals events
    /// from the other.
    notification_inbox: Arc<Mutex<crate::subagent::NotificationReceiver>>,
    /// Parked turn state set when a gated tool call suspends the turn; consumed
    /// by [`resume`](Agent::resume).
    pending: Arc<Mutex<Option<PendingResume>>>,
    /// The full [`Suspension`] of the currently parked turn, kept alongside
    /// [`pending`](Self::pending) (which only retains the parked call + loop
    /// bookkeeping) so the app can inspect a re-parked approval after
    /// [`load_thread`](Agent::load_thread) via
    /// [`pending_suspension`](Agent::pending_suspension). Set at every park
    /// and by `load_thread`; cleared when [`resume`](Agent::resume) consumes
    /// the parked turn. `None` when not suspended.
    parked_suspension: Arc<Mutex<Option<Suspension>>>,
    /// Optional mailbox receiver drained at each round boundary: any queued
    /// messages are injected as `user` turns before the round's model call. Set
    /// by [`AgentHandle::run_task_with_inbox`] for a background subagent so
    /// `send_message` delivery lands in-transcript; `None` for a plain
    /// foreground turn.
    inbox: Arc<Mutex<Option<tokio::sync::mpsc::UnboundedReceiver<String>>>>,
    /// User messages steering the currently active foreground turn. Messages
    /// are drained at model-round boundaries; a pending steer forces another
    /// model sample even when the previous sample looked final.
    turn_input: Arc<Mutex<TurnInputState>>,
    /// Wakes an opted-in model wait; the turn-input flag remains stop authority.
    model_cancel: Arc<tokio::sync::Notify>,
    interrupt_model_on_cancel: bool,
    cancellation_session: Option<crate::SessionCancellation>,
    /// Optional durable state store. When set, the committed snapshot is saved at
    /// every round commit, at a suspension park, and at turn end. Explicit
    /// durable-turn mode also saves progress and propagates fixed save errors.
    /// Defaults to `None` (no persistence).
    checkpointer: Option<Arc<dyn Checkpointer>>,
    durable_turn_checkpoints: bool,
    checkpoint_failed: Arc<std::sync::atomic::AtomicBool>,
    recovery_info: Arc<Mutex<Option<crate::checkpoint::RecoveryInfo>>>,
    /// Optional append-only observability sink. When set, lifecycle events (user
    /// / assistant / tool / suspended / resumed) are journaled fire-and-forget.
    /// Defaults to `None`.
    journal: Option<Arc<dyn TranscriptSink>>,
    /// Thread identity for checkpoint/journal writes. Behind a `Mutex` because
    /// `Agent` is used behind `&` and [`load_thread`](Agent::load_thread)
    /// switches it. Defaults to `"default"` — the app always passes one.
    thread_id: Arc<Mutex<String>>,
    /// Monotonic checkpoint sequence, bumped on every save.
    seq: Arc<AtomicU64>,
}

impl Agent {
    /// Start building an agent around `model`.
    pub fn builder(model: Arc<dyn ModelProvider>) -> AgentBuilder {
        AgentBuilder::new(model)
    }

    /// This agent's name (defaults to `"agent"`); used to label suspension
    /// frames and registry entries for subagents.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Tool definitions offered to the model, sorted by name for determinism.
    fn tool_defs(&self) -> Vec<ToolDef> {
        let mut defs: Vec<ToolDef> = self.tools.values().map(|t| t.def()).collect();
        defs.sort_by(|a, b| a.name.cmp(&b.name));
        defs
    }

    /// Comma-joined, sorted list of available tool names (for error messages).
    fn tool_names(&self) -> String {
        let mut names: Vec<&str> = self.tools.keys().map(|s| s.as_str()).collect();
        names.sort_unstable();
        names.join(", ")
    }

    /// Run one turn: append `user_input`, then loop model↔tools to completion.
    /// Returns [`TurnResult::Suspended`] if a gated tool call pauses the turn;
    /// finish it with [`resume`](Agent::resume).
    pub async fn run_turn(&self, user_input: impl Into<String>) -> Result<TurnResult, AgentError> {
        let _turn_activity = self.begin_turn()?;
        let user_input = user_input.into();
        self.in_turn_scope(self.run_turn_inner(user_input)).await
    }

    async fn in_turn_scope<F>(&self, work: F) -> Result<TurnResult, AgentError>
    where
        F: std::future::Future<Output = Result<TurnResult, AgentError>>,
    {
        let cancellation = self.turn_input.lock().unwrap().cancellation.clone();
        match cancellation {
            Some(turn) => {
                turn.scope(async {
                    match work.await {
                        Err(AgentError::Cancelled)
                            if turn.is_cancelled()
                                && !self.turn_input.lock().unwrap().finishing =>
                        {
                            self.finish_cancelled().await
                        }
                        result => result,
                    }
                })
                .await
            }
            None => crate::cancellation::unscoped(work).await,
        }
    }

    async fn run_turn_inner(&self, user_input: String) -> Result<TurnResult, AgentError> {
        if self.durable_turn_checkpoints && self.pending.lock().unwrap().is_some() {
            return Err(AgentError::State(
                "Resolve the pending approval before starting another turn.".into(),
            ));
        }
        // Preserve temporal order across turns: a background result that
        // arrived while idle belongs before the user's next message.
        self.drain_agent_notifications_into_state();

        // 1. Commit the user message.
        let user_msg = Msg::user(user_input);
        self.journal("user", serde_json::to_value(&user_msg).unwrap_or_default());
        {
            let mut committed = self.state.lock().unwrap();
            if self.durable_turn_checkpoints {
                reconcile_recovered_calls(&mut committed, None);
            }
            committed.messages.push(user_msg);
        }
        self.save_progress_checkpoint(None).await?;

        // 2. turn_start on a working clone, committed back.
        {
            let mut working = self.snapshot();
            self.chain.turn_start(&mut working).await?;
            self.commit(working);
        }

        // 3. Model↔tool loop.
        self.drive_from_round(0).await
    }

    /// The model↔tool loop, entered at round `start`. `run_turn` calls it with 0;
    /// `resume` calls it with `pending.round + 1` after handling the parked call
    /// and any remaining calls of the suspended round. Shared so both paths run
    /// identical round choreography.
    async fn drive_from_round(&self, start: usize) -> Result<TurnResult, AgentError> {
        for round in start..self.max_rounds {
            if self.take_cancel_request() {
                return self.finish_cancelled().await;
            }
            // Same-turn steering and addressed subagent notifications are
            // committed before taking the round snapshot, so neither can be
            // silently lost behind an in-flight model request.
            self.drain_steering_into_state();
            self.drain_agent_notifications_into_state();
            self.save_progress_checkpoint(None).await?;
            let mut working = self.snapshot();

            // Round-boundary mailbox injection: drain any messages a peer sent
            // via `send_message` into this (background) agent's inbox and splice
            // them into the transcript as `user` turns before we call the model.
            self.drain_inbox(&mut working);

            // Rebuild the per-turn system additions from scratch each round.
            working.system_extras.clear();
            self.chain.prepare_model(&mut working).await?;

            let mut system = self.system_prompt.clone();
            for extra in &working.system_extras {
                system.push_str("\n\n");
                system.push_str(extra);
            }

            let req = ModelRequest {
                system,
                messages: working.messages.clone(),
                tools: self.tool_defs(),
            };

            // Model terminal: no argument-lifetime borrow, so a closure is fine.
            let model = self.model.clone();
            let terminal = move |req: ModelRequest| -> BoxedModelFuture<'_> {
                let model = model.clone();
                Box::pin(async move { model.complete(req).await })
            };
            let response = if self.interrupt_model_on_cancel {
                // Only model sampling is dropped. Tool dispatch, prepare/review
                // hooks and turn-end persistence retain their normal ownership.
                tokio::select! {
                    biased;
                    _ = self.model_cancelled() => return self.finish_cancelled().await,
                    response = self.chain.around_model(req, &terminal) => response?,
                }
            } else {
                self.chain.around_model(req, &terminal).await?
            };

            // Remember where the assistant message lands: review_model hooks may
            // MUTATE it in place (e.g. patch_tool_calls arg repair), and
            // dispatch/journal/finish must all see the post-hook message — a
            // pre-hook snapshot would silently nullify those repairs and let
            // the persisted transcript diverge from what actually executed.
            let assistant_idx = working.messages.len();
            working.messages.push(response.message.clone());

            // Anchor for CompactionMiddleware's token estimate: an exact
            // count as of right now, so it only needs to locally estimate
            // messages appended after this point rather than the whole
            // transcript. See `AgentState::last_usage`.
            working.last_usage = Some(response.usage.clone());
            working.last_usage_msg_count = working.messages.len();

            let decision = self.chain.review_model_flow(&mut working).await?;

            // End and Remodel both leave this round's tool_calls unanswered
            // (no tool results will follow). OpenAI-compatible providers
            // reject an assistant-with-tool_calls that has no matching tool
            // messages, so strip them, leaving plain assistant content,
            // before the message is committed.
            if !matches!(decision, Flow::Proceed) {
                if let Some(Msg::Assistant { tool_calls, .. }) =
                    working.messages.get_mut(assistant_idx)
                {
                    tool_calls.clear();
                }
            }

            // The post-hook assistant message (falling back to the raw model
            // response if a hook removed it entirely).
            let assistant = working
                .messages
                .get(assistant_idx)
                .cloned()
                .unwrap_or(response.message);
            self.commit(working);
            self.journal(
                "assistant",
                serde_json::to_value(&assistant).unwrap_or_default(),
            );
            self.save_checkpoint(None).await?;

            // A response can complete in the same poll that requests a stop.
            // Keep its actual usage/transcript and balance calls without dispatch.
            if self.take_cancel_request() {
                self.append_cancelled_tool_results(&assistant);
                return self.finish_cancelled().await;
            }

            match decision {
                Flow::Finish => {
                    match self.turn_boundary_action() {
                        TurnBoundaryAction::Continue => continue,
                        TurnBoundaryAction::Cancel => return self.finish_cancelled().await,
                        TurnBoundaryAction::Finish => {}
                    }
                    return self.finish(assistant).await;
                }
                Flow::Resample => continue,
                Flow::Proceed => {
                    let calls: Vec<ToolCall> = match &assistant {
                        Msg::Assistant { tool_calls, .. } => tool_calls.clone(),
                        _ => vec![],
                    };
                    if calls.is_empty() {
                        match self.turn_boundary_action() {
                            TurnBoundaryAction::Continue => continue,
                            TurnBoundaryAction::Cancel => return self.finish_cancelled().await,
                            TurnBoundaryAction::Finish => {}
                        }
                        return self.finish(assistant).await;
                    }
                    for (i, c) in calls.iter().enumerate() {
                        if self.take_cancel_request() {
                            self.append_cancelled_calls(&calls[i..]);
                            return self.finish_cancelled().await;
                        }
                        match self.dispatch_call(c.clone()).await {
                            Ok(()) => {}
                            Err(AgentError::Suspended(s)) => {
                                // A gated call paused the turn: park the call, the
                                // not-yet-dispatched rest of this round, and the
                                // round index for `resume` to pick up.
                                *self.pending.lock().unwrap() = Some(PendingResume {
                                    call: c.clone(),
                                    remaining: calls[i + 1..].to_vec(),
                                    round,
                                });
                                *self.parked_suspension.lock().unwrap() = Some((*s).clone());
                                self.journal(
                                    "suspended",
                                    serde_json::to_value(s.as_ref()).unwrap_or_default(),
                                );
                                self.save_checkpoint(Some(&s)).await?;
                                return Ok(TurnResult::Suspended(*s));
                            }
                            Err(error) => return Err(error),
                        }
                    }
                }
            }
        }

        Err(AgentError::MaxRounds(self.max_rounds))
    }

    /// Finish a suspended turn by applying the human's `action` to the parked
    /// tool call, then draining the rest of the round and continuing the
    /// model↔tool loop. `deny_default` is the tool-message text used for a
    /// `Deny` with no feedback. Errors with [`AgentError::State`] if no turn is
    /// suspended.
    pub async fn resume(
        &self,
        action: crate::interrupt::HitlAction,
        deny_default: &str,
    ) -> Result<TurnResult, AgentError> {
        let _turn_activity = self.begin_turn()?;
        self.in_turn_scope(self.resume_inner(action, deny_default))
            .await
    }

    async fn resume_inner(
        &self,
        action: crate::interrupt::HitlAction,
        deny_default: &str,
    ) -> Result<TurnResult, AgentError> {
        use crate::interrupt::HitlAction;
        let pending = self
            .pending
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| AgentError::State("no suspension to resume".into()))?;
        let PendingResume {
            call,
            remaining,
            round,
        } = pending;
        // This parked turn is being consumed; clear the retained suspension so
        // `pending_suspension` reports `None` unless a later call re-parks.
        *self.parked_suspension.lock().unwrap() = None;
        self.journal("resumed", serde_json::to_value(&action).unwrap_or_default());
        // Retire the durable approval BEFORE executing an approved call. A
        // crash after approval must recover an unknown operation, not a stale
        // parked approval that appears safe to execute for the first time.
        self.save_progress_checkpoint(None).await?;

        // Apply the decision to the parked call.
        match action {
            HitlAction::Deny { feedback } => {
                let content = if feedback.is_empty() {
                    deny_default.to_string()
                } else {
                    format!("{deny_default} Feedback: {feedback}")
                };
                self.state.lock().unwrap().messages.push(Msg::Tool {
                    tool_call_id: call.id,
                    content,
                    is_error: true,
                });
                self.save_progress_checkpoint(None).await?;
            }
            HitlAction::Approve => {
                self.dispatch_approved(call).await?;
            }
            HitlAction::ApproveEdited { args } => {
                let mut edited = call;
                edited.args = args;
                self.dispatch_approved(edited).await?;
            }
        }

        // Drain the remaining calls of the suspended round in order.
        for (i, c) in remaining.iter().enumerate() {
            match self.dispatch_call(c.clone()).await {
                Ok(()) => {}
                Err(AgentError::Suspended(s)) => {
                    *self.pending.lock().unwrap() = Some(PendingResume {
                        call: c.clone(),
                        remaining: remaining[i + 1..].to_vec(),
                        round,
                    });
                    *self.parked_suspension.lock().unwrap() = Some((*s).clone());
                    self.journal(
                        "suspended",
                        serde_json::to_value(s.as_ref()).unwrap_or_default(),
                    );
                    self.save_checkpoint(Some(&s)).await?;
                    return Ok(TurnResult::Suspended(*s));
                }
                Err(error) => return Err(error),
            }
        }

        // Transcript reconciliation before re-entering the loop: after a
        // process restart, `load_thread` re-parks with `remaining: []` (the
        // not-yet-dispatched sibling calls of the suspended round are not
        // persisted), so a multi-call round would leave tool_calls with no tool
        // results — which OpenAI-compatible providers reject with a 400.
        // Synthesize an is_error result for any still-unanswered call on the
        // last assistant message so the transcript stays valid.
        {
            let mut committed = self.state.lock().unwrap();
            let answered: std::collections::HashSet<String> = committed
                .messages
                .iter()
                .filter_map(|m| match m {
                    Msg::Tool { tool_call_id, .. } => Some(tool_call_id.clone()),
                    _ => None,
                })
                .collect();
            let unanswered: Vec<String> = committed
                .messages
                .iter()
                .rev()
                .find_map(|m| match m {
                    Msg::Assistant { tool_calls, .. } if !tool_calls.is_empty() => {
                        Some(tool_calls.iter().map(|c| c.id.clone()).collect::<Vec<_>>())
                    }
                    _ => None,
                })
                .unwrap_or_default()
                .into_iter()
                .filter(|id| !answered.contains(id))
                .collect();
            for id in unanswered {
                committed.messages.push(Msg::Tool {
                    tool_call_id: id,
                    content: "Tool call was not executed (the session was interrupted while awaiting approval).".into(),
                    is_error: true,
                });
            }
        }

        // Continue the model↔tool loop from the next round.
        self.drive_from_round(round + 1).await
    }

    /// Dispatch an approved `call` during a resume, bypassing the middleware
    /// chain so the gate that already fired isn't consulted again. Because the
    /// chain is skipped it cannot re-suspend, so it only ever appends a tool
    /// result (or propagates a non-suspension error).
    async fn dispatch_approved(&self, call: ToolCall) -> Result<(), AgentError> {
        self.dispatch_call_inner(call, true).await
    }

    /// Terminal exit of a turn: fire `turn_end` (the sixth lifecycle hook)
    /// on a working copy, commit it, and return the final message. Runs once
    /// per turn on the End / no-more-tool-calls paths (not on `MaxRounds`,
    /// which is an error exit).
    async fn finish(&self, message: Msg) -> Result<TurnResult, AgentError> {
        self.turn_input.lock().unwrap().finishing = true;
        let mut working = self.snapshot();
        self.chain.turn_end(&mut working).await?;
        self.commit(working);
        self.save_checkpoint(None).await?;
        Ok(TurnResult::Done(TurnOutput { message }))
    }

    async fn finish_cancelled(&self) -> Result<TurnResult, AgentError> {
        let message = Msg::assistant("Stopped by the user.");
        self.journal(
            "assistant",
            serde_json::to_value(&message).unwrap_or_default(),
        );
        self.state.lock().unwrap().messages.push(message.clone());
        self.finish(message).await
    }

    fn append_cancelled_tool_results(&self, assistant: &Msg) {
        let calls = match assistant {
            Msg::Assistant { tool_calls, .. } => tool_calls.as_slice(),
            _ => &[],
        };
        self.append_cancelled_calls(calls);
    }

    fn append_cancelled_calls(&self, calls: &[ToolCall]) {
        if calls.is_empty() {
            return;
        }
        let messages = calls
            .iter()
            .map(|call| Msg::Tool {
                tool_call_id: call.id.clone(),
                content: "Tool call cancelled by the user.".into(),
                is_error: true,
            })
            .collect::<Vec<_>>();
        for message in &messages {
            self.journal("tool", serde_json::to_value(message).unwrap_or_default());
        }
        self.state.lock().unwrap().messages.extend(messages);
    }

    /// Dispatch a single tool call through the middleware onion, commit any
    /// state mutations, and append the resulting `Tool` message. Most errors
    /// become an `is_error` tool result rather than crashing the loop; a
    /// [`AgentError::Suspended`] is instead propagated so the caller can pause
    /// the turn (no tool message is appended for it).
    async fn dispatch_call(&self, call: ToolCall) -> Result<(), AgentError> {
        self.dispatch_call_inner(call, false).await
    }

    /// Core dispatch. When `bypass_chain` is true the middleware onion is
    /// skipped and the tool terminal is invoked directly — used by [`resume`]
    /// for a human-approved call so the gate that already fired isn't consulted
    /// a second time (which would re-suspend the same call forever).
    async fn dispatch_call_inner(
        &self,
        call: ToolCall,
        bypass_chain: bool,
    ) -> Result<(), AgentError> {
        let id = call.id.clone();
        // t0 = the committed state snapshot the tool starts from. Keep it so the
        // commit can distinguish the tool's `ctx.state` file deltas from the
        // backend's mid-dispatch writes to committed.files (see
        // `merge_dispatch_result` and the two-channel note atop this module).
        let t0 = self.snapshot();
        let mut working = t0.clone();
        let backend = self.backend.clone();
        let tools = self.tools.clone();
        let names = self.tool_names();

        let result = {
            let mut ctx = ToolCtx {
                state: &mut working,
                backend,
                registry: self.registry.clone(),
                notifications: self.notifications.clone(),
            };
            // Terminal handler: look the tool up by name and invoke it, or
            // return an `is_error` outcome for an unknown tool. Bound through
            // `as_tool_handler` so the higher-ranked return-lifetime resolves.
            let terminal = as_tool_handler(move |call: ToolCall, ctx: &mut ToolCtx<'_>| {
                let found = tools.get(&call.name).cloned();
                let names = names.clone();
                Box::pin(async move {
                    match found {
                        Some(tool) => tool.call(call.args, ctx).await,
                        None => Ok(ToolOutcome::err(format!(
                            "Tool '{}' not found. Available: {}",
                            call.name, names
                        ))),
                    }
                })
            });
            if bypass_chain {
                terminal(call, &mut ctx).await
            } else {
                self.chain.around_tool(call, &mut ctx, &terminal).await
            }
        };

        // Three-way merge instead of a blanket overwrite commit: the backend may
        // have written to committed.files mid-dispatch (StateFs shares the Arc),
        // which a plain `commit(working)` would clobber with the stale snapshot.
        {
            let mut committed = self.state.lock().unwrap();
            merge_dispatch_result(&mut committed, &t0, working);
        }

        let msg = match result {
            Ok(outcome) => Msg::Tool {
                tool_call_id: id,
                content: outcome.content,
                is_error: outcome.is_error,
            },
            // A gated call raised a suspension: propagate it untouched so the
            // caller can pause the turn. No tool message is appended.
            Err(e @ AgentError::Suspended(_)) => return Err(e),
            Err(e) => Msg::Tool {
                tool_call_id: id,
                content: e.to_string(),
                is_error: true,
            },
        };
        self.journal("tool", serde_json::to_value(&msg).unwrap_or_default());
        self.state.lock().unwrap().messages.push(msg);
        self.save_progress_checkpoint(None).await?;
        Ok(())
    }

    /// Add a user message to the currently active turn. The message is sampled
    /// at the next safe model-round boundary; it does not cancel an in-flight
    /// model request or tool call. Returns `false` if no turn is active so the
    /// caller can queue it as a separate follow-up turn instead.
    pub fn steer(&self, message: impl Into<String>) -> bool {
        let message = message.into();
        if message.trim().is_empty() {
            return false;
        }
        let mut input = self.turn_input.lock().unwrap();
        if !input.active {
            return false;
        }
        input.pending.push_back(message);
        true
    }

    pub fn is_turn_active(&self) -> bool {
        self.turn_input.lock().unwrap().active
    }

    /// Request cancellation of the active turn. By default the loop stops at
    /// its next safe boundary. With `interrupt_model_on_cancel` enabled, a
    /// pending model sample is dropped; tools still finish their owned work.
    /// Skipped committed tool calls receive cancelled results.
    pub fn request_cancel(&self) -> bool {
        let mut input = self.turn_input.lock().unwrap();
        if !input.active {
            return false;
        }
        input.cancel_requested = true;
        input.pending.clear();
        if let Some(turn) = &input.cancellation {
            turn.cancel();
        }
        self.model_cancel.notify_waiters();
        true
    }

    async fn model_cancelled(&self) {
        if let Some(turn) = crate::TurnCancellation::current() {
            turn.cancelled().await;
            return;
        }
        loop {
            let notified = self.model_cancel.notified();
            tokio::pin!(notified);
            // Register before checking the authoritative flag: a stop between
            // that check and the first await must not strand a stalled stream.
            notified.as_mut().enable();
            if self.turn_input.lock().unwrap().cancel_requested {
                return;
            }
            notified.await;
        }
    }

    /// Drop queued steering input. Intended for explicit conversation resets
    /// and thread switches while no turn is running.
    pub fn clear_pending_input(&self) {
        let mut input = self.turn_input.lock().unwrap();
        input.pending.clear();
        input.cancel_requested = false;
    }

    fn begin_turn(&self) -> Result<TurnActivityGuard, AgentError> {
        let mut input = self.turn_input.lock().unwrap();
        if input.active {
            return Err(AgentError::State("an agent turn is already running".into()));
        }
        let cancellation = if self.interrupt_model_on_cancel {
            Some(
                crate::TurnCancellation::current()
                    .filter(|turn| {
                        self.cancellation_session
                            .as_ref()
                            .is_none_or(|session| session.same_owner(&turn.session()))
                    })
                    .unwrap_or_else(|| {
                        self.cancellation_session
                            .clone()
                            .unwrap_or_default()
                            .new_turn()
                    }),
            )
        } else {
            None
        };
        if cancellation
            .as_ref()
            .is_some_and(|turn| turn.is_cancelled())
        {
            return Err(AgentError::Cancelled);
        }
        input.active = true;
        input.cancel_requested = false;
        input.cancellation = cancellation;
        input.finishing = false;
        drop(input);
        Ok(TurnActivityGuard {
            state: self.turn_input.clone(),
        })
    }

    fn drain_steering_into_state(&self) {
        let pending = {
            let mut input = self.turn_input.lock().unwrap();
            input.pending.drain(..).collect::<Vec<_>>()
        };
        self.append_steering_messages(pending);
    }

    fn take_cancel_request(&self) -> bool {
        let mut input = self.turn_input.lock().unwrap();
        let requested = input.cancel_requested
            || input
                .cancellation
                .as_ref()
                .is_some_and(|turn| turn.is_cancelled());
        input.cancel_requested = false;
        if requested {
            input.pending.clear();
        }
        requested
    }

    /// Atomically close the steering window only when no input is waiting. A
    /// steer racing with turn completion is therefore either included in this
    /// turn or rejected by `steer`, never stranded in an inactive turn.
    fn turn_boundary_action(&self) -> TurnBoundaryAction {
        // A background child may have completed while the model was sampling.
        // Feed that result back and force another round before accepting a
        // seemingly-final answer from the parent.
        let received_notification = self.drain_agent_notifications_into_state();
        let pending = {
            let mut input = self.turn_input.lock().unwrap();
            if input.cancel_requested
                || input
                    .cancellation
                    .as_ref()
                    .is_some_and(|turn| turn.is_cancelled())
            {
                input.cancel_requested = false;
                input.pending.clear();
                input.active = false;
                return TurnBoundaryAction::Cancel;
            }
            if input.pending.is_empty() && !received_notification {
                input.active = false;
                input.cancel_requested = false;
                return TurnBoundaryAction::Finish;
            }
            input.pending.drain(..).collect::<Vec<_>>()
        };
        self.append_steering_messages(pending);
        TurnBoundaryAction::Continue
    }

    fn append_steering_messages(&self, messages: Vec<String>) {
        if messages.is_empty() {
            return;
        }
        let user_messages = messages.into_iter().map(Msg::user).collect::<Vec<_>>();
        for message in &user_messages {
            self.journal("user", serde_json::to_value(message).unwrap_or_default());
        }
        self.state.lock().unwrap().messages.extend(user_messages);
    }

    /// Drain notifications addressed to this agent and append them as runtime
    /// input. Returns true when at least one message was committed, allowing a
    /// turn boundary to force another model sample.
    fn drain_agent_notifications_into_state(&self) -> bool {
        use crate::subagent::NotificationKind;

        let notifications = self.notification_inbox.lock().unwrap().drain();
        let messages = notifications
            .into_iter()
            .filter(|notification| notification.to == self.name)
            .map(|notification| match notification.kind {
                NotificationKind::Completed(text) => format!(
                    "[background agent '{}' completed]
{}",
                    notification.from, text
                ),
                NotificationKind::Message(message) => format!(
                    "[message from agent '{}']
{}",
                    notification.from, message
                ),
                NotificationKind::NeedsApproval(suspension) => format!(
                    "[background agent '{}' is waiting for approval]
{}",
                    notification.from, suspension.request.summary
                ),
            })
            .collect::<Vec<_>>();
        if messages.is_empty() {
            return false;
        }
        self.append_steering_messages(messages);
        true
    }

    /// Drain any pending inbox messages into `working` as `user` messages. A
    /// no-op when no inbox is set (the foreground path) or nothing is queued.
    fn drain_inbox(&self, working: &mut AgentState) {
        let mut guard = self.inbox.lock().unwrap();
        if let Some(rx) = guard.as_mut() {
            while let Ok(message) = rx.try_recv() {
                working.messages.push(Msg::user(message));
            }
        }
    }

    /// Reset the per-task plumbing at the start of a subagent task: install the
    /// new inbox — drained at each round boundary so `send_message` delivery
    /// reaches the running subagent's transcript — or clear a stale one from a
    /// previous task, and drop any parked suspension left by an earlier
    /// suspended run on the same handle.
    pub(crate) fn reset_task_plumbing(
        &self,
        inbox: Option<tokio::sync::mpsc::UnboundedReceiver<String>>,
    ) {
        *self.inbox.lock().unwrap() = inbox;
        *self.pending.lock().unwrap() = None;
        *self.parked_suspension.lock().unwrap() = None;
        let mut turn_input = self.turn_input.lock().unwrap();
        turn_input.active = false;
        turn_input.pending.clear();
        turn_input.cancel_requested = false;
        turn_input.cancellation = None;
        turn_input.finishing = false;
    }

    /// Clone the committed state into a working copy.
    fn snapshot(&self) -> AgentState {
        self.state.lock().unwrap().clone()
    }

    /// Replace the committed state with a working copy.
    fn commit(&self, working: AgentState) {
        *self.state.lock().unwrap() = working;
    }

    /// The current thread id.
    fn thread(&self) -> String {
        self.thread_id.lock().unwrap().clone()
    }

    /// Persist the current committed snapshot to the checkpointer, if one is set.
    /// Bumps `seq` and saves under the current thread id. Legacy agents ignore
    /// save errors; durable-turn agents refuse further work with a fixed error.
    async fn save_progress_checkpoint(
        &self,
        suspension: Option<&Suspension>,
    ) -> Result<(), AgentError> {
        if self.durable_turn_checkpoints {
            self.save_checkpoint(suspension).await?;
        }
        Ok(())
    }

    async fn save_checkpoint(&self, suspension: Option<&Suspension>) -> Result<(), AgentError> {
        let result = self.try_save_checkpoint(suspension).await;
        self.checkpoint_failed
            .store(result.is_err(), Ordering::SeqCst);
        if self.durable_turn_checkpoints && result.is_err() {
            return Err(checkpoint_save_error());
        }
        Ok(())
    }

    async fn try_save_checkpoint(&self, suspension: Option<&Suspension>) -> Result<(), AgentError> {
        if let Some(cp) = &self.checkpointer {
            let mut snapshot = self.snapshot();
            if self.durable_turn_checkpoints {
                if snapshot.custom.contains_key(RECOVERY_KEY) {
                    return Err(checkpoint_save_error());
                }
                snapshot.custom.insert(
                    RECOVERY_KEY.into(),
                    serde_json::json!({
                        "version": 1,
                        "in_progress": !self.turn_input.lock().unwrap().finishing
                    }),
                );
            }
            let seq = self.seq.load(Ordering::SeqCst);
            let next = seq.checked_add(1).ok_or_else(checkpoint_save_error)?;
            self.seq
                .compare_exchange(seq, next, Ordering::SeqCst, Ordering::SeqCst)
                .map_err(|_| checkpoint_save_error())?;
            let thread = self.thread();
            cp.save(&thread, seq, &snapshot, suspension).await?;
        } else if self.durable_turn_checkpoints {
            return Err(checkpoint_save_error());
        }
        Ok(())
    }

    /// Whether the last attempted checkpoint failed. Completed effects remain
    /// in memory; applications must not claim that those effects were saved.
    pub fn checkpoint_failed(&self) -> bool {
        self.checkpoint_failed.load(Ordering::SeqCst)
    }

    pub fn durable_turn_checkpoints_enabled(&self) -> bool {
        self.durable_turn_checkpoints
    }

    pub fn recovery_info(&self) -> Option<crate::checkpoint::RecoveryInfo> {
        *self.recovery_info.lock().unwrap()
    }

    /// Append a lifecycle event to the transcript sink, if one is set.
    /// Fire-and-forget: `ts` is `0` (the framework has no clock — app-side sinks
    /// may restamp on write). A no-op when no journal is configured.
    fn journal(&self, kind: &str, data: serde_json::Value) {
        if let Some(j) = &self.journal {
            let thread = self.thread();
            j.append(&thread, &TranscriptEvent { ts: 0, kind, data });
        }
    }

    /// Restore this agent from the latest checkpoint of `thread_id`. Requires a
    /// checkpointer (else errors). Returns `Ok(false)` if the thread has no
    /// saved state; otherwise replaces the committed state, resumes the sequence
    /// counter, re-parks any persisted suspension so [`resume`](Agent::resume)
    /// can finish the interrupted turn, and returns `Ok(true)`. Switches the
    /// agent's active thread id to `thread_id`.
    pub async fn load_thread(&self, thread_id: &str) -> Result<bool, AgentError> {
        let cp = self
            .checkpointer
            .as_ref()
            .ok_or_else(|| AgentError::State("no checkpointer configured".into()))?;
        let loaded = cp.load_latest(thread_id).await.map_err(|error| {
            if self.durable_turn_checkpoints {
                checkpoint_load_error()
            } else {
                error
            }
        })?;
        let Some((seq, mut state, suspension)) = loaded else {
            return Ok(false);
        };
        let next_seq = seq.checked_add(1).ok_or_else(checkpoint_load_error)?;
        let recovery = if self.durable_turn_checkpoints {
            let unfinished_turn = match state.custom.remove(RECOVERY_KEY) {
                Some(value) => {
                    let marker: RecoveryMarker =
                        serde_json::from_value(value).map_err(|_| checkpoint_load_error())?;
                    if marker.version != 1 {
                        return Err(checkpoint_load_error());
                    }
                    marker.in_progress
                }
                None => false,
            };
            let unanswered = reconcile_recovered_calls(
                &mut state,
                suspension.as_ref().map(|s| s.request.tool_call.id.as_str()),
            );
            (unfinished_turn || unanswered > 0).then_some(crate::checkpoint::RecoveryInfo {
                unfinished_turn,
                unresolved_tool_calls: unanswered,
            })
        } else {
            None
        };
        *self.thread_id.lock().unwrap() = thread_id.to_string();
        self.clear_pending_input();
        self.commit(state);
        self.seq.store(next_seq, Ordering::SeqCst);
        self.checkpoint_failed.store(false, Ordering::SeqCst);
        *self.recovery_info.lock().unwrap() = recovery;
        // Re-park a persisted suspension so the app can resume the interrupted
        // turn. The remaining calls of the suspended round are not persisted, so
        // resume continues from the model↔tool loop after applying the decision.
        *self.pending.lock().unwrap() = suspension.as_ref().map(|s| PendingResume {
            call: s.request.tool_call.clone(),
            remaining: vec![],
            round: 0,
        });
        // Copy the re-parked suspension into `parked_suspension` so the app can
        // surface the pending approval immediately after loading the thread.
        *self.parked_suspension.lock().unwrap() = suspension;
        self.journal("resumed", serde_json::json!({ "thread_id": thread_id }));
        Ok(true)
    }

    /// The suspension of the currently parked turn, if any. Set at every park
    /// (including a re-park after [`load_thread`](Agent::load_thread)) and
    /// cleared when [`resume`](Agent::resume) consumes the parked turn. The
    /// app uses this after loading a thread to surface a pending approval.
    pub fn pending_suspension(&self) -> Option<Suspension> {
        self.parked_suspension.lock().unwrap().clone()
    }

    /// Switch the active thread id and reset the checkpoint sequence to 0. Used
    /// by the app's `/clear` to rotate to a fresh thread (the old one stays on
    /// disk, resumable). Unlike [`load_thread`](Agent::load_thread) this does
    /// not touch state — pair it with a state reset when starting fresh.
    pub fn set_thread_id(&self, id: &str) {
        *self.thread_id.lock().unwrap() = id.to_string();
        self.seq.store(0, Ordering::SeqCst);
        self.checkpoint_failed.store(false, Ordering::SeqCst);
        *self.recovery_info.lock().unwrap() = None;
    }
}

const RECOVERY_KEY: &str = "_craftsman_durable_turn_v1";

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RecoveryMarker {
    version: u64,
    in_progress: bool,
}

fn checkpoint_save_error() -> AgentError {
    AgentError::State("Session checkpoint could not be saved. Further work was stopped; completed operations may have effects that were not saved. Inspect the workspace before retrying.".into())
}

fn checkpoint_load_error() -> AgentError {
    AgentError::State("Session checkpoint could not be restored. The current session was kept; no recovered operations were replayed.".into())
}

// Reconcile each assistant/tool group separately: ids can be reused in later
// rounds. Put unknown results BEFORE the next user/assistant message so a new
// turn never accidentally supplies results to an earlier interrupted round.
fn reconcile_recovered_calls(state: &mut AgentState, parked_id: Option<&str>) -> usize {
    let mut pending: Vec<String> = Vec::new();
    let mut recovered = 0;
    let original = std::mem::take(&mut state.messages);
    for message in original {
        match &message {
            Msg::Tool { tool_call_id, .. } => {
                if let Some(index) = pending.iter().position(|id| id == tool_call_id) {
                    pending.remove(index);
                }
            }
            _ => {
                recovered += append_unknown_results(&mut state.messages, &mut pending, parked_id);
            }
        }
        if let Msg::Assistant { tool_calls, .. } = &message {
            pending.extend(tool_calls.iter().map(|call| call.id.clone()));
        }
        state.messages.push(message);
    }
    recovered + append_unknown_results(&mut state.messages, &mut pending, parked_id)
}

fn append_unknown_results(
    messages: &mut Vec<Msg>,
    pending: &mut Vec<String>,
    parked_id: Option<&str>,
) -> usize {
    let mut count = 0;
    for id in pending.drain(..) {
        if parked_id == Some(id.as_str()) {
            continue;
        }
        messages.push(Msg::Tool {
            tool_call_id: id,
            content: "Completion was not recorded before the session was interrupted. This operation may have had effects. Inspect the workspace before retrying it.".into(),
            is_error: true,
        });
        count += 1;
    }
    count
}

/// Dispatch commit: the tool may have mutated state through TWO channels —
/// `ctx.state` (the working clone: todos, custom, etc.) and, when the default
/// state-backed [`StateFs`] is in use, `ctx.backend` (which shares the committed
/// `Arc` and mutates `committed.files` directly mid-dispatch). A plain overwrite
/// commit clobbers the backend channel. Merge instead:
/// - scalar/collection fields other than `.files`: take working's (tool's view)
/// - `.files`: keep committed's current entries (backend writes), then apply
///   the tool's `ctx.state` file deltas (working vs the `t0` snapshot): upserts
///   for added/changed keys, removals for keys the tool deleted.
///
/// For a `RealFs`-style backend that never touches `.files`, the file merge is
/// a no-op (committed.files == t0.files == working.files), so behavior is
/// identical to a blanket commit.
fn merge_dispatch_result(committed: &mut AgentState, t0: &AgentState, working: AgentState) {
    let AgentState {
        messages,
        todos,
        files: working_files,
        system_extras,
        custom,
        last_usage,
        last_usage_msg_count,
    } = working;

    // Non-file fields: the tool's working view wins wholesale.
    committed.messages = messages;
    committed.todos = todos;
    committed.system_extras = system_extras;
    committed.custom = custom;
    committed.last_usage = last_usage;
    committed.last_usage_msg_count = last_usage_msg_count;

    // Files: start from committed (which already reflects any backend writes made
    // mid-dispatch), then replay the tool's ctx.state file deltas on top.
    // Upserts / changes: any key whose working value differs from t0.
    for (k, v) in &working_files {
        if t0.files.get(k) != Some(v) {
            committed.files.insert(k.clone(), v.clone());
        }
    }
    // Deletions: keys present at t0 but removed from the working copy.
    for k in t0.files.keys() {
        if !working_files.contains_key(k) {
            committed.files.remove(k);
        }
    }
}

/// Constrains an inline closure to the higher-ranked [`ToolHandler`] bound.
/// A bare closure can't infer a return lifetime tied to its `ctx` argument;
/// funnelling it through this identity fn pins the `for<'c>` signature.
fn as_tool_handler<F>(f: F) -> F
where
    F: for<'c, 'd> Fn(ToolCall, &'c mut ToolCtx<'d>) -> BoxedToolFuture<'c> + Send + Sync,
{
    f
}

/// Builder for [`Agent`]. `max_rounds` defaults to 50; state defaults to a
/// fresh in-memory `AgentState`; the backend defaults to a [`StateFs`] sharing
/// the committed state.
pub struct AgentBuilder {
    model: Arc<dyn ModelProvider>,
    name: String,
    tools: Vec<Arc<dyn Tool>>,
    middleware: Vec<Arc<dyn AgentMiddleware>>,
    backend: Option<Arc<dyn Backend>>,
    system_prompt: String,
    max_rounds: usize,
    interrupt_model_on_cancel: bool,
    cancellation_session: Option<crate::SessionCancellation>,
    state: Option<Arc<Mutex<AgentState>>>,
    registry: Option<Arc<crate::subagent::AgentRegistry>>,
    notifications: Option<crate::subagent::NotificationQueue>,
    checkpointer: Option<Arc<dyn Checkpointer>>,
    durable_turn_checkpoints: bool,
    journal: Option<Arc<dyn TranscriptSink>>,
    thread_id: String,
    harness_profile: Option<crate::profiles::HarnessProfile>,
}

impl AgentBuilder {
    fn new(model: Arc<dyn ModelProvider>) -> Self {
        Self {
            model,
            name: "agent".into(),
            tools: vec![],
            middleware: vec![],
            backend: None,
            system_prompt: String::new(),
            max_rounds: 50,
            interrupt_model_on_cancel: false,
            cancellation_session: None,
            state: None,
            registry: None,
            notifications: None,
            checkpointer: None,
            durable_turn_checkpoints: false,
            journal: None,
            thread_id: "default".into(),
            harness_profile: None,
        }
    }

    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    pub fn system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = prompt.into();
        self
    }

    pub fn tool(mut self, tool: Arc<dyn Tool>) -> Self {
        self.tools.push(tool);
        self
    }

    pub fn middleware(mut self, mw: Arc<dyn AgentMiddleware>) -> Self {
        self.middleware.push(mw);
        self
    }

    pub fn backend(mut self, backend: Arc<dyn Backend>) -> Self {
        self.backend = Some(backend);
        self
    }

    pub fn max_rounds(mut self, n: usize) -> Self {
        self.max_rounds = n;
        self
    }

    /// Drop pending model sampling when this agent receives `request_cancel`.
    /// Default false preserves cooperative framework behavior. Opt in only
    /// when the provider and all `around_model` middleware are safe to drop:
    /// they must own their I/O and release it without detached work or required
    /// async cleanup. This creates an opted-in turn scope; cancellation-aware
    /// tools finish their owned cleanup, compaction models observe the scope,
    /// and explicitly opted-in foreground children inherit it. It does not
    /// force-drop mutation hooks or imply remote generation/billing stopped.
    pub fn interrupt_model_on_cancel(mut self, enabled: bool) -> Self {
        self.interrupt_model_on_cancel = enabled;
        self
    }

    /// Attach the session shutdown owner to opted-in turns. Other agents ignore
    /// it, and foreground children inherit only when explicitly opted in.
    pub fn cancellation_session(mut self, session: crate::SessionCancellation) -> Self {
        self.cancellation_session = Some(session);
        self
    }

    pub fn state(mut self, state: Arc<Mutex<AgentState>>) -> Self {
        self.state = Some(state);
        self
    }

    /// Inject a shared [`AgentRegistry`] so spawned subagents are tracked in a
    /// registry the app also holds. Defaults to a fresh private registry.
    pub fn registry(mut self, registry: Arc<crate::subagent::AgentRegistry>) -> Self {
        self.registry = Some(registry);
        self
    }

    /// Inject a shared [`NotificationQueue`] so `send_message` and background
    /// completion notifications reach both this agent's internal subscription
    /// and any application receivers. Defaults to a fresh private bus.
    pub fn notifications(mut self, notifications: crate::subagent::NotificationQueue) -> Self {
        self.notifications = Some(notifications);
        self
    }

    /// Install a durable [`Checkpointer`]. When set, the committed state is saved
    /// at every round commit, at a suspension park, and at turn end. Defaults to
    /// no persistence.
    pub fn checkpointer(mut self, checkpointer: Arc<dyn Checkpointer>) -> Self {
        self.checkpointer = Some(checkpointer);
        self
    }

    /// Require successful durable saves before sampling/dispatch and after each
    /// completed tool. Defaults false, preserving legacy persistence behavior.
    /// A missing store or save failure stops further work with a fixed error.
    /// Reopening preserves recorded results and marks unresolved calls unknown;
    /// it never automatically replays operations. This is not effect rollback.
    pub fn durable_turn_checkpoints(mut self, enabled: bool) -> Self {
        self.durable_turn_checkpoints = enabled;
        self
    }

    /// Install an append-only [`TranscriptSink`] for lifecycle journaling.
    /// Defaults to no journal.
    pub fn journal(mut self, journal: Arc<dyn TranscriptSink>) -> Self {
        self.journal = Some(journal);
        self
    }

    /// Set the thread id used for checkpoint/journal writes. Defaults to
    /// `"default"`; the app always passes one.
    pub fn thread_id(mut self, thread_id: impl Into<String>) -> Self {
        self.thread_id = thread_id.into();
        self
    }

    /// Apply final tool visibility and description rewrites after explicit and
    /// middleware-contributed tools have been collected.
    pub fn harness_profile(mut self, profile: crate::profiles::HarnessProfile) -> Self {
        self.harness_profile = Some(profile);
        self
    }

    pub fn build(self) -> Agent {
        let state = self
            .state
            .unwrap_or_else(|| Arc::new(Mutex::new(AgentState::default())));
        let backend = self
            .backend
            .unwrap_or_else(|| Arc::new(StateFs::new(state.clone())));

        let harness_profile = self.harness_profile.clone();
        // Explicit tools first, then middleware-contributed tools.
        let mut tools: HashMap<String, Arc<dyn Tool>> = HashMap::new();
        for t in self.tools {
            tools.insert(t.def().name, t);
        }
        for m in &self.middleware {
            for (def, tool) in m.tools() {
                tools.insert(def.name, tool);
            }
        }
        if let Some(profile) = &harness_profile {
            tools = tools
                .into_values()
                .filter_map(|tool| crate::profiles::apply_tool_profile(tool, profile))
                .map(|tool| (tool.def().name, tool))
                .collect();
        }

        let registry = self
            .registry
            .unwrap_or_else(|| Arc::new(crate::subagent::AgentRegistry::default()));
        // Default notifications: a fresh private bus. Every agent subscribes
        // internally; applications can supply a shared bus and retain their own
        // independent display receiver.
        let notifications = self.notifications.unwrap_or_else(|| {
            let (queue, _rx) = crate::subagent::notification_channel();
            queue
        });

        let notification_inbox = Arc::new(Mutex::new(notifications.subscribe()));

        Agent {
            state,
            name: self.name,
            model: self.model,
            tools,
            chain: MiddlewareChain::new(self.middleware),
            backend,
            system_prompt: self.system_prompt,
            max_rounds: self.max_rounds,
            interrupt_model_on_cancel: self.interrupt_model_on_cancel,
            cancellation_session: self.cancellation_session,
            registry,
            notifications,
            notification_inbox,
            pending: Arc::new(Mutex::new(None)),
            parked_suspension: Arc::new(Mutex::new(None)),
            inbox: Arc::new(Mutex::new(None)),
            turn_input: Arc::new(Mutex::new(TurnInputState::default())),
            model_cancel: Arc::new(tokio::sync::Notify::new()),
            checkpointer: self.checkpointer,
            durable_turn_checkpoints: self.durable_turn_checkpoints,
            checkpoint_failed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            recovery_info: Arc::new(Mutex::new(None)),
            journal: self.journal,
            thread_id: Arc::new(Mutex::new(self.thread_id)),
            seq: Arc::new(AtomicU64::new(0)),
        }
    }
}

#[cfg(test)]
#[path = "agent_recovery_tests.rs"]
mod recovery_tests;

#[cfg(test)]
#[path = "agent_owned_cancellation_tests.rs"]
mod owned_cancellation_tests;

#[cfg(test)]
#[path = "agent_model_cancellation_tests.rs"]
mod model_cancellation_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ModelProvider, ModelRequest, ModelResponse, ToolDef, Usage};
    use crate::state::{Msg, ToolCall};
    use crate::testing::ScriptedModel;
    use crate::tool::{Tool, ToolCtx, ToolOutcome};
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
    use std::sync::{Arc, Mutex};
    use tokio::sync::Notify;

    struct Echo;
    #[async_trait]
    impl Tool for Echo {
        fn def(&self) -> ToolDef {
            ToolDef {
                name: "echo".into(),
                description: "echo args.text".into(),
                parameters: serde_json::json!({"type":"object","properties":{"text":{"type":"string"}}}),
            }
        }
        async fn call(
            &self,
            args: serde_json::Value,
            _ctx: &mut ToolCtx<'_>,
        ) -> Result<ToolOutcome, crate::AgentError> {
            Ok(ToolOutcome::ok(
                args["text"].as_str().unwrap_or("").to_string(),
            ))
        }
    }

    fn call(name: &str, id: &str, args: serde_json::Value) -> ToolCall {
        ToolCall {
            id: id.into(),
            name: name.into(),
            args,
        }
    }

    /// Unwrap a completed turn, panicking if it suspended.
    fn done(r: TurnResult) -> TurnOutput {
        match r {
            TurnResult::Done(o) => o,
            TurnResult::Suspended(_) => panic!("expected Done, got Suspended"),
        }
    }

    struct SteeringModel {
        requests: Mutex<Vec<ModelRequest>>,
        calls: AtomicUsize,
        first_started: Notify,
        release_first: Notify,
    }

    impl SteeringModel {
        fn new() -> Self {
            Self {
                requests: Mutex::new(Vec::new()),
                calls: AtomicUsize::new(0),
                first_started: Notify::new(),
                release_first: Notify::new(),
            }
        }
    }

    #[async_trait]
    impl ModelProvider for SteeringModel {
        async fn complete(&self, req: ModelRequest) -> Result<ModelResponse, AgentError> {
            self.requests.lock().unwrap().push(req);
            let call = self.calls.fetch_add(1, AtomicOrdering::SeqCst);
            if call == 0 {
                self.first_started.notify_one();
                self.release_first.notified().await;
                Ok(ModelResponse {
                    message: Msg::assistant("initial answer"),
                    usage: Usage::default(),
                })
            } else {
                Ok(ModelResponse {
                    message: Msg::assistant("revised answer"),
                    usage: Usage::default(),
                })
            }
        }
    }

    #[tokio::test]
    async fn steer_during_model_call_forces_another_sample_in_the_same_turn() {
        let model = Arc::new(SteeringModel::new());
        let agent = Agent::builder(model.clone()).build();
        let running_agent = agent.clone();
        let task = tokio::spawn(async move { running_agent.run_turn("initial request").await });

        model.first_started.notified().await;
        assert!(agent.is_turn_active());
        assert!(agent.steer("new constraint"));
        model.release_first.notify_one();

        let result = task.await.unwrap().unwrap();
        assert_eq!(done(result).final_text(), "revised answer");
        assert!(!agent.is_turn_active());
        assert!(!agent.steer("too late"));

        let requests = model.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert!(matches!(
            requests[1].messages.as_slice(),
            [
                Msg::User { content: initial },
                Msg::Assistant { content: first, .. },
                Msg::User { content: steer },
            ] if initial == "initial request"
                && first == "initial answer"
                && steer == "new constraint"
        ));
    }

    #[tokio::test]
    async fn cancel_during_model_call_stops_before_tools_and_balances_tool_calls() {
        struct BlockingToolModel {
            started: Notify,
            release: Notify,
        }
        #[async_trait]
        impl ModelProvider for BlockingToolModel {
            async fn complete(&self, _req: ModelRequest) -> Result<ModelResponse, AgentError> {
                self.started.notify_one();
                self.release.notified().await;
                Ok(ModelResponse {
                    message: Msg::Assistant {
                        content: "about to run".into(),
                        tool_calls: vec![call(
                            "echo",
                            "cancelled-call",
                            serde_json::json!({"text": "must not execute"}),
                        )],
                    },
                    usage: Usage::default(),
                })
            }
        }

        let model = Arc::new(BlockingToolModel {
            started: Notify::new(),
            release: Notify::new(),
        });
        let agent = Agent::builder(model.clone()).tool(Arc::new(Echo)).build();
        let running_agent = agent.clone();
        let task = tokio::spawn(async move { running_agent.run_turn("start").await });

        model.started.notified().await;
        assert!(agent.request_cancel());
        model.release.notify_one();

        let result = done(task.await.unwrap().unwrap());
        assert_eq!(result.final_text(), "Stopped by the user.");
        assert!(!agent.is_turn_active());
        assert!(!agent.request_cancel());

        let state = agent.state.lock().unwrap();
        assert!(state.messages.iter().any(|message| {
            matches!(
                message,
                Msg::Tool {
                    tool_call_id,
                    content,
                    is_error: true,
                } if tool_call_id == "cancelled-call"
                    && content == "Tool call cancelled by the user."
            )
        }));
        assert!(!state.messages.iter().any(|message| {
            matches!(message, Msg::Tool { content, .. } if content == "must not execute")
        }));
    }

    #[tokio::test]
    async fn addressed_notification_reaches_agent_context_and_ui_subscription() {
        let model = Arc::new(ScriptedModel::new(vec![Msg::assistant("acknowledged")]));
        let (queue, mut ui_receiver) = crate::subagent::notification_channel();
        let agent = Agent::builder(model.clone())
            .name("main")
            .notifications(queue.clone())
            .build();

        queue.push(crate::subagent::Notification {
            from: "researcher#0".into(),
            to: "main".into(),
            kind: crate::subagent::NotificationKind::Completed("found the answer".into()),
        });

        let result = done(agent.run_turn("continue").await.unwrap());
        assert_eq!(result.final_text(), "acknowledged");
        let requests = model.requests.lock().unwrap();
        assert!(requests[0].messages.iter().any(|message| {
            matches!(message, Msg::User { content } if content.contains("researcher#0") && content.contains("completed") && content.contains("found the answer"))
        }));
        let displayed = ui_receiver.drain();
        assert_eq!(
            displayed.len(),
            1,
            "UI subscription must receive the same event"
        );
    }

    #[tokio::test]
    async fn plain_answer_ends_turn() {
        let model = Arc::new(ScriptedModel::new(vec![Msg::assistant("done")]));
        let agent = Agent::builder(model).build();
        let out = done(agent.run_turn("hi").await.unwrap());
        assert_eq!(out.final_text(), "done");
        let st = agent.state.lock().unwrap();
        assert_eq!(st.messages.len(), 2); // user + assistant
    }

    #[tokio::test]
    async fn tool_round_trip_feeds_result_back() {
        let model = Arc::new(ScriptedModel::new(vec![
            Msg::Assistant {
                content: String::new(),
                tool_calls: vec![call("echo", "c1", serde_json::json!({"text":"ping"}))],
            },
            Msg::assistant("got ping"),
        ]));
        let agent = Agent::builder(model).tool(Arc::new(Echo)).build();
        let out = done(agent.run_turn("go").await.unwrap());
        assert_eq!(out.final_text(), "got ping");
        let st = agent.state.lock().unwrap();
        // user, assistant(tool_call), tool result, assistant
        assert!(matches!(st.messages[2], Msg::Tool { ref content, .. } if content == "ping"));
    }

    #[tokio::test]
    async fn unknown_tool_becomes_is_error_result_not_crash() {
        let model = Arc::new(ScriptedModel::new(vec![
            Msg::Assistant {
                content: String::new(),
                tool_calls: vec![call("nope", "c1", serde_json::json!({}))],
            },
            Msg::assistant("recovered"),
        ]));
        let agent = Agent::builder(model).build();
        let out = done(agent.run_turn("go").await.unwrap());
        assert_eq!(out.final_text(), "recovered");
        let st = agent.state.lock().unwrap();
        assert!(matches!(st.messages[2], Msg::Tool { is_error: true, .. }));
    }

    #[tokio::test]
    async fn max_rounds_guard_fires() {
        let script: Vec<Msg> = (0..60)
            .map(|i| Msg::Assistant {
                content: String::new(),
                tool_calls: vec![call(
                    "echo",
                    &format!("c{i}"),
                    serde_json::json!({"text":"x"}),
                )],
            })
            .collect();
        let model = Arc::new(ScriptedModel::new(script));
        let agent = Agent::builder(model)
            .tool(Arc::new(Echo))
            .max_rounds(5)
            .build();
        let err = agent.run_turn("go").await.unwrap_err();
        assert!(matches!(err, crate::AgentError::MaxRounds(5)));
    }

    #[tokio::test]
    async fn backend_file_write_and_state_mutation_both_survive_dispatch() {
        struct WriteBoth;
        #[async_trait]
        impl Tool for WriteBoth {
            fn def(&self) -> ToolDef {
                ToolDef {
                    name: "write_both".into(),
                    description: "".into(),
                    parameters: serde_json::json!({"type":"object"}),
                }
            }
            async fn call(
                &self,
                _args: serde_json::Value,
                ctx: &mut ToolCtx<'_>,
            ) -> Result<ToolOutcome, crate::AgentError> {
                ctx.backend.write("new.txt", "hello").await?;
                ctx.state.todos.push(crate::state::Todo {
                    content: "did it".into(),
                    status: crate::state::TodoStatus::Completed,
                });
                Ok(ToolOutcome::ok("done"))
            }
        }
        let model = Arc::new(ScriptedModel::new(vec![
            Msg::Assistant {
                content: String::new(),
                tool_calls: vec![call("write_both", "c1", serde_json::json!({}))],
            },
            Msg::assistant("finished"),
        ]));
        let agent = Agent::builder(model).tool(Arc::new(WriteBoth)).build();
        agent.run_turn("go").await.unwrap();
        let st = agent.state.lock().unwrap();
        assert_eq!(
            st.files.get("new.txt").map(|f| f.content.as_str()),
            Some("hello"),
            "backend write must survive commit"
        );
        assert_eq!(st.todos.len(), 1, "state mutation must survive commit");
    }

    #[tokio::test]
    async fn remodel_strips_dangling_tool_calls_before_resend() {
        use crate::middleware::Flow;
        use std::sync::Mutex as StdMutex;
        struct RemodelOnce {
            fired: StdMutex<bool>,
        }
        #[async_trait]
        impl crate::middleware::AgentMiddleware for RemodelOnce {
            fn name(&self) -> &str {
                "remodel_once"
            }
            async fn review_model(
                &self,
                st: &mut crate::state::AgentState,
            ) -> Result<Flow, crate::AgentError> {
                let has_calls = matches!(st.messages.last(), Some(Msg::Assistant { tool_calls, .. }) if !tool_calls.is_empty());
                let mut fired = self.fired.lock().unwrap();
                if has_calls && !*fired {
                    *fired = true;
                    return Ok(Flow::Resample);
                }
                Ok(Flow::Proceed)
            }
        }
        let model = Arc::new(ScriptedModel::new(vec![
            Msg::Assistant {
                content: String::new(),
                tool_calls: vec![call("echo", "c1", serde_json::json!({"text":"x"}))],
            },
            Msg::assistant("ok"),
        ]));
        let agent = Agent::builder(model.clone())
            .tool(Arc::new(Echo))
            .middleware(Arc::new(RemodelOnce {
                fired: StdMutex::new(false),
            }))
            .build();
        let out = done(agent.run_turn("go").await.unwrap());
        assert_eq!(out.final_text(), "ok");
        let reqs = model.requests.lock().unwrap();
        // The second ModelRequest must not carry an assistant message with tool_calls.
        let has_dangling = reqs[1]
            .messages
            .iter()
            .any(|m| matches!(m, Msg::Assistant { tool_calls, .. } if !tool_calls.is_empty()));
        assert!(
            !has_dangling,
            "second request must not contain assistant-with-tool_calls"
        );
    }

    #[tokio::test]
    async fn review_model_arg_repairs_reach_dispatch() {
        // PatchToolCallsMiddleware repairs string-encoded args in review_model.
        // The loop must dispatch the REPAIRED call, not a pre-hook snapshot —
        // otherwise the builtin is silently nullified and the persisted
        // transcript diverges from what actually executed.
        let model = Arc::new(ScriptedModel::new(vec![
            Msg::Assistant {
                content: String::new(),
                tool_calls: vec![call(
                    "echo",
                    "c1",
                    serde_json::Value::String("{\"text\":\"ping\"}".into()),
                )],
            },
            Msg::assistant("done"),
        ]));
        let agent = Agent::builder(model)
            .tool(Arc::new(Echo))
            .middleware(Arc::new(crate::builtin::PatchToolCallsMiddleware))
            .build();
        let out = done(agent.run_turn("go").await.unwrap());
        assert_eq!(out.final_text(), "done");
        let st = agent.state.lock().unwrap();
        // Echo returns args["text"]: "" for the raw string args, "ping" once
        // the repaired object args reach dispatch.
        assert!(
            st.messages
                .iter()
                .any(|m| matches!(m, Msg::Tool { content, .. } if content == "ping")),
            "dispatched call must carry the review_model-repaired args"
        );
    }

    #[tokio::test]
    async fn decision_end_strips_dangling_tool_calls() {
        use crate::middleware::Flow;
        // A middleware Ends the turn while the assistant message still carries
        // tool_calls. The committed transcript must not keep the dangling
        // tool_calls (OpenAI-compatible providers 400 on the next turn).
        struct EndNow;
        #[async_trait]
        impl crate::middleware::AgentMiddleware for EndNow {
            fn name(&self) -> &str {
                "end_now"
            }
            async fn review_model(
                &self,
                _st: &mut crate::state::AgentState,
            ) -> Result<Flow, crate::AgentError> {
                Ok(Flow::Finish)
            }
        }
        let model = Arc::new(ScriptedModel::new(vec![Msg::Assistant {
            content: "stopping".into(),
            tool_calls: vec![call("echo", "c1", serde_json::json!({"text":"x"}))],
        }]));
        let agent = Agent::builder(model)
            .tool(Arc::new(Echo))
            .middleware(Arc::new(EndNow))
            .build();
        agent.run_turn("go").await.unwrap();
        let st = agent.state.lock().unwrap();
        assert!(
            !st.messages
                .iter()
                .any(|m| matches!(m, Msg::Assistant { tool_calls, .. } if !tool_calls.is_empty())),
            "End must not leave an assistant message with unanswered tool_calls"
        );
    }

    #[tokio::test]
    async fn turn_end_fires_once_per_turn() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        struct Counter;
        #[async_trait]
        impl crate::middleware::AgentMiddleware for Counter {
            fn name(&self) -> &str {
                "counter"
            }
            async fn turn_end(
                &self,
                _st: &mut crate::state::AgentState,
            ) -> Result<(), crate::AgentError> {
                CALLS.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        }
        // A tool round then a plain answer: turn_end must fire exactly once
        // (at the terminal exit), not per round.
        let model = Arc::new(ScriptedModel::new(vec![
            Msg::Assistant {
                content: String::new(),
                tool_calls: vec![call("echo", "c1", serde_json::json!({"text":"x"}))],
            },
            Msg::assistant("done"),
        ]));
        let agent = Agent::builder(model)
            .tool(Arc::new(Echo))
            .middleware(Arc::new(Counter))
            .build();
        agent.run_turn("go").await.unwrap();
        assert_eq!(CALLS.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn system_extras_are_appended_each_round() {
        struct Extra;
        #[async_trait]
        impl crate::middleware::AgentMiddleware for Extra {
            fn name(&self) -> &str {
                "extra"
            }
            async fn prepare_model(
                &self,
                st: &mut crate::state::AgentState,
            ) -> Result<(), crate::AgentError> {
                st.system_extras.push("EXTRA SECTION".into());
                Ok(())
            }
        }
        let model = Arc::new(ScriptedModel::new(vec![Msg::assistant("k")]));
        let agent = Agent::builder(model.clone())
            .system_prompt("BASE")
            .middleware(Arc::new(Extra))
            .build();
        agent.run_turn("q").await.unwrap();
        let reqs = model.requests.lock().unwrap();
        assert!(reqs[0].system.starts_with("BASE"));
        assert!(reqs[0].system.contains("EXTRA SECTION"));
    }

    // ---- suspend / resume --------------------------------------------------

    struct SuspendGate;
    #[async_trait]
    impl crate::middleware::AgentMiddleware for SuspendGate {
        fn name(&self) -> &str {
            "gate"
        }
        async fn around_tool(
            &self,
            call: crate::state::ToolCall,
            ctx: &mut ToolCtx<'_>,
            next: crate::middleware::ToolNext<'_>,
        ) -> Result<ToolOutcome, crate::AgentError> {
            if call.name == "echo" {
                return Err(crate::AgentError::Suspended(Box::new(
                    crate::interrupt::Suspension {
                        frames: vec![crate::interrupt::SuspensionFrame {
                            agent_name: "main".into(),
                        }],
                        request: crate::interrupt::HitlRequest {
                            tool_call: call,
                            summary: "gate".into(),
                        },
                    },
                )));
            }
            next.run(call, ctx).await
        }
    }

    #[tokio::test]
    async fn gated_call_suspends_then_deny_resumes() {
        let model = Arc::new(ScriptedModel::new(vec![
            Msg::Assistant {
                content: String::new(),
                tool_calls: vec![call("echo", "c1", serde_json::json!({"text":"x"}))],
            },
            Msg::assistant("after deny"),
        ]));
        let agent = Agent::builder(model)
            .tool(Arc::new(Echo))
            .middleware(Arc::new(SuspendGate))
            .build();
        match agent.run_turn("go").await.unwrap() {
            TurnResult::Suspended(s) => assert_eq!(s.request.tool_call.id, "c1"),
            _ => panic!("expected suspension"),
        }
        let out = agent
            .resume(
                crate::interrupt::HitlAction::Deny {
                    feedback: "no".into(),
                },
                "Denied.",
            )
            .await
            .unwrap();
        match out {
            TurnResult::Done(o) => assert_eq!(o.final_text(), "after deny"),
            _ => panic!(),
        }
        let st = agent.state.lock().unwrap();
        assert!(st.messages.iter().any(
            |m| matches!(m, Msg::Tool{is_error:true, content, ..} if content.contains("Denied"))
        ));
    }

    #[tokio::test]
    async fn approve_edited_replaces_args() {
        let model = Arc::new(ScriptedModel::new(vec![
            Msg::Assistant {
                content: String::new(),
                tool_calls: vec![call("echo", "c1", serde_json::json!({"text":"orig"}))],
            },
            Msg::assistant("done"),
        ]));
        let agent = Agent::builder(model)
            .tool(Arc::new(Echo))
            .middleware(Arc::new(SuspendGate))
            .build();
        agent.run_turn("go").await.unwrap();
        agent
            .resume(
                crate::interrupt::HitlAction::ApproveEdited {
                    args: serde_json::json!({"text":"edited"}),
                },
                "Denied.",
            )
            .await
            .unwrap();
        let st = agent.state.lock().unwrap();
        assert!(
            st.messages
                .iter()
                .any(|m| matches!(m, Msg::Tool{content, ..} if content == "edited"))
        );
    }

    // ---- checkpoint / journal ----------------------------------------------

    use crate::testing::MemCheckpointer;

    /// Recording TranscriptSink capturing (kind) of every appended event.
    #[derive(Default)]
    struct RecordSink {
        events: Mutex<Vec<(String, String, serde_json::Value)>>,
    }
    impl crate::checkpoint::TranscriptSink for RecordSink {
        fn append(&self, thread_id: &str, event: &crate::checkpoint::TranscriptEvent<'_>) {
            self.events.lock().unwrap().push((
                thread_id.to_string(),
                event.kind.to_string(),
                event.data.clone(),
            ));
        }
    }

    #[tokio::test]
    async fn checkpoint_saved_at_each_round_and_turn_end() {
        // Tool round then plain answer: 2 assistant-round saves + 1 finish save.
        let model = Arc::new(ScriptedModel::new(vec![
            Msg::Assistant {
                content: String::new(),
                tool_calls: vec![call("echo", "c1", serde_json::json!({"text":"x"}))],
            },
            Msg::assistant("done"),
        ]));
        let cp = Arc::new(MemCheckpointer::default());
        let agent = Agent::builder(model)
            .tool(Arc::new(Echo))
            .checkpointer(cp.clone())
            .thread_id("t1")
            .build();
        agent.run_turn("go").await.unwrap();
        let saves = cp.saves.lock().unwrap();
        // round 0 commit, round 1 commit, finish = 3 saves.
        assert_eq!(saves.len(), 3);
        // Monotonic seq under the configured thread id.
        assert_eq!(saves[0].0, "t1");
        assert_eq!(saves.iter().map(|s| s.1).collect::<Vec<_>>(), vec![0, 1, 2]);
        // Final save carries no suspension and the completed transcript.
        assert!(saves[2].3.is_none());
        assert!(
            saves[2]
                .2
                .messages
                .iter()
                .any(|m| matches!(m, Msg::Assistant{content,..} if content == "done"))
        );
    }

    #[tokio::test]
    async fn suspension_is_checkpointed_and_restorable() {
        let model = Arc::new(ScriptedModel::new(vec![
            Msg::Assistant {
                content: String::new(),
                tool_calls: vec![call("echo", "c1", serde_json::json!({"text":"x"}))],
            },
            Msg::assistant("after"),
        ]));
        let cp = Arc::new(MemCheckpointer::default());
        let agent = Agent::builder(model)
            .tool(Arc::new(Echo))
            .middleware(Arc::new(SuspendGate))
            .checkpointer(cp.clone())
            .thread_id("t2")
            .build();
        match agent.run_turn("go").await.unwrap() {
            TurnResult::Suspended(_) => {}
            _ => panic!("expected suspension"),
        }
        // The latest save must carry the suspension.
        let (_seq, _st, susp) = cp.load_latest("t2").await.unwrap().unwrap();
        assert_eq!(susp.as_ref().unwrap().request.tool_call.id, "c1");

        // A fresh agent restores the thread and re-parks the suspension so it can
        // be resumed to completion.
        let model2 = Arc::new(ScriptedModel::new(vec![Msg::assistant("after")]));
        let agent2 = Agent::builder(model2)
            .tool(Arc::new(Echo))
            .middleware(Arc::new(SuspendGate))
            .checkpointer(cp.clone())
            .build();
        assert!(agent2.load_thread("t2").await.unwrap());
        let out = agent2
            .resume(
                crate::interrupt::HitlAction::Deny {
                    feedback: String::new(),
                },
                "Denied.",
            )
            .await
            .unwrap();
        match out {
            TurnResult::Done(o) => assert_eq!(o.final_text(), "after"),
            _ => panic!(),
        }
    }

    #[tokio::test]
    async fn load_thread_missing_returns_false_and_requires_checkpointer() {
        // No checkpointer -> error.
        let model = Arc::new(ScriptedModel::new(vec![Msg::assistant("x")]));
        let agent = Agent::builder(model).build();
        assert!(agent.load_thread("nope").await.is_err());

        // Checkpointer but unknown thread -> Ok(false).
        let model = Arc::new(ScriptedModel::new(vec![Msg::assistant("x")]));
        let cp = Arc::new(MemCheckpointer::default());
        let agent = Agent::builder(model).checkpointer(cp).build();
        assert!(!agent.load_thread("nope").await.unwrap());
    }

    #[tokio::test]
    async fn journal_records_lifecycle_events() {
        let model = Arc::new(ScriptedModel::new(vec![
            Msg::Assistant {
                content: String::new(),
                tool_calls: vec![call("echo", "c1", serde_json::json!({"text":"x"}))],
            },
            Msg::assistant("done"),
        ]));
        let sink = Arc::new(RecordSink::default());
        let agent = Agent::builder(model)
            .tool(Arc::new(Echo))
            .journal(sink.clone())
            .thread_id("tj")
            .build();
        agent.run_turn("go").await.unwrap();
        let events = sink.events.lock().unwrap();
        let kinds: Vec<&str> = events.iter().map(|(_, k, _)| k.as_str()).collect();
        // user, assistant(round0), tool, assistant(round1)
        assert_eq!(kinds, vec!["user", "assistant", "tool", "assistant"]);
        assert!(events.iter().all(|(t, ..)| t == "tj"));
    }

    #[tokio::test]
    async fn pending_suspension_set_at_park_and_cleared_at_resume() {
        let model = Arc::new(ScriptedModel::new(vec![
            Msg::Assistant {
                content: String::new(),
                tool_calls: vec![call("echo", "c1", serde_json::json!({"text":"x"}))],
            },
            Msg::assistant("after"),
        ]));
        let agent = Agent::builder(model)
            .tool(Arc::new(Echo))
            .middleware(Arc::new(SuspendGate))
            .build();
        // No suspension before the turn.
        assert!(agent.pending_suspension().is_none());
        match agent.run_turn("go").await.unwrap() {
            TurnResult::Suspended(_) => {}
            _ => panic!("expected suspension"),
        }
        // Parked: the retained suspension is visible and matches the gated call.
        let susp = agent
            .pending_suspension()
            .expect("suspension retained at park");
        assert_eq!(susp.request.tool_call.id, "c1");
        // Resuming consumes the parked turn and clears the retained suspension.
        agent
            .resume(
                crate::interrupt::HitlAction::Deny {
                    feedback: String::new(),
                },
                "Denied.",
            )
            .await
            .unwrap();
        assert!(agent.pending_suspension().is_none());
    }

    #[tokio::test]
    async fn pending_suspension_visible_after_load_thread() {
        // Suspend on one agent, persist, then load on a fresh agent: the
        // re-parked suspension must be visible via pending_suspension.
        let model = Arc::new(ScriptedModel::new(vec![Msg::Assistant {
            content: String::new(),
            tool_calls: vec![call("echo", "c1", serde_json::json!({"text":"x"}))],
        }]));
        let cp = Arc::new(MemCheckpointer::default());
        let agent = Agent::builder(model)
            .tool(Arc::new(Echo))
            .middleware(Arc::new(SuspendGate))
            .checkpointer(cp.clone())
            .thread_id("ts")
            .build();
        match agent.run_turn("go").await.unwrap() {
            TurnResult::Suspended(_) => {}
            _ => panic!("expected suspension"),
        }

        let model2 = Arc::new(ScriptedModel::new(vec![Msg::assistant("after")]));
        let agent2 = Agent::builder(model2)
            .tool(Arc::new(Echo))
            .middleware(Arc::new(SuspendGate))
            .checkpointer(cp)
            .build();
        assert!(agent2.pending_suspension().is_none());
        assert!(agent2.load_thread("ts").await.unwrap());
        let susp = agent2
            .pending_suspension()
            .expect("suspension re-parked after load_thread");
        assert_eq!(susp.request.tool_call.id, "c1");
    }

    #[tokio::test]
    async fn restart_resume_of_multi_call_round_leaves_no_orphaned_tool_calls() {
        // A round of THREE tool calls suspends on the first (gated) one. After a
        // process restart (fresh agent + load_thread), the checkpoint knows the
        // suspension but not the two not-yet-dispatched sibling calls. Resuming
        // must still produce a transcript where every tool_call id has a tool
        // result — otherwise the next model call 400s on orphaned tool_calls.
        let model = Arc::new(ScriptedModel::new(vec![Msg::Assistant {
            content: String::new(),
            tool_calls: vec![
                call("echo", "c1", serde_json::json!({"text":"a"})),
                call("other", "c2", serde_json::json!({"text":"b"})),
                call("other", "c3", serde_json::json!({"text":"c"})),
            ],
        }]));
        let cp = Arc::new(MemCheckpointer::default());
        let agent = Agent::builder(model)
            .tool(Arc::new(Echo))
            .tool(Arc::new(crate::testing::NoopTool::new("other")))
            .middleware(Arc::new(SuspendGate)) // gates "echo" only
            .checkpointer(cp.clone())
            .thread_id("tr")
            .build();
        match agent.run_turn("go").await.unwrap() {
            TurnResult::Suspended(_) => {}
            _ => panic!("expected suspension"),
        }

        // Restart: fresh agent, same checkpointer, load the thread, resume.
        let model2 = Arc::new(ScriptedModel::new(vec![Msg::assistant("after")]));
        let agent2 = Agent::builder(model2.clone())
            .tool(Arc::new(Echo))
            .tool(Arc::new(crate::testing::NoopTool::new("other")))
            .middleware(Arc::new(SuspendGate))
            .checkpointer(cp)
            .build();
        assert!(agent2.load_thread("tr").await.unwrap());
        let out = agent2
            .resume(crate::interrupt::HitlAction::Approve, "Denied.")
            .await
            .unwrap();
        assert!(matches!(out, TurnResult::Done(_)));

        // Every tool_call id on every assistant message must have a tool result.
        let st = agent2.state.lock().unwrap();
        let answered: Vec<&str> = st
            .messages
            .iter()
            .filter_map(|m| match m {
                Msg::Tool { tool_call_id, .. } => Some(tool_call_id.as_str()),
                _ => None,
            })
            .collect();
        for m in &st.messages {
            if let Msg::Assistant { tool_calls, .. } = m {
                for tc in tool_calls {
                    assert!(
                        answered.contains(&tc.id.as_str()),
                        "tool_call {} has no tool result — orphaned transcript",
                        tc.id
                    );
                }
            }
        }
        drop(st);
        // And the model request sent after resume must be transcript-valid too.
        let reqs = model2.requests.lock().unwrap();
        assert!(!reqs.is_empty(), "resume must reach the model");
    }

    #[tokio::test]
    async fn set_thread_id_switches_thread_and_resets_seq() {
        let model = Arc::new(ScriptedModel::new(vec![
            Msg::assistant("first"),
            Msg::assistant("second"),
        ]));
        let cp = Arc::new(MemCheckpointer::default());
        let agent = Agent::builder(model)
            .checkpointer(cp.clone())
            .thread_id("a")
            .build();
        agent.run_turn("hi").await.unwrap();
        // One finish save on thread "a" (plain answer = single round + finish).
        {
            let saves = cp.saves.lock().unwrap();
            assert!(saves.iter().all(|s| s.0 == "a"));
        }
        // Rotate to a fresh thread: saves land under "b" and seq restarts at 0.
        agent.set_thread_id("b");
        agent.run_turn("again").await.unwrap();
        let saves = cp.saves.lock().unwrap();
        let b_saves: Vec<_> = saves.iter().filter(|s| s.0 == "b").collect();
        assert!(!b_saves.is_empty(), "expected saves under thread b");
        assert_eq!(b_saves[0].1, 0, "seq must reset to 0 on the new thread");
    }

    #[tokio::test]
    async fn no_checkpointer_no_saves() {
        let model = Arc::new(ScriptedModel::new(vec![Msg::assistant("done")]));
        let agent = Agent::builder(model).build();
        // Just verifying the persist points are inert without a checkpointer.
        let out = done(agent.run_turn("hi").await.unwrap());
        assert_eq!(out.final_text(), "done");
    }
}
