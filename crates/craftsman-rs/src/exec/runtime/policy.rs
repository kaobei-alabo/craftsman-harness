//! One immutable invocation gate and fail latch across tools and all providers.
//! Tool wrappers are the final boundary, including middleware-contributed tools:
//! profile transforms and a future mistaken resume cannot bypass the middleware.

use super::super::permissions::{InvocationPermissions, OnUnapproved, ToolDecision};
use async_trait::async_trait;
use craftsman_harness::{
    AgentError, AgentMiddleware, AgentState, Flow, ModelProvider, ModelRequest, ModelResponse,
    Next, Tool, ToolCall, ToolCtx, ToolDef, ToolErrorKind, ToolMeta, ToolNext, ToolOutcome,
};
use serde_json::Value;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub(crate) const NAME: &str = "exec_invocation";

pub(crate) struct InvocationGate {
    permissions: Arc<InvocationPermissions>,
    approval_required: AtomicBool,
    execution_closed: AtomicBool,
    approval_changed: tokio::sync::Notify,
    // Only fixed gate diagnostics select this publisher. Other tool/provider
    // bodies still require their own original-byte, pre-cut adapters.
    fixed_redactor: Option<crate::exec::redaction::Redactor>,
}

impl InvocationGate {
    #[cfg(test)]
    pub(crate) fn new(permissions: Arc<InvocationPermissions>) -> Self {
        Self {
            permissions,
            approval_required: AtomicBool::new(false),
            execution_closed: AtomicBool::new(false),
            approval_changed: tokio::sync::Notify::new(),
            fixed_redactor: None,
        }
    }

    /// Actual builder construction captures permissions and fixed-diagnostic
    /// seeds together; no separately supplied redactor or ambient reload.
    pub(crate) fn captured(prepared: &crate::exec::construction::Prepared) -> Self {
        Self {
            permissions: prepared.permissions().clone(),
            approval_required: AtomicBool::new(false),
            execution_closed: AtomicBool::new(false),
            approval_changed: tokio::sync::Notify::new(),
            fixed_redactor: Some(prepared.credentials().redactor().clone()),
        }
    }

    pub(crate) fn approval_required(&self) -> bool {
        self.approval_required.load(Ordering::Acquire)
    }

    /// Deny-only finalization seal, independent of the trusted approval fact.
    pub(crate) fn close_execution(&self) {
        self.execution_closed.store(true, Ordering::Release);
    }
    pub(crate) fn execution_closed(&self) -> bool {
        self.execution_closed.load(Ordering::Acquire)
    }

    #[cfg(feature = "gadgets")]
    pub(crate) fn matches_permissions(&self, permissions: &Arc<InvocationPermissions>) -> bool {
        Arc::ptr_eq(&self.permissions, permissions)
    }

    /// Observe trusted typed outcomes/import failures, never guest/model strings.
    /// Source approval and per-import unapproved errors share this SAME latch.
    pub(crate) fn observe_error_kind(&self, kind: ToolErrorKind) {
        if kind == ToolErrorKind::NeedsApproval
            && self.permissions.on_unapproved() == OnUnapproved::Fail
        {
            self.approval_required.store(true, Ordering::Release);
            self.approval_changed.notify_waiters();
        }
    }

    fn observe_outcome(&self, outcome: &ToolOutcome) {
        if let (true, Some(kind)) = (outcome.is_error, outcome.meta.error_kind) {
            self.observe_error_kind(kind);
        }
    }

    /// Registration before checking closes the lost-wakeup race; all main,
    /// child and guest waiters can observe the monotonic invocation seal.
    pub(crate) async fn wait_approval_required(&self) {
        loop {
            let changed = self.approval_changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.approval_required() {
                return;
            }
            changed.await;
        }
    }

    fn check(&self) -> Result<(), AgentError> {
        if self.approval_required() {
            Err(AgentError::State(
                "headless invocation requires explicit approval".into(),
            ))
        } else if self.execution_closed() {
            Err(AgentError::State("[REDACTED]".into()))
        } else {
            Ok(())
        }
    }

