//! Framework-side HITL gate: suspends the turn (via `AgentError::Suspended`)
//! for any tool in `interrupt_on`, to be resolved by `Agent::resume`. The
//! reusable, checkpointer-friendly alternative to an app's inline prompt;
//! background subagents use this.
use crate::error::AgentError;
use crate::interrupt::{HitlRequest, Suspension, SuspensionFrame};
use crate::middleware::{AgentMiddleware, ToolNext};
use crate::state::ToolCall;
use crate::tool::{ToolCtx, ToolOutcome};
use async_trait::async_trait;
use std::collections::HashSet;

pub struct HitlPolicyMiddleware {
    interrupt_on: HashSet<String>,
    agent_name: String,
}

impl HitlPolicyMiddleware {
    pub fn new(
        interrupt_on: impl IntoIterator<Item = impl Into<String>>,
        agent_name: impl Into<String>,
    ) -> Self {
        Self {
            interrupt_on: interrupt_on.into_iter().map(Into::into).collect(),
            agent_name: agent_name.into(),
        }
    }
}

#[async_trait]
impl AgentMiddleware for HitlPolicyMiddleware {
    fn name(&self) -> &str {
        "hitl_policy"
    }
    async fn around_tool(
        &self,
        call: ToolCall,
        ctx: &mut ToolCtx<'_>,
        next: ToolNext<'_>,
    ) -> Result<ToolOutcome, AgentError> {
        if self.interrupt_on.contains(&call.name) {
            let summary = format!("{} {}", call.name, call.args);
            return Err(AgentError::Suspended(Box::new(Suspension {
                frames: vec![SuspensionFrame {
                    agent_name: self.agent_name.clone(),
                }],
                request: HitlRequest {
                    tool_call: call,
                    summary,
                },
            })));
        }
        next.run(call, ctx).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interrupt::HitlAction;
    use crate::testing::{NoopTool, ScriptedModel};
    use crate::{Agent, Msg, TurnResult};
    use std::sync::Arc;

    #[tokio::test]
    async fn policy_suspends_listed_tool_only() {
        let model = Arc::new(ScriptedModel::new(vec![
            Msg::Assistant {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall {
                    id: "c1".into(),
                    name: "execute".into(),
                    args: serde_json::json!({}),
                }],
            },
            Msg::assistant("ok"),
        ]));
        let agent = Agent::builder(model)
            .tool(Arc::new(NoopTool::new("execute")))
            .middleware(Arc::new(HitlPolicyMiddleware::new(["execute"], "main")))
            .build();
        assert!(matches!(
            agent.run_turn("go").await.unwrap(),
            TurnResult::Suspended(_)
        ));
        let out = agent.resume(HitlAction::Approve, "Denied.").await.unwrap();
        assert!(matches!(out, TurnResult::Done(_)));
    }

    #[tokio::test]
    async fn policy_ignores_unlisted_tool() {
        let model = Arc::new(ScriptedModel::new(vec![
            Msg::Assistant {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall {
                    id: "c1".into(),
                    name: "read_file".into(),
                    args: serde_json::json!({}),
                }],
            },
            Msg::assistant("ok"),
        ]));
        let agent = Agent::builder(model)
            .tool(Arc::new(NoopTool::new("read_file")))
            .middleware(Arc::new(HitlPolicyMiddleware::new(["execute"], "main")))
            .build();
        assert!(matches!(
            agent.run_turn("go").await.unwrap(),
            TurnResult::Done(_)
        ));
    }
}
