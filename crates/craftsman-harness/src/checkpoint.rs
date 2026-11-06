//! Durable-state traits. Pure interfaces — storage implementations live in the
//! app (craftsman-rs provides libsql + JSONL); the framework only defines when to
//! persist and how to restore.
use crate::error::AgentError;
use crate::interrupt::Suspension;
use crate::state::AgentState;
use async_trait::async_trait;
use serde::Serialize;

/// Summary of one persisted thread, for a `/threads`-style listing.
#[derive(Debug, Clone)]
pub struct ThreadInfo {
    pub id: String,
    pub created: i64,
    pub updated: i64,
    pub title: String,
    pub checkpoints: u64,
    /// Project directories attached to this conversation, primary first.
    pub project_contexts: Vec<String>,
    /// Deprecated execution-root metadata retained for storage migration.
    pub workspace: String,
}

/// Durable state store for agent threads.
///
/// The framework calls [`save`](Checkpointer::save) at every persist point
/// (each round commit, at a suspension park, and at turn end) and
/// [`load_latest`](Checkpointer::load_latest) via [`Agent::load_thread`].
///
/// By default save failures do not abort the turn. Agents explicitly selecting
/// `AgentBuilder::durable_turn_checkpoints` instead await successful saves
/// before work and after each tool result, and stop further dispatch on failure.
/// Neither policy rolls back completed effects. Load errors always propagate.
#[async_trait]
pub trait Checkpointer: Send + Sync {
    async fn save(
        &self,
        thread_id: &str,
        seq: u64,
        state: &AgentState,
        suspension: Option<&Suspension>,
    ) -> Result<(), AgentError>;
    async fn load_latest(
        &self,
        thread_id: &str,
    ) -> Result<Option<(u64, AgentState, Option<Suspension>)>, AgentError>;
    async fn list_threads(&self) -> Result<Vec<ThreadInfo>, AgentError>;
    async fn delete_thread(&self, thread_id: &str) -> Result<(), AgentError>;
}

/// Facts about a reopened checkpoint. An unresolved call may have had effects;
/// absence of a durable result is never evidence that it did not execute.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecoveryInfo {
    pub unfinished_turn: bool,
    pub unresolved_tool_calls: usize,
}

/// Append-only observability sink (the Claude-Code-style JSONL journal).
/// Implementations must be non-blocking-ish and infallible from the caller's
/// perspective — never let journaling break a turn.
pub trait TranscriptSink: Send + Sync {
    fn append(&self, thread_id: &str, event: &TranscriptEvent<'_>);
}

/// One journal entry. `ts` is Unix seconds stamped by the APP (the framework has
/// no clock dependency); the agent always passes `0` and an app-side sink may
/// restamp on write.
#[derive(Serialize)]
pub struct TranscriptEvent<'a> {
    pub ts: i64,
    pub kind: &'a str,
    pub data: serde_json::Value,
}
