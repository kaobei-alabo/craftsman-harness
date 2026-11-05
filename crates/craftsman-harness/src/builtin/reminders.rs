//! `SystemRemindersMiddleware` — nudges the model back to its todo list when it
//! has been neglected. A Claude-Code-style `<system-reminder>` block is appended
//! to a tool result once the todo list has gone `stale_rounds` tool-dispatch
//! rounds without being rewritten (and there are still open, non-completed
//! items).
//!
//! ## Round / staleness tracking
//! There is no round counter in `AgentState`, so this middleware keeps its own in
//! `state.custom["_todo_meta"]` as `{ "round": u64, "last_write": u64 }`:
//! - `round` increments on every `around_tool` (each tool dispatch is a round
//!   for this purpose).
//! - `last_write` is set to the current round whenever the just-run tool was
//!   `write_todos` (the todo list was refreshed).
//!
//! When `round - last_write >= stale_rounds` and open todos remain, the reminder
//! is appended to that round's tool result.

use crate::error::AgentError;
use crate::middleware::{AgentMiddleware, ToolNext};
use crate::state::{TodoStatus, ToolCall};
use crate::tool::{ToolCtx, ToolOutcome};
use async_trait::async_trait;

const META_KEY: &str = "_todo_meta";

/// The nudge appended to a tool result when the todo list has gone stale.
// PROVENANCE(P5): no match found in deepagents or langchain;
// see docs/oss-review.md 5.1 #2. Rewritten in P5.
const TODO_REMINDER: &str = "\n\n<system-reminder>\nYour todo list has not been \
updated in a while and still has open items. If you have made progress, mark \
completed items done and add any new steps with `write_todos`. If the plan has \
changed, rewrite it. Keep the list an accurate reflection of the current state.\n\
</system-reminder>";

/// Middleware that appends a todo-list reminder when the list is neglected.
pub struct SystemRemindersMiddleware {
    stale_rounds: u64,
}

impl SystemRemindersMiddleware {
    /// `stale_rounds` is how many tool-dispatch rounds may pass without a
    /// `write_todos` before the reminder fires (when open todos remain).
    pub fn new(stale_rounds: u64) -> Self {
        Self { stale_rounds }
    }
}

impl Default for SystemRemindersMiddleware {
    /// Defaults to a 3-round staleness window.
    fn default() -> Self {
        Self::new(3)
    }
}

/// Read `(round, last_write)` from `state.custom["_todo_meta"]`, defaulting to
/// `(0, 0)` when absent or malformed.
fn read_meta(custom: &serde_json::Map<String, serde_json::Value>) -> (u64, u64) {
    let meta = custom.get(META_KEY);
    let round = meta
        .and_then(|m| m.get("round"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let last_write = meta
        .and_then(|m| m.get("last_write"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    (round, last_write)
}

fn write_meta(
    custom: &mut serde_json::Map<String, serde_json::Value>,
    round: u64,
    last_write: u64,
) {
    custom.insert(
        META_KEY.to_string(),
        serde_json::json!({ "round": round, "last_write": last_write }),
    );
}

#[async_trait]
impl AgentMiddleware for SystemRemindersMiddleware {
    fn name(&self) -> &str {
        "system_reminders"
    }

    async fn around_tool(
        &self,
        call: ToolCall,
        ctx: &mut ToolCtx<'_>,
        next: ToolNext<'_>,
    ) -> Result<ToolOutcome, AgentError> {
        let is_write_todos = call.name == "write_todos";
        let mut outcome = next.run(call, ctx).await?;

        // Advance the round counter and record a write if this was write_todos.
        let (round, last_write) = read_meta(&ctx.state.custom);
        let round = round + 1;
        let last_write = if is_write_todos { round } else { last_write };
        write_meta(&mut ctx.state.custom, round, last_write);

        let has_open = ctx
            .state
            .todos
            .iter()
            .any(|t| t.status != TodoStatus::Completed);

        // Fire only when the list is stale AND still has open work, and never on
        // the very round the list was (re)written.
        if has_open && !is_write_todos && round.saturating_sub(last_write) >= self.stale_rounds {
            outcome.content.push_str(TODO_REMINDER);
        }
        Ok(outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::middleware::MiddlewareChain;
    use crate::state::{AgentState, Todo, TodoStatus};
    use std::sync::{Arc, Mutex};

    /// Drive one tool dispatch through a chain containing only the reminders
    /// middleware, returning the resulting tool-result content. `tool_name` is
    /// the call being run; the terminal always returns "OK".
    async fn run_call(chain: &MiddlewareChain, st: &mut AgentState, tool_name: &str) -> String {
        use crate::backend::StateFs;
        let backend = Arc::new(StateFs::new(Arc::new(Mutex::new(AgentState::default()))));
        let registry = Arc::new(crate::subagent::AgentRegistry::default());
        let (notifications, _rx) = crate::subagent::notification_channel();
        let mut ctx = ToolCtx {
            state: st,
            backend,
            registry,
            notifications,
        };
        let call = ToolCall {
            id: "c".into(),
            name: tool_name.into(),
            args: serde_json::json!({}),
        };
        fn terminal<'c>(
            _call: ToolCall,
            _ctx: &'c mut ToolCtx<'_>,
        ) -> crate::middleware::BoxedToolFuture<'c> {
            Box::pin(async { Ok(ToolOutcome::ok("OK")) })
        }
        chain
            .around_tool(call, &mut ctx, &terminal)
            .await
            .unwrap()
            .content
    }

    fn chain(stale: u64) -> MiddlewareChain {
        MiddlewareChain::new(vec![Arc::new(SystemRemindersMiddleware::new(stale))])
    }

    #[tokio::test]
    async fn no_reminder_when_no_open_todos() {
        let chain = chain(1);
        let mut st = AgentState::default();
        // Completed todo only — nothing open to nudge about.
        st.todos.push(Todo {
            content: "done".into(),
            status: TodoStatus::Completed,
        });
        // Run several rounds; never reminded.
        for _ in 0..5 {
            let out = run_call(&chain, &mut st, "noop").await;
            assert_eq!(out, "OK");
        }
    }

    #[tokio::test]
    async fn reminder_fires_after_stale_rounds() {
        let chain = chain(3);
        let mut st = AgentState::default();
        st.todos.push(Todo {
            content: "step".into(),
            status: TodoStatus::InProgress,
        });
        // Rounds 1 and 2: not yet stale (round - last_write(0) < 3).
        assert_eq!(run_call(&chain, &mut st, "noop").await, "OK");
        assert_eq!(run_call(&chain, &mut st, "noop").await, "OK");
        // Round 3: 3 - 0 >= 3 -> reminder fires.
        let out = run_call(&chain, &mut st, "noop").await;
        assert!(
            out.contains("<system-reminder>"),
            "expected reminder, got: {out}"
        );
    }

    #[tokio::test]
    async fn write_todos_resets_staleness_and_never_self_reminds() {
        let chain = chain(2);
        let mut st = AgentState::default();
        st.todos.push(Todo {
            content: "step".into(),
            status: TodoStatus::Pending,
        });
        // Round 1: write_todos — resets last_write to 1, and never appends a
        // reminder to its own result.
        let out = run_call(&chain, &mut st, "write_todos").await;
        assert!(!out.contains("<system-reminder>"));
        // Round 2: 2 - 1 = 1 < 2, no reminder.
        assert_eq!(run_call(&chain, &mut st, "noop").await, "OK");
        // Round 3: 3 - 1 = 2 >= 2, reminder fires.
        let out = run_call(&chain, &mut st, "noop").await;
        assert!(out.contains("<system-reminder>"));
    }
}