    pub(crate) fn denied(&self, name: &str, args: &Value) -> Option<ToolOutcome> {
        let (message, kind) = if self.approval_required() {
            (
                "Headless invocation stopped; this tool was not executed.",
                ToolErrorKind::NeedsApproval,
            )
        } else if self.execution_closed() {
            (
                "This invocation is finalized; this tool was not executed.",
                ToolErrorKind::DeniedCapability,
            )
        } else if !supported_tool(name) {
            (
                "This capability is unavailable in headless v1.",
                ToolErrorKind::DeniedCapability,
            )
        } else {
            match self.permissions.tool_decision(name, args) {
                ToolDecision::Allow => return None,
                ToolDecision::Deny => (
                    "This tool call is denied by the invocation policy.",
                    ToolErrorKind::DeniedCapability,
                ),
                ToolDecision::Unapproved => {
                    self.observe_error_kind(ToolErrorKind::NeedsApproval);
                    (
                        "This tool call requires an explicit invocation allow.",
                        ToolErrorKind::NeedsApproval,
                    )
                }
            }
        };
        // Fixed data only: never echo model arguments, rules or credential names.
        let mut meta = ToolMeta::new();
        meta.error_kind = Some(kind);
        let message = match &self.fixed_redactor {
            None => message.to_owned(), // Permission-only test foundations.
            Some(redactor) => {
                let safe = redactor.text(message).ok().filter(|s| s.len() <= 4096);
                let safe = safe.filter(|s| {
                    let mut check = redactor.byte_stream();
                    check.push(s.as_bytes()).is_ok() && !check.matched()
                });
                if safe.is_none() {
                    meta.truncated = true;
                }
                safe.unwrap_or_else(|| "[REDACTED]".into())
            }
        };
        Some(ToolOutcome::err(message).with_meta(meta))
    }

    pub(crate) fn wrap_tool(self: &Arc<Self>, inner: Arc<dyn Tool>) -> Arc<dyn Tool> {
        Arc::new(GuardedTool {
            inner,
            gate: self.clone(),
        })
    }

    pub(crate) fn wrap_tools(self: &Arc<Self>, tools: Vec<Arc<dyn Tool>>) -> Vec<Arc<dyn Tool>> {
        tools
            .into_iter()
            .filter(|tool| supported_tool(&tool.def().name))
            .map(|tool| self.wrap_tool(tool))
            .collect()
    }

    pub(crate) fn wrap_provider(
        self: &Arc<Self>,
        inner: Arc<dyn ModelProvider>,
    ) -> Arc<dyn ModelProvider> {
        Arc::new(GuardedProvider {
            inner,
            gate: self.clone(),
        })
    }

    pub(crate) fn wrap_middleware(
        self: &Arc<Self>,
        inner: Arc<dyn AgentMiddleware>,
    ) -> Arc<dyn AgentMiddleware> {
        Arc::new(GuardedMiddleware {
            inner,
            gate: self.clone(),
        })
    }
}

// Callables remain absent until the strict source/grant/guest-host adapter exists.
fn supported_tool(name: &str) -> bool {
    matches!(
        name,
        "read_file"
            | "write_file"
            | "edit_file"
            | "ls"
            | "glob"
            | "grep"
            | "execute"
            | "web_fetch"
            | "web_search"
            | "write_todos"
            | "task"
            | "send_message"
            | "ask_user"
    )
}

#[async_trait]
impl AgentMiddleware for InvocationGate {
    fn name(&self) -> &str {
        NAME
    }

    async fn prepare_model(&self, _: &mut AgentState) -> Result<(), AgentError> {
        self.check()
    }

