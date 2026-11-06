//! Subagent primitives: the declarative [`SubAgentSpec`], the [`AgentHandle`]
//! trait a spawnable agent implements (with [`Agent`] recursing into
//! itself), a lightweight [`AgentRegistry`] of statuses, and the notification
//! plumbing background agents use to report back.
//!
//! This module is TYPES + the `Agent` handle impl; the `task` tool that
//! spawns children lives in a later task.

use crate::error::AgentError;
use crate::model::ModelProvider;
use crate::state::AgentState;
use crate::tool::Tool;
use crate::{Agent, TurnResult};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Declarative description of a subagent the `task` tool can spawn: a
/// name/description the parent sees, a system
/// prompt, an explicit tool set, an optional model override, and the HITL
/// `interrupt_on` set.
pub struct SubAgentSpec {
    pub name: String,
    pub description: String,
    pub system_prompt: String,
    pub tools: Vec<Arc<dyn Tool>>,
    pub model: Option<Arc<dyn ModelProvider>>,
    /// Resolved model harness applied when this spec is compiled. Runtime
    /// middleware factories are materialized per task, and final tool policy
    /// runs after middleware-contributed tools are collected.
    pub harness_profile: Option<crate::profiles::HarnessProfile>,
    pub interrupt_on: Vec<String>,
    /// RESERVED — not yet enforced. Recursion is currently bounded by
    /// construction instead: a subagent only gets the `task` tool if its spec's
    /// `tools` include it, and no built-in wiring does that. Don't rely on this
    /// field as a depth guard until it is threaded through `ToolCtx`.
    pub max_depth: usize,
}

/// The result of running a subagent task to completion.
pub struct AgentOutcome {
    /// The child's final assistant text.
    pub final_text: String,
    /// The child's ending state, so the caller can merge back file changes.
    pub state_updates: AgentState,
}

/// Outcome of driving a subagent task: it either ran to completion (`Done`) or a
/// gated tool call paused it awaiting a human decision (`Suspended`). Like
/// [`TurnResult`](crate::TurnResult), but at the subagent-task boundary so the
/// `task` tool can surface a suspended background child (via a
/// [`NeedsApproval`](NotificationKind::NeedsApproval) notification) instead of
/// mis-reporting it as a failure.
pub enum TaskResult {
    Done(AgentOutcome),
    Suspended(crate::interrupt::Suspension),
}

/// A spawnable agent: given a prompt and an inherited state, run one task.
/// [`Agent`] implements this by recursing into its own
/// [`run_turn`](Agent::run_turn).
#[async_trait]
pub trait AgentHandle: Send + Sync {
    /// Run one task from `prompt`, seeded from `inherited`. Returns
    /// [`TaskResult::Suspended`] if a gated tool call paused the child.
    async fn run_task(&self, prompt: &str, inherited: AgentState)
    -> Result<TaskResult, AgentError>;

    /// Run one task with an optional mailbox `inbox` drained at each round
    /// boundary (peer `send_message` delivery injected into the transcript as
    /// `user` turns). The default implementation ignores the inbox and delegates
    /// to [`run_task`](AgentHandle::run_task); [`Agent`] overrides it to wire
    /// the receiver in.
    async fn run_task_with_inbox(
        &self,
        prompt: &str,
        inherited: AgentState,
        _inbox: Option<tokio::sync::mpsc::UnboundedReceiver<String>>,
    ) -> Result<TaskResult, AgentError> {
        self.run_task(prompt, inherited).await
    }
}

#[async_trait]
impl AgentHandle for Agent {
    /// Run one task. Seeds a **fresh** conversation from `inherited`
    /// (keeping files/custom/etc. but clearing messages, todos, and the
    /// per-turn system extras; a subagent does not inherit messages or todos),
    /// then drives a single turn.
    ///
    /// Because a subagent `Agent` handle is dedicated to one task at a time,
    /// resetting `self.state` here is safe.
    ///
    /// A gated tool call that pauses the child turn is surfaced as
    /// [`TaskResult::Suspended`] (carrying the [`Suspension`]) rather than an
    /// error, so the caller can decide how to handle it: the foreground `task`
    /// tool declines it (the gated tool has not run), while a background spawn
    /// posts a [`NeedsApproval`](NotificationKind::NeedsApproval) notification.
    async fn run_task(
        &self,
        prompt: &str,
        inherited: AgentState,
    ) -> Result<TaskResult, AgentError> {
        self.run_task_with_inbox(prompt, inherited, None).await
    }

