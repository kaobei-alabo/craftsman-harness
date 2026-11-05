//! Actual-builder-only views for CONCRETE built-in todo/task/message producers.
//! Not a universal post-budget scrub, registry/inbox/state/provider serializer,
//! public exec, callable gadget registration, persistence or P2g cancellation.

mod views;
use crate::agent::{FrameworkScope, SubagentOutputSource};
use crate::exec::redaction::Redactor;
use crate::exec::runtime::InvocationGate;
use async_trait::async_trait;
use craftsman_harness::builtin::TodoListMiddleware;
use craftsman_harness::state::Todo;
use craftsman_harness::{
    AgentError, AgentMiddleware, AgentState, Flow, ModelRequest, ModelResponse,
    NamedMiddlewareFactory, Next, Tool, ToolCall, ToolCtx, ToolDef, ToolErrorKind, ToolMeta,
    ToolNext, ToolOutcome,
};
use serde_json::Value;
use std::sync::Arc;
use views::Fault;

pub(crate) struct Framework {
    gate: Arc<InvocationGate>,
    redactor: Redactor,
}
impl Framework {
    /// The ONLY constructor takes the actual builder's opaque lease. No caller
    /// seeds/gates/config, mutable approval, later reload or raw storage writer.
    pub(crate) fn from_scope(scope: FrameworkScope<'_>) -> Self {
        Self {
            gate: scope.gate(),
            redactor: scope.prepared().credentials().redactor().clone(),
        }
    }
    /// Concrete source construction is the witness, NOT an arbitrary middleware
    /// name/tool label. Repeated materialization shares only frozen policy/seeds;
    /// each tool context/record/stream is independent. No child tool is auto-added.
    pub(crate) fn todo_factory(self: &Arc<Self>) -> NamedMiddlewareFactory {
        let framework = self.clone();
        NamedMiddlewareFactory::new("todos", move || {
            Arc::new(KnownMiddleware {
                inner: Arc::new(TodoListMiddleware::new()),
                kind: Kind::Todos,
                framework: framework.clone(),
            })
        })
    }
    pub(crate) fn subagents(
        self: &Arc<Self>,
        source: SubagentOutputSource,
    ) -> Option<Arc<dyn AgentMiddleware>> {
        let (original, gate) = source.into_parts();
        if !Arc::ptr_eq(&self.gate, &gate) {
            return None;
        }
        Some(Arc::new(KnownMiddleware {
            inner: Arc::new(original),
            kind: Kind::Subagents,
            framework: self.clone(),
        }))
    }
    fn observe(&self, outcome: &ToolOutcome) {
        if outcome.is_error {
            if let Some(kind) = outcome.meta.error_kind {
                self.gate.observe_error_kind(kind);
            }
        }
    }
    fn fixed(&self, mut original: ToolOutcome, kind: ToolErrorKind) -> ToolOutcome {
        // Never promote success metadata (including NeedsApproval) into typed
        // failure authority when a VIEW fails. Preserve genuine original errors.
        if !original.is_error || original.meta.error_kind.is_none() {
            original.meta.error_kind = Some(kind);
        }
        original.is_error = true;
        original.content = "[REDACTED]".into();
        original.meta.truncated = true;
        self.recheck(original)
    }
    fn recheck(&self, mut outcome: ToolOutcome) -> ToolOutcome {
        if self.gate.approval_required() {
            outcome.content = "[REDACTED]".into();
            outcome.is_error = true;
            outcome.meta.error_kind = Some(ToolErrorKind::NeedsApproval);
            outcome.meta.truncated = true;
        }
        outcome
    }
    fn publish(&self, mut outcome: ToolOutcome) -> ToolOutcome {
        self.observe(&outcome); // Trusted typed facts BEFORE rendering/retention.
        if self.gate.approval_required() {
            return self.recheck(outcome);
        }
        if outcome.meta.truncated {
            // No complete original record exists: never finish a source-stage cut
            // or pretend a redacted prefix is the uncut result.
            return self.fixed(outcome, ToolErrorKind::OutputLimit);
        }
        match views::text(&self.redactor, &outcome.content) {
            Ok(view) => {
                outcome.content = view.text;
                outcome.meta.truncated |= view.truncated;
                self.recheck(outcome)
            }
            Err(fault) => self.fixed(outcome, error_kind(fault)),
        }
    }
    fn todo_result(&self, mut outcome: ToolOutcome, original: &[Todo]) -> ToolOutcome {
        match views::todos(&self.redactor, original) {
            Ok(view) => {
                outcome.content = view.text;
                self.recheck(outcome)
            }
            Err(fault) => self.fixed(outcome, error_kind(fault)),
        }
    }
    #[cfg(test)]
    pub(crate) fn publish_for_tests(&self, outcome: ToolOutcome) -> ToolOutcome {
        self.publish(outcome)
    }
    #[cfg(test)]
    pub(crate) fn recheck_for_tests(&self, outcome: ToolOutcome) -> ToolOutcome {
        self.recheck(outcome)
    }
}
fn error_kind(fault: Fault) -> ToolErrorKind {
    match fault {
        Fault::Limit => ToolErrorKind::OutputLimit,
        Fault::Unsafe => ToolErrorKind::Failed,
    }
}
fn failure(kind: ToolErrorKind) -> ToolOutcome {
    let mut meta = ToolMeta::new();
    meta.error_kind = Some(kind);
    meta.truncated = kind == ToolErrorKind::OutputLimit;
    ToolOutcome::err("[REDACTED]").with_meta(meta)
}
#[derive(Clone, Copy)]
enum Kind {
    Todos,
    Subagents,
}
struct KnownMiddleware {
    inner: Arc<dyn AgentMiddleware>,
    kind: Kind,
    framework: Arc<Framework>,
}
#[async_trait]
impl AgentMiddleware for KnownMiddleware {
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn tools(&self) -> Vec<(ToolDef, Arc<dyn Tool>)> {
        self.inner
            .tools()
            .into_iter()
            .map(|(def, original)| {
                let tool: Arc<dyn Tool> = match (self.kind, def.name.as_str()) {
                    (Kind::Todos, "write_todos") => Arc::new(TodoTool {
                        def: def.clone(),
                        framework: self.framework.clone(),
                    }),
                    (Kind::Subagents, "task" | "send_message") => Arc::new(TextTool {
                        original,
                        framework: self.framework.clone(),
                    }),
                    _ => Arc::new(Unavailable {
                        def: def.clone(),
                        framework: self.framework.clone(),
                    }),
                };
                (def, tool)
            })
            .collect()
    }
    async fn turn_start(&self, state: &mut AgentState) -> Result<(), AgentError> {
        self.inner.turn_start(state).await
    }
    async fn prepare_model(&self, state: &mut AgentState) -> Result<(), AgentError> {
        if self.framework.gate.approval_required() {
            return Err(AgentError::State("[REDACTED]".into()));
        }
        if matches!(self.kind, Kind::Todos) && views::admit_todo_context(&state.todos).is_err() {
            return Err(AgentError::State("[REDACTED]".into()));
        }
        let before = state.system_extras.len();
        self.inner.prepare_model(state).await?;
        if matches!(self.kind, Kind::Todos) {
            // Known Todo middleware appends a COMPLETE plaintext context, with
            // no own preview cut. Only these added views are selected; ORIGINAL
            // todos stay untouched. Other request/definition/state channels are
            // NOT claimed redacted by this narrow producer adapter.
            for index in before..state.system_extras.len() {
                match views::text(&self.framework.redactor, &state.system_extras[index]) {
                    Ok(view) => state.system_extras[index] = view.text,
                    Err(_) => {
                        state.system_extras.truncate(before);
                        return Err(AgentError::State("[REDACTED]".into()));
                    }
                }
            }
        }
        if self.framework.gate.approval_required() {
            state.system_extras.truncate(before);
            return Err(AgentError::State("[REDACTED]".into()));
        }
        Ok(())
    }
    async fn around_model(
        &self,
        request: ModelRequest,
        next: Next<'_>,
    ) -> Result<ModelResponse, AgentError> {
        self.inner.around_model(request, next).await
    }
    async fn review_model(&self, state: &mut AgentState) -> Result<Flow, AgentError> {
        self.inner.review_model(state).await
    }
    async fn around_tool(
        &self,
        call: ToolCall,
        ctx: &mut ToolCtx<'_>,
        next: ToolNext<'_>,
    ) -> Result<ToolOutcome, AgentError> {
        self.inner.around_tool(call, ctx, next).await
    }
    async fn turn_end(&self, state: &mut AgentState) -> Result<(), AgentError> {
        self.inner.turn_end(state).await
    }
}
struct TodoTool {
    def: ToolDef,
    framework: Arc<Framework>,
}
#[async_trait]
impl Tool for TodoTool {
    fn def(&self) -> ToolDef {
        self.def.clone()
    }
    async fn call(&self, args: Value, ctx: &mut ToolCtx<'_>) -> Result<ToolOutcome, AgentError> {
        if let Some(outcome) = self.framework.gate.denied(&self.def.name, &args) {
            return Ok(outcome);
        }
        if let Err(fault) = views::admit_todos(&args["todos"]) {
            return Ok(self.framework.recheck(failure(error_kind(fault))));
        }
        // SAME parser/typed state assignment as the concrete original producer,
        // but select original fields BEFORE its raw JSON formatter/escaping.
        let todos: Vec<Todo> = match serde_json::from_value(args["todos"].clone()) {
            Ok(todos) => todos,
            Err(_) => return Ok(self.framework.recheck(failure(ToolErrorKind::InvalidArgs))),
        };
        if let Some(outcome) = self.framework.gate.denied(&self.def.name, &args) {
            return Ok(outcome);
        }
        ctx.state.todos = todos; // Original mutation; privacy failures are not rollback.
        Ok(self
            .framework
            .todo_result(ToolOutcome::ok(""), &ctx.state.todos))
    }
}
struct TextTool {
    original: Arc<dyn Tool>,
    framework: Arc<Framework>,
}
#[async_trait]
impl Tool for TextTool {
    fn def(&self) -> ToolDef {
        self.original.def()
    }
    async fn call(&self, args: Value, ctx: &mut ToolCtx<'_>) -> Result<ToolOutcome, AgentError> {
        if let Some(outcome) = self.framework.gate.denied(&self.original.def().name, &args) {
            return Ok(outcome);
        }
        // ORIGINAL argument/delivery/task/state semantics. These concrete tools
        // return complete text before ResultBudgetsMiddleware; registry previews,
        // notifications, inboxes and child/provider state are separate producers.
        // Do not drop a foreground child and fabricate a terminal registry state.
        let outcome = match self.original.call(args, ctx).await {
            Ok(outcome) => outcome,
            Err(_) => failure(ToolErrorKind::Failed), // Never echo raw error Debug/Display.
        };
        Ok(self.framework.publish(outcome))
    }
}
struct Unavailable {
    def: ToolDef,
    framework: Arc<Framework>,
}
#[async_trait]
impl Tool for Unavailable {
    fn def(&self) -> ToolDef {
        self.def.clone()
    }
    async fn call(&self, args: Value, _: &mut ToolCtx<'_>) -> Result<ToolOutcome, AgentError> {
        let outcome = self
            .framework
            .gate
            .denied(&self.def.name, &args)
            .unwrap_or_else(|| failure(ToolErrorKind::DeniedCapability));
        Ok(self.framework.recheck(outcome))
    }
}
