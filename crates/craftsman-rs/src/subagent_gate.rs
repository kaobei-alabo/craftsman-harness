//! HITL gate for delegated (subagent) tools.
//!
//! The main agent's `ApprovalMiddleware` only wraps the MAIN agent's tool
//! calls — a subagent is a separate `Agent`, so without its own gate,
//! `task("use execute to run rm -rf …")` would mutate the host with zero
//! prompts: one level of delegation would defeat the safety boundary
//! (rule #6).
//!
//! Interactive frontends can provide the same approval callback used by the
//! main agent. A delegated gated call then opens the normal approval modal and
//! continues that exact tool call after approval. Headless runs without an
//! approval channel remain fail-closed and return guidance instead of blocking.

use crate::agent::{
    ApprovalPrompt, ApprovalPromptResult, CallApprovalPrompt, ToolActivityEvent, ToolActivitySink,
};
use crate::hitl::{ApprovalState, ToolPermission, rejection_result};
use async_trait::async_trait;
use craftsman_harness::{AgentError, Tool, ToolCall, ToolCtx, ToolDef, ToolOutcome};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub struct SubagentGate {
    inner: Arc<dyn Tool>,
    approval: Arc<Mutex<ApprovalState>>,
    approval_prompt: Option<ApprovalPrompt>,
    workspace_root: PathBuf,
    correlated: Option<(CallApprovalPrompt, ToolActivitySink)>,
}

impl SubagentGate {
    /// Wrap every tool in `tools` that is HITL-gated (host-mutating or
    /// network-egress); pass the rest through untouched.
    #[cfg(test)]
    pub fn wrap_all(
        tools: Vec<Arc<dyn Tool>>,
        approval: &Arc<Mutex<ApprovalState>>,
        approval_prompt: Option<ApprovalPrompt>,
    ) -> Vec<Arc<dyn Tool>> {
        let workspace = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self::wrap_all_at(tools, approval, approval_prompt, &workspace)
    }

    #[cfg(test)]
    pub fn wrap_all_at(
        tools: Vec<Arc<dyn Tool>>,
        approval: &Arc<Mutex<ApprovalState>>,
        approval_prompt: Option<ApprovalPrompt>,
        workspace_root: &Path,
    ) -> Vec<Arc<dyn Tool>> {
        Self::wrap_all_at_correlated(tools, approval, approval_prompt, workspace_root, None)
    }

    /// ACP assigns a presentation identity at the actual delegated invocation,
    /// including allowed/read-only tools, without changing framework ToolCtx.
    pub fn wrap_all_at_correlated(
        tools: Vec<Arc<dyn Tool>>,
        approval: &Arc<Mutex<ApprovalState>>,
        approval_prompt: Option<ApprovalPrompt>,
        workspace_root: &Path,
        correlated: Option<(CallApprovalPrompt, ToolActivitySink)>,
    ) -> Vec<Arc<dyn Tool>> {
        use crate::tools::{GATED_TOOLS, NETWORK_TOOLS};
        tools
            .into_iter()
            .map(|t| {
                let name = t.def().name;
                if correlated.is_some()
                    || GATED_TOOLS.contains(&name.as_str())
                    || NETWORK_TOOLS.contains(&name.as_str())
                {
                    Arc::new(SubagentGate {
                        inner: t,
                        approval: approval.clone(),
                        approval_prompt: approval_prompt.clone(),
                        workspace_root: workspace_root.to_path_buf(),
                        correlated: correlated.clone(),
                    }) as Arc<dyn Tool>
                } else {
                    t
                }
            })
            .collect()
    }
}

#[async_trait]
impl Tool for SubagentGate {
    fn def(&self) -> ToolDef {
        self.inner.def()
    }

