//! `ResultBudgetsMiddleware` — caps the byte size of a single tool result. When
//! a tool returns more than `max_bytes_per_result` bytes, the content is
//! truncated at a UTF-8 char boundary and an explicit marker is appended so the
//! model knows the result was clipped.
//!
//! This is a **deterministic** truncation, not an eviction/summarization scheme —
//! the documented Claude-Code divergence. A given oversized result always yields
//! the same clipped output, and the marker states exactly how many of how many
//! bytes are shown so the model can decide to fetch more narrowly.

use crate::error::AgentError;
use crate::middleware::{AgentMiddleware, ToolNext};
use crate::state::ToolCall;
use crate::tool::{ToolCtx, ToolOutcome};
use async_trait::async_trait;

/// Middleware that truncates oversized tool results to a byte budget.
pub struct ResultBudgetsMiddleware {
    max_bytes_per_result: usize,
}

impl ResultBudgetsMiddleware {
    /// Results whose content exceeds `max_bytes_per_result` bytes are truncated.
    pub fn new(max_bytes_per_result: usize) -> Self {
        Self {
            max_bytes_per_result,
        }
    }
}

/// Truncate `content` to at most `max_bytes`, clipping at a UTF-8 char boundary
/// and appending an explicit marker. Returns the content unchanged when it fits.
fn truncate(content: &str, max_bytes: usize) -> String {
    let total = content.len();
    if total <= max_bytes {
        return content.to_string();
    }
    // Walk the cut down to a char boundary so we never split a multibyte char.
    let mut cut = max_bytes;
    while cut > 0 && !content.is_char_boundary(cut) {
        cut -= 1;
    }
    let shown = &content[..cut];
    format!("{shown}… [result truncated: {cut} of {total} bytes shown]")
}

#[async_trait]
impl AgentMiddleware for ResultBudgetsMiddleware {
    fn name(&self) -> &str {
        "result_budgets"
    }

    async fn around_tool(
        &self,
        call: ToolCall,
        ctx: &mut ToolCtx<'_>,
        next: ToolNext<'_>,
    ) -> Result<ToolOutcome, AgentError> {
        let mut outcome = next.run(call, ctx).await?;
        outcome.content = truncate(&outcome.content, self.max_bytes_per_result);
        Ok(outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::middleware::MiddlewareChain;
    use crate::state::AgentState;
    use std::sync::{Arc, Mutex};

    async fn run_returning(chain: &MiddlewareChain, body: String) -> String {
        use crate::backend::StateFs;
        let mut st = AgentState::default();
        let backend = Arc::new(StateFs::new(Arc::new(Mutex::new(AgentState::default()))));
        let registry = Arc::new(crate::subagent::AgentRegistry::default());
        let (notifications, _rx) = crate::subagent::notification_channel();
        let mut ctx = ToolCtx {
            state: &mut st,
            backend,
            registry,
            notifications,
        };
        // Thread the desired result body through the call args: a plain `fn`
        // terminal (needed for the higher-ranked return-lifetime bound; closures
        // can't infer it) reads `args.body` and returns it verbatim.
        let call = ToolCall {
            id: "c".into(),
            name: "noop".into(),
            args: serde_json::json!({ "body": body }),
        };
        fn terminal<'c>(
            call: ToolCall,
            _ctx: &'c mut ToolCtx<'_>,
        ) -> crate::middleware::BoxedToolFuture<'c> {
            let body = call.args["body"].as_str().unwrap_or_default().to_string();
            Box::pin(async move { Ok(ToolOutcome::ok(body)) })
        }
        chain
            .around_tool(call, &mut ctx, &terminal)
            .await
            .unwrap()
            .content
    }

    fn chain(max: usize) -> MiddlewareChain {
        MiddlewareChain::new(vec![Arc::new(ResultBudgetsMiddleware::new(max))])
    }

    #[tokio::test]
    async fn under_budget_passes_through_unchanged() {
        let out = run_returning(&chain(100), "small result".into()).await;
        assert_eq!(out, "small result");
    }

    #[tokio::test]
    async fn over_budget_is_truncated_with_marker() {
        let body = "x".repeat(500);
        let out = run_returning(&chain(100), body).await;
        assert!(out.starts_with(&"x".repeat(100)));
        assert!(out.contains("[result truncated: 100 of 500 bytes shown]"));
        // Deterministic: same input -> same output.
        let out2 = run_returning(&chain(100), "x".repeat(500)).await;
        assert_eq!(out, out2);
    }

    #[test]
    fn truncate_respects_char_boundaries() {
        // Each 'é' is 2 bytes. A budget of 5 lands mid-char at byte 5, so the
        // cut walks back to byte 4 (two full 'é's) — never splitting a char.
        let s = "ééé"; // 6 bytes
        let out = truncate(s, 5);
        assert!(out.starts_with("éé"));
        assert!(out.contains("[result truncated: 4 of 6 bytes shown]"));
        // The shown prefix must be valid UTF-8 (it is, since `out` is a String).
        assert!(out.starts_with("éé…") || out.starts_with("éé… "));
    }

    #[test]
    fn exact_budget_is_not_truncated() {
        let s = "12345";
        assert_eq!(truncate(s, 5), "12345");
    }
}
