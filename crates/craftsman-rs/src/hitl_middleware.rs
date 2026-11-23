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
    inline_source: Option<Arc<crate::privacy_entry::InlineDecisionProducer>>,
    privacy_prompt: Option<Arc<crate::privacy_entry::PrivacyApprovalPrompt>>,
    approval: Arc<Mutex<ApprovalState>>,
    /// Route activity lines to stderr (one-shot) instead of stdout (REPL), so
    /// the streamed answer on stdout stays clean in headless mode.
    activity_to_stderr: bool,
    activity_sink: Option<ActivitySink>,
    tool_activity_sink: Option<ToolActivitySink>,
    approval_prompt: Option<ApprovalPrompt>,
    call_approval_prompt: Option<CallApprovalPrompt>,
    workspace_root: PathBuf,
    verification: Option<Arc<crate::verification::VerificationController>>,
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
            inline_source: None,
            privacy_prompt: None,
            approval,
            activity_to_stderr,
            activity_sink,
            tool_activity_sink,
            approval_prompt,
            workspace_root,
            call_approval_prompt: None,
            verification: None,
        }
    }

    pub fn with_call_prompt(mut self, prompt: Option<CallApprovalPrompt>) -> Self {
        self.call_approval_prompt = prompt;
        self
    }
    pub(crate) fn with_privacy_prompt(
        mut self,
        prompt: Option<Arc<crate::privacy_entry::PrivacyApprovalPrompt>>,
    ) -> Self {
        self.privacy_prompt = prompt;
        self
    }
    pub(crate) fn with_privacy_boundary(
        mut self,
        boundary: Option<&Arc<crate::privacy_session::ConversationBoundary>>,
    ) -> Self {
        // Registration belongs to the installed gate's own retained policy
        // and input callback, not a caller's operation or decision tuple.
        self.inline_source = boundary.map(|boundary| {
            crate::privacy_entry::InlineDecisionProducer::register(
                boundary,
                self.approval.clone(),
                self.workspace_root.clone(),
                self.privacy_prompt.clone(),
                self.verification.clone(),
            )
        });
        self
    }

    pub(crate) fn with_verification(
        mut self,
        controller: Option<Arc<crate::verification::VerificationController>>,
    ) -> Self {
        self.verification = controller;
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
        Some(self.prompt_approval(call, summary, details))
    }

    fn prompt_approval(&self, call: &ToolCall, summary: &str, details: &str) -> Decision {
        if let Some(prompt) = &self.call_approval_prompt {
            return match prompt(call, summary, details) {
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
            };
        }
        match &self.approval_prompt {
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
                if call.name == "verify" && self.verification.is_some() {
                    return Decision::Reject(
                        "Declared verification requires an interactive approval callback.".into(),
                    );
                }
                let mut appr = self.approval.lock().unwrap();
                prompt_decision(&mut appr, &call.name, summary)
            }
        }
    }
}

#[async_trait]
impl AgentMiddleware for ApprovalMiddleware {
    fn name(&self) -> &str {
        "hitl"
    }

    /// The gate lets a call through unchanged or answers it itself. It never
    /// touches a recorded message or the system text.
    fn view_support(&self) -> craftsman_harness::provenance::ViewSupport {
        craftsman_harness::provenance::ViewSupport::Preserving
    }

    async fn around_tool(
        &self,
        call: ToolCall,
        ctx: &mut ToolCtx<'_>,
        next: ToolNext<'_>,
    ) -> Result<ToolOutcome, AgentError> {
        let tracked = next.tracked();
        let review = match self
            .verification
            .as_ref()
            .filter(|_| call.name == "verify" && tracked.is_none())
        {
            Some(controller) => Some(controller.approval_context().await?),
            None => None,
        };
        let summary = review.as_ref().map_or_else(
            || summarize_call(&call.name, &call.args),
            |review| review.summary.clone(),
        );
        // In a privacy conversation, what a file write or edit will really
        // do was derived when the agent planned the call, before this
        // review. The policy and a person then decide on the real content.
        // The call itself stays as the model wrote it. A call that could
        // not be derived is answered with the refusal before any effect.
        let reviewed = tracked
            .as_ref()
            .and_then(crate::privacy_session::ReviewedFile::of);
        if let Some(reviewed) = &reviewed {
            if let Err(refusal) = reviewed.derived() {
                return Ok(ToolOutcome::err(refusal.to_string()));
            }
        }
        // Ordinary sinks/ACP frames retain MODEL arguments, never the decoded
        // preview. Only the registered private UI request may carry that view.
        let details = match &review {
            Some(review) => review.details.clone(),
            None => crate::tools::approval_details_at(&call.name, &call.args, &self.workspace_root),
        };
        let call_id = call.id.clone();
        let tool = call.name.clone();
        self.emit(&summary);
        self.emit_tool_event(ToolActivityEvent::Started {
            call_id: call_id.clone(),
            tool: tool.clone(),
            summary: summary.clone(),
            details: details.clone(),
        });

        let mut running_call = call.clone();
        let decision = if let Some(tracked) = &tracked {
            let Some(producer) = &self.inline_source else {
                if let Some(reviewed) = &reviewed {
                    reviewed.refused_before_receive(tracked);
                }
                return Ok(ToolOutcome::err(
                    "This selected call has no registered input/policy producer.",
                ));
            };
            match producer.receive_inline(tracked, None).await {
                Ok(resolution) => {
                    running_call = resolution.call;
                    match resolution.permission {
                        ToolPermission::Allow => None,
                        ToolPermission::Deny(reason) => Some(Decision::Reject(reason)),
                        ToolPermission::Prompt => {
                            return Ok(ToolOutcome::err(
                                "This call has no actual answered approval.",
                            ));
                        }
                    }
                }
                Err(error @ AgentError::Cancelled) => return Err(error),
                Err(refusal) => return Ok(ToolOutcome::err(refusal.to_string())),
            }
        } else if let (Some(controller), Some(review)) = (&self.verification, &review) {
            let (plan, allowed) = {
                let state = self.approval.lock().unwrap();
                (
                    state.mode == crate::config::PermissionMode::Plan,
                    state.auto_approve
                        || state.mode == crate::config::PermissionMode::BypassPermissions,
                )
            };
            if plan {
                Some(Decision::Reject("Permission mode 'plan' does not run verification commands. Switch modes to permit these checks.".into()))
            } else if controller.needs_project_approval()
                || controller.needs_protected_approval(review)
                || !allowed
            {
                let decision = self.prompt_approval(&call, &summary, &details);
                if matches!(decision, Decision::Approve) {
                    controller.approve_protected(review);
                }
                Some(decision)
            } else {
                None
            }
        } else {
            self.approval_decision(&call, &summary, &details)
        };
        if let Some(Decision::Reject(feedback)) = decision {
            let content = rejection_result(&feedback);
            self.emit_tool_event(ToolActivityEvent::Finished {
                call_id,
                tool,
                content: content.clone(),
                is_error: true,
            });
            return Ok(ToolOutcome::err(content));
        }

        let outcome = match next.run(running_call, ctx).await {
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