    async fn run_task_with_inbox(
        &self,
        prompt: &str,
        inherited: AgentState,
        inbox: Option<tokio::sync::mpsc::UnboundedReceiver<String>>,
    ) -> Result<TaskResult, AgentError> {
        let mut seed = inherited;
        seed.messages.clear();
        seed.todos.clear();
        seed.system_extras.clear();
        *self.state.lock().unwrap() = seed;

        // Reset per-task plumbing unconditionally: a reused handle must not
        // drain a previous task's mailbox receiver or report its stale parked
        // suspension. (The `task` tool compiles a fresh handle per invocation,
        // but `AgentHandle` is public API — external callers may reuse one.)
        self.reset_task_plumbing(inbox);

        match self.run_turn(prompt).await? {
            TurnResult::Done(out) => Ok(TaskResult::Done(AgentOutcome {
                final_text: out.final_text().to_string(),
                state_updates: self.state.lock().unwrap().clone(),
            })),
            TurnResult::Suspended(s) => Ok(TaskResult::Suspended(s)),
        }
    }
}

/// Lifecycle status of a tracked agent.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    Running,
    Done,
    Failed,
    Suspended,
}

/// A registry entry. Status-only entries (`join`/`inbox` = `None`) come from the
/// simple [`AgentRegistry::insert`]; background agents carry a `JoinHandle` for
/// their spawned Tokio task and an `inbox` sender for mailbox delivery.
pub struct AgentEntry {
    pub status: AgentStatus,
    /// Short live activity label shown by terminal/front-end task views.
    pub progress: String,
    /// The spawned task's join handle, for background agents.
    pub join: Option<tokio::task::JoinHandle<()>>,
    /// Mailbox sender: `send_message` to a running agent pushes here. A
    /// background agent's `run_task_with_inbox` drains these at each round
    /// boundary and injects them as `user` turns (see the `subagents` module doc
    /// on round-boundary injection).
    pub inbox: Option<tokio::sync::mpsc::UnboundedSender<String>>,
}

/// Thread-safe map of agent name → status, so a parent (and the UI) can list
/// and poll spawned subagents.
#[derive(Default)]
pub struct AgentRegistry {
    inner: Mutex<HashMap<String, AgentEntry>>,
    cancellations: Mutex<HashMap<String, crate::TurnCancellation>>,
}

impl AgentRegistry {
    /// Insert (or replace) a status-only entry with no join handle or inbox.
    pub fn insert(&self, name: impl Into<String>, status: AgentStatus) {
        let name = name.into();
        let mut entries = self.inner.lock().unwrap();
        self.cancellations.lock().unwrap().remove(&name);
        entries.insert(
            name,
            AgentEntry {
                status,
                progress: default_progress(status).to_string(),
                join: None,
                inbox: None,
            },
        );
    }

    /// Insert (or replace) a full entry — used for background agents that carry a
    /// join handle and a mailbox sender.
    pub fn insert_entry(&self, name: impl Into<String>, entry: AgentEntry) {
        let name = name.into();
        let mut entries = self.inner.lock().unwrap();
        self.cancellations.lock().unwrap().remove(&name);
        entries.insert(name, entry);
    }

    pub(crate) fn insert_cancellable_entry(
        &self,
        name: String,
        entry: AgentEntry,
        cancellation: Option<crate::TurnCancellation>,
    ) {
        let mut entries = self.inner.lock().unwrap();
        self.cancellations.lock().unwrap().remove(&name);
        if let Some(turn) = cancellation {
            self.cancellations
                .lock()
                .unwrap()
                .insert(name.clone(), turn);
        }
        entries.insert(name, entry);
    }

    /// Transfer actual background joins to the terminal shutdown owner. Entries
    /// remain running until their workers really return and update their status.
    pub fn take_background_joins(&self) -> Vec<tokio::task::JoinHandle<()>> {
        self.inner
            .lock()
            .unwrap()
            .values_mut()
            .filter_map(|entry| entry.join.take())
            .collect()
    }