    async fn call(&self, args: Value, ctx: &mut ToolCtx<'_>) -> Result<ToolOutcome, AgentError> {
        let Some((_, sink)) = &self.correlated else {
            return self.call_gated(args, ctx, None).await;
        };
        // Different specs own different wrappers. The shared process counter
        // prevents identity reuse across specs and concurrent child runs.
        static DELEGATED_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let call = ToolCall {
            id: format!(
                "delegated-{}",
                DELEGATED_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ),
            name: self.inner.def().name,
            args: args.clone(),
        };
        sink(ToolActivityEvent::Started {
            call_id: call.id.clone(),
            tool: call.name.clone(),
            summary: crate::tools::summarize_call(&call.name, &args),
            details: crate::tools::approval_details_at(&call.name, &args, &self.workspace_root),
        });
        let result = self.call_gated(args, ctx, Some(&call)).await;
        let (content, is_error) = match &result {
            Ok(outcome) => (outcome.content.clone(), outcome.is_error),
            Err(error) => (error.to_string(), true),
        };
        sink(ToolActivityEvent::Finished {
            call_id: call.id,
            tool: call.name,
            content,
            is_error,
        });
        result
    }
}

impl SubagentGate {
    async fn call_gated(
        &self,
        args: Value,
        ctx: &mut ToolCtx<'_>,
        call: Option<&ToolCall>,
    ) -> Result<ToolOutcome, AgentError> {
        let name = self.inner.def().name;
        // Same decision the main gate makes — auto_approve, the network
        // switch, and the shell allow-list all apply. Lock scope is
        // synchronous only (never held across the await below).
        let permission = self.approval.lock().unwrap().permission(&name, &args);
        match permission {
            ToolPermission::Allow => self.inner.call(args, ctx).await,
            ToolPermission::Deny(reason) => Ok(ToolOutcome::err(reason)),
            ToolPermission::Prompt => {
                if self.approval_prompt.is_none() && self.correlated.is_none() {
                    return Ok(ToolOutcome::err(format!(
                        "'{name}' requires user approval, but this delegated run has no interactive approval channel. Do this step directly or use an explicit permission mode."
                    )));
                }
                let summary = crate::tools::summarize_call(&name, &args);
                let details = crate::tools::approval_details_at(&name, &args, &self.workspace_root);
                let decision = if let (Some((prompt, _)), Some(call)) = (&self.correlated, call) {
                    prompt(call, &summary, &details)
                } else if let Some(prompt) = &self.approval_prompt {
                    prompt(&name, &summary, &details)
                } else {
                    ApprovalPromptResult::Reject("no correlated approval channel".into())
                };
                match decision {
                    ApprovalPromptResult::Approve => self.inner.call(args, ctx).await,
                    ApprovalPromptResult::ApproveAlways => {
                        {
                            let mut state = self.approval.lock().unwrap();
                            if crate::tools::NETWORK_TOOLS.contains(&name.as_str()) {
                                state.auto_approve_network = true;
                            } else {
                                state.auto_approve = true;
                            }
                        }
                        self.inner.call(args, ctx).await
                    }
                    ApprovalPromptResult::Reject(feedback) => {
                        Ok(ToolOutcome::err(rejection_result(&feedback)))
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use craftsman_harness::{AgentRegistry, AgentState, Backend, StateFs, notification_channel};
    use serde_json::json;

    /// A tool that records whether it ran.
    struct Recorder {
        name: String,
        ran: Arc<Mutex<bool>>,
    }
    #[async_trait]
    impl Tool for Recorder {
        fn def(&self) -> ToolDef {
            ToolDef {
                name: self.name.clone(),
                description: "test".into(),
                parameters: json!({"type":"object"}),
            }
        }
        async fn call(&self, _: Value, _: &mut ToolCtx<'_>) -> Result<ToolOutcome, AgentError> {
            *self.ran.lock().unwrap() = true;
            Ok(ToolOutcome::ok("ran"))
        }
    }

    fn ctx_parts() -> (
        Arc<Mutex<AgentState>>,
        Arc<dyn Backend>,
        Arc<AgentRegistry>,
        craftsman_harness::NotificationQueue,
    ) {
        let state = Arc::new(Mutex::new(AgentState::default()));
        let backend: Arc<dyn Backend> = Arc::new(StateFs::new(state.clone()));
        let registry = Arc::new(AgentRegistry::default());
        let (notif, _rx) = notification_channel();
        (state, backend, registry, notif)
    }

    #[allow(clippy::type_complexity)]
    fn gated_recorder(
        name: &str,
        approval: ApprovalState,
    ) -> (Arc<dyn Tool>, Arc<Mutex<bool>>, Arc<Mutex<ApprovalState>>) {
        let ran = Arc::new(Mutex::new(false));
        let approval = Arc::new(Mutex::new(approval));
        let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(Recorder {
            name: name.into(),
            ran: ran.clone(),
        })];
        let wrapped = SubagentGate::wrap_all(tools, &approval, None);
        (wrapped.into_iter().next().unwrap(), ran, approval)
    }

    #[tokio::test]
    async fn gated_tool_is_denied_without_auto_approve() {
        let (tool, ran, _) = gated_recorder("execute", ApprovalState::default());
        let (state, backend, registry, notifications) = ctx_parts();
        let mut working = state.lock().unwrap().clone();
        let mut ctx = ToolCtx {
            state: &mut working,
            backend,
            registry,
            notifications,
        };
        let out = tool
            .call(json!({"command": "rm -rf /tmp/x"}), &mut ctx)
            .await
            .unwrap();
        assert!(out.is_error, "must be denied");
        assert!(out.content.contains("requires user approval"));
        assert!(!*ran.lock().unwrap(), "the gated tool must NOT have run");
    }

    #[tokio::test]
    async fn delegated_prompt_approval_runs_the_original_tool_call() {
        let ran = Arc::new(Mutex::new(false));
        let approval = Arc::new(Mutex::new(ApprovalState::default()));
        let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(Recorder {
            name: "write_file".into(),
            ran: ran.clone(),
        })];
        let prompted = Arc::new(Mutex::new(Vec::new()));
        let seen = prompted.clone();
        let prompt: ApprovalPrompt = Arc::new(move |tool, summary, details| {
            seen.lock()
                .unwrap()
                .push((tool.to_string(), summary.to_string(), details.to_string()));
            ApprovalPromptResult::Approve
        });
        let wrapped = SubagentGate::wrap_all(tools, &approval, Some(prompt));
        let (state, backend, registry, notifications) = ctx_parts();
        let mut working = state.lock().unwrap().clone();
        let mut ctx = ToolCtx {
            state: &mut working,
            backend,
            registry,
            notifications,
        };

        let out = wrapped[0]
            .call(
                json!({"file_path": "new.txt", "content": "hello"}),
                &mut ctx,
            )
            .await
            .unwrap();
        assert!(!out.is_error);
        assert!(*ran.lock().unwrap());
        let prompts = prompted.lock().unwrap();
        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0].0, "write_file");
        assert!(prompts[0].1.contains("new.txt"));
        assert!(prompts[0].2.contains("Preview"));
    }

    #[tokio::test]
    async fn delegated_prompt_rejection_returns_feedback_without_running() {
        let ran = Arc::new(Mutex::new(false));
        let approval = Arc::new(Mutex::new(ApprovalState::default()));
        let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(Recorder {
            name: "execute".into(),
            ran: ran.clone(),
        })];
        let prompt: ApprovalPrompt =
            Arc::new(|_, _, _| ApprovalPromptResult::Reject("use a safer command".into()));
        let wrapped = SubagentGate::wrap_all(tools, &approval, Some(prompt));
        let (state, backend, registry, notifications) = ctx_parts();
        let mut working = state.lock().unwrap().clone();
        let mut ctx = ToolCtx {
            state: &mut working,
            backend,
            registry,
            notifications,
        };

        let out = wrapped[0]
            .call(json!({"command": "rm file"}), &mut ctx)
            .await
            .unwrap();
        assert!(out.is_error);
        assert!(out.content.contains("use a safer command"));
        assert!(!*ran.lock().unwrap());
    }

