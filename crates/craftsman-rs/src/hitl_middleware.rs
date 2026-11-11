//! The HITL approval gate, as a framework middleware.
//!
//! `around_tool` runs for every tool the agent invokes. For each call it
//! prints a one-line activity summary, then — for the host-mutating tools
//! (`execute`/`write_file`/`edit_file`) that aren't pre-approved — pauses for
//! a human decision. A denial is
//! forwarded to the model as the tool result (so the turn continues); an
//! approval runs the wrapped call and prints its result line.
//!
//! MCP/platform tools are not host-mutating and are never gated (they're not
//! in `GATED_TOOLS`).

use crate::agent::{
    ActivitySink, ApprovalPrompt, ApprovalPromptResult, CallApprovalPrompt, ToolActivityEvent,
    ToolActivitySink,
};
use crate::hitl::{ApprovalState, Decision, ToolPermission, prompt_decision, rejection_result};
use crate::tools::summarize_call;
use async_trait::async_trait;
use craftsman_harness::{AgentError, AgentMiddleware, ToolCall, ToolCtx, ToolNext, ToolOutcome};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub struct ApprovalMiddleware {
    approval: Arc<Mutex<ApprovalState>>,
    /// Route activity lines to stderr (one-shot) instead of stdout (REPL), so
    /// the streamed answer on stdout stays clean in headless mode.
    activity_to_stderr: bool,
    activity_sink: Option<ActivitySink>,
    tool_activity_sink: Option<ToolActivitySink>,
    approval_prompt: Option<ApprovalPrompt>,
    call_approval_prompt: Option<CallApprovalPrompt>,
    workspace_root: PathBuf,
}

impl ApprovalMiddleware {
    #[cfg(test)]
    pub fn new(
        approval: Arc<Mutex<ApprovalState>>,
        activity_to_stderr: bool,
        activity_sink: Option<ActivitySink>,
        tool_activity_sink: Option<ToolActivitySink>,
        approval_prompt: Option<ApprovalPrompt>,
    ) -> Self {
        let workspace_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self::new_at(
            approval,
            activity_to_stderr,
            activity_sink,
            tool_activity_sink,
            approval_prompt,
            workspace_root,
        )
    }

    pub fn new_at(
        approval: Arc<Mutex<ApprovalState>>,
        activity_to_stderr: bool,
        activity_sink: Option<ActivitySink>,
        tool_activity_sink: Option<ToolActivitySink>,
        approval_prompt: Option<ApprovalPrompt>,
        workspace_root: PathBuf,
    ) -> Self {
        Self {
            approval,
            activity_to_stderr,
            activity_sink,
            tool_activity_sink,
            approval_prompt,
            workspace_root,
            call_approval_prompt: None,
        }
    }

    pub fn with_call_prompt(mut self, prompt: Option<CallApprovalPrompt>) -> Self {
        self.call_approval_prompt = prompt;
        self
    }

    fn emit(&self, line: &str) {
        if self.tool_activity_sink.is_some() {
            return;
        }
        if let Some(sink) = &self.activity_sink {
            sink(line);
        } else if self.activity_to_stderr {
            eprintln!("{line}");
        } else {
            println!("\n{line}");
        }
    }

    fn emit_tool_event(&self, event: ToolActivityEvent) {
        if let Some(sink) = &self.tool_activity_sink {
            sink(event);
        }
    }

    fn approval_decision(&self, call: &ToolCall, summary: &str, details: &str) -> Option<Decision> {
        // Never hold the approval-state mutex while the TUI prompt waits for
        // user input. The footer also reads this state while drawing; holding
        // it here would deadlock the renderer before the approval modal appears.
        let permission = self
            .approval
            .lock()
            .unwrap()
            .permission(&call.name, &call.args);
        match permission {
            ToolPermission::Allow => return None,
            ToolPermission::Deny(reason) => return Some(Decision::Reject(reason)),
            ToolPermission::Prompt => {}
        }
        if let Some(prompt) = &self.call_approval_prompt {
            return Some(match prompt(call, summary, details) {
                ApprovalPromptResult::Approve => Decision::Approve,
                ApprovalPromptResult::ApproveAlways => {
                    let mut approval = self.approval.lock().unwrap();
                    if crate::tools::NETWORK_TOOLS.contains(&call.name.as_str()) {
                        approval.auto_approve_network = true;
                    } else {
                        approval.auto_approve = true;
                    }
                    Decision::Approve
                }
                ApprovalPromptResult::Reject(feedback) => Decision::Reject(feedback),
            });
        }
        Some(match &self.approval_prompt {
            Some(prompt) => match prompt(&call.name, summary, details) {
                ApprovalPromptResult::Approve => Decision::Approve,
                ApprovalPromptResult::ApproveAlways => {
                    let mut appr = self.approval.lock().unwrap();
                    if crate::tools::NETWORK_TOOLS.contains(&call.name.as_str()) {
                        appr.auto_approve_network = true;
                    } else {
                        appr.auto_approve = true;
                    }
                    Decision::Approve
                }
                ApprovalPromptResult::Reject(feedback) => Decision::Reject(feedback),
            },
            None => {
                let mut appr = self.approval.lock().unwrap();
                prompt_decision(&mut appr, &call.name, summary)
            }
        })
    }
}