    /// Deliver a message to a named agent's inbox. Returns `true` only if the
    /// agent is still `Running`, has an inbox, and the send succeeded — a
    /// finished agent's channel may still accept sends (the sender half lives
    /// in the registry), but nothing will ever drain them, so reporting those
    /// as delivered would be a lie.
    pub fn deliver(&self, name: &str, message: String) -> bool {
        match self.inner.lock().unwrap().get(name) {
            Some(AgentEntry {
                status: AgentStatus::Running,
                inbox: Some(tx),
                ..
            }) => tx.send(message).is_ok(),
            _ => false,
        }
    }

    pub fn status(&self, name: &str) -> Option<AgentStatus> {
        self.inner.lock().unwrap().get(name).map(|e| e.status)
    }

    pub fn set_status(&self, name: &str, status: AgentStatus) {
        if let Some(e) = self.inner.lock().unwrap().get_mut(name) {
            e.status = status;
            if status != AgentStatus::Running {
                self.cancellations.lock().unwrap().remove(name);
                e.join = None;
                e.inbox = None;
            }
        }
    }

    /// Attach the spawned background task handle after the entry has been
    /// registered. If the task already finished, dropping the completed handle
    /// is sufficient and avoids reviving a terminal entry as cancellable.
    pub fn set_join(&self, name: &str, join: tokio::task::JoinHandle<()>) {
        let mut entries = self.inner.lock().unwrap();
        match entries.get_mut(name) {
            Some(entry) if entry.status == AgentStatus::Running => entry.join = Some(join),
            _ => drop(join),
        }
    }

    /// Replace the short live progress label for a tracked task.
    pub fn set_progress(&self, name: &str, progress: impl Into<String>) {
        if let Some(e) = self.inner.lock().unwrap().get_mut(name) {
            e.progress = progress.into();
        }
    }

    /// A tracked task's current progress label.
    pub fn progress(&self, name: &str) -> Option<String> {
        self.inner
            .lock()
            .unwrap()
            .get(name)
            .map(|entry| entry.progress.clone())
    }

    /// All tracked agents with status + progress, sorted by name.
    pub fn details(&self) -> Vec<AgentTaskInfo> {
        let mut out: Vec<AgentTaskInfo> = self
            .inner
            .lock()
            .unwrap()
            .iter()
            .map(|(name, entry)| AgentTaskInfo {
                name: name.clone(),
                status: entry.status,
                progress: entry.progress.clone(),
            })
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    /// Abort a running background agent by name. Returns false when the task
    /// does not exist, has already finished, or is not a background task.
    pub fn abort(&self, name: &str) -> bool {
        let mut entries = self.inner.lock().unwrap();
        let Some(entry) = entries.get_mut(name) else {
            return false;
        };
        if entry.status != AgentStatus::Running {
            return false;
        }
        if let Some(turn) = self.cancellations.lock().unwrap().get(name) {
            turn.cancel();
            entry.progress = "cancelling".to_string();
            return true;
        }
        let Some(handle) = entry.join.take() else {
            return false;
        };
        handle.abort();
        entry.status = AgentStatus::Failed;
        entry.progress = "cancelled by user".to_string();
        entry.inbox = None;
        true
    }

    /// All tracked agents as `(name, status)`, sorted by name for determinism.
    pub fn list(&self) -> Vec<(String, AgentStatus)> {
        let mut out: Vec<(String, AgentStatus)> = self
            .inner
            .lock()
            .unwrap()
            .iter()
            .map(|(n, e)| (n.clone(), e.status))
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }
}

fn default_progress(status: AgentStatus) -> &'static str {
    match status {
        AgentStatus::Running => "starting",
        AgentStatus::Done => "completed",
        AgentStatus::Failed => "failed",
        AgentStatus::Suspended => "waiting for approval",
    }
}

/// Immutable task row used by UIs without exposing registry internals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTaskInfo {
    pub name: String,
    pub status: AgentStatus,
    pub progress: String,
}

/// A message flowing between agents (child → parent, or peer → peer).
#[derive(Clone)]
pub struct Notification {
    pub from: String,
    pub to: String,
    pub kind: NotificationKind,
}

/// The payload of a [`Notification`].
#[derive(Clone)]
pub enum NotificationKind {
    /// A background task finished; carries the final text.
    Completed(String),
    /// A free-form inter-agent message.
    Message(String),
    /// A background agent hit a HITL gate and needs a human decision.
    NeedsApproval(crate::interrupt::Suspension),
}