    #[tokio::test]
    async fn delegated_approve_always_updates_shared_permission_state() {
        let ran = Arc::new(Mutex::new(false));
        let approval = Arc::new(Mutex::new(ApprovalState::default()));
        let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(Recorder {
            name: "edit_file".into(),
            ran: ran.clone(),
        })];
        let prompt: ApprovalPrompt = Arc::new(|_, _, _| ApprovalPromptResult::ApproveAlways);
        let wrapped = SubagentGate::wrap_all(tools, &approval, Some(prompt));
        let (state, backend, registry, notifications) = ctx_parts();
        let mut working = state.lock().unwrap().clone();
        let mut ctx = ToolCtx {
            state: &mut working,
            backend,
            registry,
            notifications,
        };

        let out = wrapped[0].call(json!({}), &mut ctx).await.unwrap();
        assert!(!out.is_error);
        assert!(*ran.lock().unwrap());
        assert!(approval.lock().unwrap().auto_approve);
    }

    #[tokio::test]
    async fn gated_tool_runs_under_auto_approve_and_allow_list() {
        // auto_approve covers everything.
        let (tool, ran, _) = gated_recorder(
            "execute",
            ApprovalState {
                auto_approve: true,
                ..Default::default()
            },
        );
        let (state, backend, registry, notifications) = ctx_parts();
        let mut working = state.lock().unwrap().clone();
        let mut ctx = ToolCtx {
            state: &mut working,
            backend,
            registry,
            notifications,
        };
        let out = tool.call(json!({"command": "ls"}), &mut ctx).await.unwrap();
        assert!(!out.is_error);
        assert!(*ran.lock().unwrap());

        // The shell allow-list pre-approves an execute by first token.
        let (tool, ran, _) = gated_recorder(
            "execute",
            ApprovalState {
                shell_allow_list: vec!["git".into()],
                ..Default::default()
            },
        );
        let (state, backend, registry, notifications) = ctx_parts();
        let mut working = state.lock().unwrap().clone();
        let mut ctx = ToolCtx {
            state: &mut working,
            backend,
            registry,
            notifications,
        };
        let out = tool
            .call(json!({"command": "git status"}), &mut ctx)
            .await
            .unwrap();
        if cfg!(windows) {
            assert!(out.is_error, "Windows allow-list remains fail-closed");
            assert!(!*ran.lock().unwrap());
        } else {
            assert!(!out.is_error);
            assert!(*ran.lock().unwrap());
        }
    }

    #[tokio::test]
    async fn mid_session_always_is_honored_dynamically() {
        // The gate consults the SHARED state at call time: flipping
        // auto_approve after wrap (like the "always" prompt choice) ungates.
        let (tool, ran, approval) = gated_recorder("write_file", ApprovalState::default());
        approval.lock().unwrap().auto_approve = true;
        let (state, backend, registry, notifications) = ctx_parts();
        let mut working = state.lock().unwrap().clone();
        let mut ctx = ToolCtx {
            state: &mut working,
            backend,
            registry,
            notifications,
        };
        let out = tool.call(json!({}), &mut ctx).await.unwrap();
        assert!(!out.is_error);
        assert!(*ran.lock().unwrap());
    }

    #[tokio::test]
    async fn ungated_tools_pass_through_unwrapped() {
        let ran = Arc::new(Mutex::new(false));
        let approval = Arc::new(Mutex::new(ApprovalState::default()));
        let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(Recorder {
            name: "read_file".into(),
            ran: ran.clone(),
        })];
        let wrapped = SubagentGate::wrap_all(tools, &approval, None);
        let (state, backend, registry, notifications) = ctx_parts();
        let mut working = state.lock().unwrap().clone();
        let mut ctx = ToolCtx {
            state: &mut working,
            backend,
            registry,
            notifications,
        };
        let out = wrapped[0].call(json!({}), &mut ctx).await.unwrap();
        assert!(!out.is_error);
        assert!(*ran.lock().unwrap(), "read_file is not gated and must run");
    }

    #[tokio::test]
    async fn network_tools_gate_under_network_switch() {
        // web_fetch denied by default…
        let (tool, ran, _) = gated_recorder("web_fetch", ApprovalState::default());
        let (state, backend, registry, notifications) = ctx_parts();
        let mut working = state.lock().unwrap().clone();
        let mut ctx = ToolCtx {
            state: &mut working,
            backend,
            registry,
            notifications,
        };
        let out = tool
            .call(json!({"url": "https://example.com"}), &mut ctx)
            .await
            .unwrap();
        assert!(out.is_error);
        assert!(!*ran.lock().unwrap());

        // …and allowed under auto_approve_network alone.
        let (tool, ran, _) = gated_recorder(
            "web_fetch",
            ApprovalState {
                auto_approve_network: true,
                ..Default::default()
            },
        );
        let (state, backend, registry, notifications) = ctx_parts();
        let mut working = state.lock().unwrap().clone();
        let mut ctx = ToolCtx {
            state: &mut working,
            backend,
            registry,
            notifications,
        };
        let out = tool
            .call(json!({"url": "https://example.com"}), &mut ctx)
            .await
            .unwrap();
        assert!(!out.is_error);
        assert!(*ran.lock().unwrap());
    }
}