#[async_trait]
impl AgentMiddleware for ApprovalMiddleware {
    fn name(&self) -> &str {
        "hitl"
    }

    async fn around_tool(
        &self,
        call: ToolCall,
        ctx: &mut ToolCtx<'_>,
        next: ToolNext<'_>,
    ) -> Result<ToolOutcome, AgentError> {
        let summary = summarize_call(&call.name, &call.args);
        let details =
            crate::tools::approval_details_at(&call.name, &call.args, &self.workspace_root);
        let call_id = call.id.clone();
        let tool = call.name.clone();
        self.emit(&summary);
        self.emit_tool_event(ToolActivityEvent::Started {
            call_id: call_id.clone(),
            tool: tool.clone(),
            summary: summary.clone(),
            details: details.clone(),
        });

        if let Some(Decision::Reject(feedback)) = self.approval_decision(&call, &summary, &details)
        {
            let content = rejection_result(&feedback);
            self.emit_tool_event(ToolActivityEvent::Finished {
                call_id,
                tool,
                content: content.clone(),
                is_error: true,
            });
            return Ok(ToolOutcome::err(content));
        }

        let outcome = match next.run(call, ctx).await {
            Ok(outcome) => outcome,
            Err(error) => {
                self.emit_tool_event(ToolActivityEvent::Finished {
                    call_id,
                    tool,
                    content: error.to_string(),
                    is_error: true,
                });
                return Err(error);
            }
        };
        let first = outcome.content.lines().next().unwrap_or("");
        self.emit(&format!("   ↳ {}", crate::util::one_line(first, 200)));
        self.emit_tool_event(ToolActivityEvent::Finished {
            call_id,
            tool,
            content: outcome.content.clone(),
            is_error: outcome.is_error,
        });
        Ok(outcome)
    }
}

// The approve and deny paths are covered end-to-end through the real binary in
// `tests/loop_test.rs` (a mock server drives a gated `execute` call; the deny
// test runs with closed stdin so the gate reads EOF and forwards the rejection
// to the model). The gating decision itself is unit-tested in `hitl.rs`.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_prompt_runs_without_holding_approval_mutex() {
        let approval = Arc::new(Mutex::new(ApprovalState::default()));
        let probe = approval.clone();
        let middleware = ApprovalMiddleware::new(
            approval,
            false,
            None,
            None,
            Some(Arc::new(move |_tool, _summary, _details| {
                assert!(
                    probe.try_lock().is_ok(),
                    "approval state must be unlocked while the UI waits"
                );
                ApprovalPromptResult::Reject("no".to_string())
            })),
        );
        let call = ToolCall {
            id: "call-1".to_string(),
            name: "execute".to_string(),
            args: serde_json::json!({"command": "rm file"}),
        };
        assert_eq!(
            middleware.approval_decision(&call, "execute rm file", "command: rm file"),
            Some(Decision::Reject("no".to_string()))
        );
    }

    #[test]
    fn approve_always_updates_state_after_prompt_returns() {
        let approval = Arc::new(Mutex::new(ApprovalState::default()));
        let middleware = ApprovalMiddleware::new(
            approval.clone(),
            false,
            None,
            None,
            Some(Arc::new(|_, _, _| ApprovalPromptResult::ApproveAlways)),
        );
        let call = ToolCall {
            id: "call-1".to_string(),
            name: "execute".to_string(),
            args: serde_json::json!({"command": "rm file"}),
        };
        assert_eq!(
            middleware.approval_decision(&call, "execute rm file", "command: rm file"),
            Some(Decision::Approve)
        );
        assert!(approval.lock().unwrap().auto_approve);
    }
}