/// Sender half of the notification channel; cheap to clone and hand to many
/// background agents.
#[derive(Clone, Default)]
pub struct NotificationQueue {
    subscribers: Arc<Mutex<Vec<tokio::sync::mpsc::UnboundedSender<Notification>>>>,
}

impl NotificationQueue {
    /// Enqueue a notification for every subscriber. Closed subscribers are
    /// removed lazily; one dropped UI/agent receiver never prevents delivery to
    /// the others.
    pub fn push(&self, n: Notification) {
        self.subscribers
            .lock()
            .unwrap()
            .retain(|tx| tx.send(n.clone()).is_ok());
    }

    /// Add an independent receiver. The main agent and the UI both subscribe so
    /// background results are visible *and* fed back into model context.
    pub fn subscribe(&self) -> NotificationReceiver {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        self.subscribers.lock().unwrap().push(tx);
        NotificationReceiver { rx }
    }
}

/// Receiver half of the notification channel; the parent drains it to surface
/// child updates.
pub struct NotificationReceiver {
    rx: tokio::sync::mpsc::UnboundedReceiver<Notification>,
}

impl NotificationReceiver {
    /// Drain all currently-queued notifications without blocking.
    pub fn drain(&mut self) -> Vec<Notification> {
        let mut out = vec![];
        while let Ok(n) = self.rx.try_recv() {
            out.push(n);
        }
        out
    }
}

/// Create a paired notification queue and receiver.
pub fn notification_channel() -> (NotificationQueue, NotificationReceiver) {
    let queue = NotificationQueue::default();
    let receiver = queue.subscribe();
    (queue, receiver)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::ScriptedModel;
    use crate::{Agent, Msg};
    use std::sync::Arc;

    #[tokio::test]
    async fn deep_agent_implements_agent_handle() {
        let model = Arc::new(ScriptedModel::new(vec![Msg::assistant("child answer")]));
        let agent: Arc<dyn AgentHandle> = Arc::new(Agent::builder(model).name("child").build());
        let out = agent
            .run_task("do a thing", crate::state::AgentState::default())
            .await
            .unwrap();
        match out {
            TaskResult::Done(o) => assert_eq!(o.final_text, "child answer"),
            TaskResult::Suspended(_) => panic!("expected Done"),
        }
    }

    #[tokio::test]
    async fn run_task_with_inbox_injects_messages_at_round_boundary() {
        // A single-round agent: at the round-0 boundary the inbox is drained and
        // its message spliced into the transcript as a `user` turn, so it must
        // appear in the model request the agent sends.
        let model = Arc::new(ScriptedModel::new(vec![Msg::assistant("ok")]));
        let agent = Agent::builder(model.clone()).name("child").build();

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        tx.send("mailbox message".to_string()).unwrap();
        drop(tx);

        let out = agent
            .run_task_with_inbox("go", crate::state::AgentState::default(), Some(rx))
            .await
            .unwrap();
        assert!(matches!(out, TaskResult::Done(_)));

        let reqs = model.requests.lock().unwrap();
        assert!(
            reqs[0]
                .messages
                .iter()
                .any(|m| matches!(m, Msg::User { content } if content == "mailbox message")),
            "inbox message must be injected into the transcript as a user turn"
        );
    }

    #[test]
    fn registry_tracks_and_lists() {
        let reg = AgentRegistry::default();
        reg.insert("researcher", AgentStatus::Running);
        assert_eq!(reg.status("researcher"), Some(AgentStatus::Running));
        reg.set_status("researcher", AgentStatus::Done);
        assert!(
            reg.list()
                .iter()
                .any(|(n, s)| n == "researcher" && *s == AgentStatus::Done)
        );
    }

    #[tokio::test]
    async fn registry_can_abort_a_running_background_task() {
        let registry = AgentRegistry::default();
        let handle = tokio::spawn(async { std::future::pending::<()>().await });
        registry.insert_entry(
            "worker",
            AgentEntry {
                status: AgentStatus::Running,
                progress: "working".into(),
                join: Some(handle),
                inbox: None,
            },
        );
        assert!(registry.abort("worker"));
        assert_eq!(registry.status("worker"), Some(AgentStatus::Failed));
        assert_eq!(
            registry.progress("worker").as_deref(),
            Some("cancelled by user")
        );
        assert!(!registry.abort("worker"));
    }
}