    async fn around_tool(
        &self,
        call: ToolCall,
        ctx: &mut ToolCtx<'_>,
        next: ToolNext<'_>,
    ) -> Result<ToolOutcome, AgentError> {
        match self.denied(&call.name, &call.args) {
            Some(outcome) => Ok(outcome),
            None => {
                let outcome = next.run(call, ctx).await?;
                self.observe_outcome(&outcome);
                Ok(outcome)
            }
        }
    }
}

struct GuardedTool {
    inner: Arc<dyn Tool>,
    gate: Arc<InvocationGate>,
}
#[async_trait]
impl Tool for GuardedTool {
    fn def(&self) -> ToolDef {
        let mut def = self.inner.def();
        if def.name == "task" {
            if let Some(properties) = def
                .parameters
                .get_mut("properties")
                .and_then(Value::as_object_mut)
            {
                properties.remove("run_in_background");
            }
            def.description = "Run a declared foreground subagent with isolated conversation context and the same immutable invocation permissions. Background execution and child delegation are unavailable.".into();
        }
        def
    }
    async fn call(&self, args: Value, ctx: &mut ToolCtx<'_>) -> Result<ToolOutcome, AgentError> {
        match self.gate.denied(&self.inner.def().name, &args) {
            Some(outcome) => Ok(outcome),
            None => {
                let outcome = self.inner.call(args, ctx).await?;
                self.gate.observe_outcome(&outcome);
                Ok(outcome)
            }
        }
    }
}

struct GuardedProvider {
    inner: Arc<dyn ModelProvider>,
    gate: Arc<InvocationGate>,
}
#[async_trait]
impl ModelProvider for GuardedProvider {
    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, AgentError> {
        self.gate.check()?;
        let response = self.inner.complete(request).await;
        // Compaction calls its provider outside the ordinary middleware onion.
        // The after-check also prevents same-poll post-failure tool dispatch.
        self.gate.check()?;
        response
    }
}

struct GuardedMiddleware {
    inner: Arc<dyn AgentMiddleware>,
    gate: Arc<InvocationGate>,
}
#[async_trait]
impl AgentMiddleware for GuardedMiddleware {
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn tools(&self) -> Vec<(ToolDef, Arc<dyn Tool>)> {
        self.inner
            .tools()
            .into_iter()
            .filter(|(def, _)| supported_tool(&def.name))
            .map(|(_, tool)| {
                let tool = self.gate.wrap_tool(tool);
                (tool.def(), tool)
            })
            .collect()
    }
    async fn turn_start(&self, state: &mut AgentState) -> Result<(), AgentError> {
        self.gate.check()?;
        self.inner.turn_start(state).await
    }
    async fn prepare_model(&self, state: &mut AgentState) -> Result<(), AgentError> {
        self.gate.check()?;
        self.inner.prepare_model(state).await
    }
    async fn around_model(
        &self,
        request: ModelRequest,
        next: Next<'_>,
    ) -> Result<ModelResponse, AgentError> {
        self.gate.check()?;
        let response = self.inner.around_model(request, next).await;
        self.gate.check()?;
        response
    }
    async fn review_model(&self, state: &mut AgentState) -> Result<Flow, AgentError> {
        self.gate.check()?;
        self.inner.review_model(state).await
    }
    async fn around_tool(
        &self,
        call: ToolCall,
        ctx: &mut ToolCtx<'_>,
        next: ToolNext<'_>,
    ) -> Result<ToolOutcome, AgentError> {
        // In children, profile middleware precedes host child_middleware.
        // Reject BEFORE a profile hook can perform any tool-side effect, while
        // terminal wrappers still recheck transformed tool names/arguments.
        match self.gate.denied(&call.name, &call.args) {
            Some(outcome) => Ok(outcome),
            None => {
                let outcome = self.inner.around_tool(call, ctx, next).await?;
                self.gate.observe_outcome(&outcome);
                Ok(outcome)
            }
        }
    }
    async fn turn_end(&self, state: &mut AgentState) -> Result<(), AgentError> {
        self.gate.check()?;
        self.inner.turn_end(state).await
    }
}
