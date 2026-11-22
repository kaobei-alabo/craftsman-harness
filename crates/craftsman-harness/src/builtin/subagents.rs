//! `SubAgentMiddleware`: the `task`/`send_message` tools that let a
//! main agent delegate to declared subagents.
//!
//! Each [`SubAgentSpec`] is compiled at construction into an
//! [`AgentHandle`](crate::subagent::AgentHandle) (a dedicated [`Agent`]),
//! stored by name. The middleware contributes two tools:
//!
//! * `task(description, subagent_type, run_in_background?)` — delegate a task to
//!   a subagent. Foreground (default): run to completion, merge back the child's
//!   file changes, and return its final text. Background
//!   (`run_in_background=true`): register the child in the shared
//!   [`AgentRegistry`](crate::subagent::AgentRegistry) as `Running` under a
//!   unique id (`{subagent_type}#{n}`) **before** spawning it on a Tokio task,
//!   then return immediately. When it finishes a
//!   [`Notification`](crate::subagent::Notification) is posted and the registry
//!   status flipped: `Completed`/`Done` on success, `Completed`/`Failed` on
//!   error, or `NeedsApproval`/`Suspended` if a gated tool paused the child.
//! * `send_message(to, message)` — post a free-form inter-agent message onto the
//!   notification queue, and (if `to` names a registered agent with an inbox)
//!   deliver it to that agent's registry inbox.
//!
//! ## What is wired vs deferred (mailboxes)
//!
//! **Wired:** background execution; completion notifications; suspended-child
//! surfacing (a gated background subagent posts a `NeedsApproval` notification
//! and flips to `Suspended` rather than being mis-reported as failed); a
//! concurrent-background cap (foreground fallback past the cap); a per-
//! background-agent registry inbox that `send_message` delivers to; and
//! **round-boundary injection** of those inbox messages into the running
//! subagent's transcript — the spawned `run_task_with_inbox` drains the inbox
//! before each round's model call and splices each message in as a `user` turn.
//! `send_message` reports honestly whether the target inbox received the message
//! (`delivered to inbox`), so nothing is silently dropped.
//!
//! **Deferred:** mid-*round* injection. Delivery happens at round boundaries,
//! not partway through a model↔tool round already in flight, so a message that
//! arrives while the subagent is mid-round is picked up at the *next* round, not
//! spliced into the current one. Two background `task`s of the same
//! `subagent_type` are registered under distinct ids (`type#N`).

//!
//! ## On an agent with a source view
//!
//! Both tools have a view call and the middleware adds its section as parts.
//! A task then runs on an agent with a boundary of the same conversation,
//! made by the delegating agent's boundary. The description and a message
//! reach the other agent as text of the call with its source, a task's
//! result and a background notification carry the source of the message the
//! task ended with, and the inbox takes parts. Registry entries, ids,
//! statuses, progress labels, the limit and every recorded text are the
//! same. Nothing is handed over as recorded text: where the conversation has
//! no source for a piece, the task is not started.

use crate::Agent;
use crate::builtin::HitlPolicyMiddleware;
use crate::error::AgentError;
use crate::middleware::AgentMiddleware;
use crate::model::{ModelProvider, ModelRequest, ModelResponse, ToolDef};
use crate::provenance::{
    ModelDelivery, ModelViewBoundary, OutcomePart, PartFrom, SourcePart, SourceRef,
    TrackedDispatch, TrackedModelRequest, TrackedPrepare, ViewSupport,
};
use crate::state::AgentState;
use crate::subagent::{
    AgentEntry, AgentHandle, AgentStatus, Notification, NotificationKind, SubAgentSpec, TaskResult,
};
use crate::tool::{Tool, ToolCtx, ToolOutcome};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// Default cap on concurrently-running delegated agents (foreground or background).
const DEFAULT_MAX_BACKGROUND: usize = 5;

/// Description of the `task` tool.
// PROVENANCE(P5): verbatim (a prefix) from deepagents 0.3.8 TASK_TOOL_DESCRIPTION (middleware/subagents.py);
// see docs/oss-review.md 5.1 #3. Rewritten in P5.
const TASK_TOOL_DESCRIPTION: &str = "Launch an ephemeral subagent to handle complex, multi-step independent tasks with isolated context windows.\n\nAvailable agent types and the tools they have access to:\n{available_agents}\n\nWhen using the Task tool, you must specify a subagent_type parameter to select which agent type to use.\n\n## Usage notes:\n1. Launch multiple agents concurrently whenever possible, to maximize performance; to do that, use a single message with multiple tool uses\n2. When the agent is done, it will return a single message back to you. The result returned by the agent is not visible to the user. To show the user the result, you should send a text message back to the user with a concise summary of the result.\n3. Each agent invocation is stateless. You will not be able to send additional messages to the agent, nor will the agent be able to communicate with you outside of its final report. Therefore, your prompt should contain a highly detailed task description for the agent to perform autonomously and you should specify exactly what information the agent should return back to you in its final and only message to you.\n4. The agent's outputs should generally be trusted\n5. Clearly tell the agent whether you expect it to create content, perform analysis, or just do research (search, file reads, web fetches, etc.), since it is not aware of the user's intent";

/// Shared inner state, held by the middleware and by both tools it contributes.
struct AbortOnDrop<T> {
    handle: Option<tokio::task::JoinHandle<T>>,
}

impl<T> AbortOnDrop<T> {
    fn new(handle: tokio::task::JoinHandle<T>) -> Self {
        Self {
            handle: Some(handle),
        }
    }

    async fn join(&mut self) -> Result<T, tokio::task::JoinError> {
        self.handle.as_mut().expect("join handle present").await
    }
}

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
    }
}

struct SubAgentInner {
    /// Host-supplied middleware shared with each fresh child. Per-child state
    /// belongs in AgentState rather than mutable fields of these handles.
    child_middleware: Vec<Arc<dyn AgentMiddleware>>,
    /// name → spec. A FRESH `Agent` handle is compiled per `task`
    /// invocation (see [`compile`]) — a shared compiled handle would make
    /// concurrent same-type tasks share one state/inbox and corrupt each other.
    specs: HashMap<String, Arc<SubAgentSpec>>,
    /// This middleware's host agent name, used as the `from` on notifications.
    agent_name: String,
    /// Cap on concurrently-running delegated agents. The historical field and
    /// builder name retain `background` for API compatibility.
    max_background: usize,
    /// Monotonic counter minting a unique registry id per background spawn, so
    /// two background `task`s of the same `subagent_type` don't collide on one
    /// registry key.
    spawn_seq: AtomicUsize,
    /// Each spec's system text as one owned piece, for a delegated agent
    /// with a source view.
    prompts: Vec<(String, Arc<str>)>,
    /// The section this middleware adds to the system text, as parts. It is
    /// built once, so that every round hands over the same owned pieces.
    section: OnceLock<Vec<SourcePart>>,
}

/// A fixed piece of this module's own text, as one owned object.
fn piece(cell: &'static OnceLock<Arc<str>>, text: &'static str) -> Arc<str> {
    cell.get_or_init(|| Arc::from(text)).clone()
}

/// What a spawned task runs as: one type for every such task.
type ErasedTask = std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>;

impl SubAgentInner {
    /// Backtick-wrapped, comma-joined, sorted subagent names — for prompts and
    /// error messages.
    fn allowed(&self) -> String {
        let mut names: Vec<&str> = self.specs.keys().map(|s| s.as_str()).collect();
        names.sort_unstable();
        names
            .iter()
            .map(|n| format!("`{n}`"))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Middleware that provides the `task` and `send_message` tools over a set of
/// declared subagents.
pub struct SubAgentMiddleware {
    inner: Arc<SubAgentInner>,
}

struct ProgressModel {
    inner: Arc<dyn ModelProvider>,
    registry: Arc<crate::subagent::AgentRegistry>,
    task_id: String,
    round: AtomicUsize,
}

#[async_trait]
impl ModelProvider for ProgressModel {
    async fn complete(&self, req: ModelRequest) -> Result<ModelResponse, AgentError> {
        let round = self.round.fetch_add(1, Ordering::Relaxed) + 1;
        self.registry
            .set_progress(&self.task_id, format!("thinking · round {round}"));
        self.inner.complete(req).await
    }

    /// The same label, and the provider's own view path.
    async fn complete_with_view(
        &self,
        req: TrackedModelRequest,
    ) -> Result<ModelDelivery, AgentError> {
        let round = self.round.fetch_add(1, Ordering::Relaxed) + 1;
        self.registry
            .set_progress(&self.task_id, format!("thinking · round {round}"));
        self.inner.complete_with_view(req).await
    }
}

struct ProgressTool {
    inner: Arc<dyn Tool>,
    registry: Arc<crate::subagent::AgentRegistry>,
    task_id: String,
}

#[async_trait]
impl Tool for ProgressTool {
    fn def(&self) -> ToolDef {
        self.inner.def()
    }

    async fn call(
        &self,
        args: serde_json::Value,
        ctx: &mut ToolCtx<'_>,
    ) -> Result<ToolOutcome, AgentError> {
        let name = self.inner.def().name;
        self.registry
            .set_progress(&self.task_id, format!("using {name}"));
        let outcome = self.inner.call(args, ctx).await;
        self.registry
            .set_progress(&self.task_id, format!("reviewing {name} result"));
        outcome
    }

    /// The same labels, and the tool's own view call.
    async fn call_with_view(
        &self,
        args: serde_json::Value,
        ctx: &mut ToolCtx<'_>,
        tracked: &TrackedDispatch,
    ) -> Result<ToolOutcome, AgentError> {
        let name = self.inner.def().name;
        self.registry
            .set_progress(&self.task_id, format!("using {name}"));
        let outcome = self.inner.call_with_view(args, ctx, tracked).await;
        self.registry
            .set_progress(&self.task_id, format!("reviewing {name} result"));
        outcome
    }
}

fn task_preview(description: &str) -> String {
    let one_line = description.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut preview = one_line.chars().take(100).collect::<String>();
    if one_line.chars().count() > 100 {
        preview.push('…');
    }
    if preview.is_empty() {
        "starting".to_string()
    } else {
        format!("starting · {preview}")
    }
}

/// Compile `spec` into a fresh, single-use [`Agent`] handle. Called once
/// per `task` invocation: each running task gets its own state, inbox, and
/// pending-suspension slots, so concurrent (or sequential) tasks of the same
/// type can never observe each other's half-written state or steal each
/// other's mailbox receiver.
fn compile(
    spec: &SubAgentSpec,
    task_id: &str,
    registry: Arc<crate::subagent::AgentRegistry>,
    child_middleware: &[Arc<dyn AgentMiddleware>],
) -> Arc<dyn AgentHandle> {
    Arc::new(child(spec, task_id, registry, child_middleware, None))
}

/// The agent of one task. With `view`, it is an agent with that source view
/// boundary: its system text is the boundary's, and it has none of its own.
/// Everything else is the same.
fn child(
    spec: &SubAgentSpec,
    task_id: &str,
    registry: Arc<crate::subagent::AgentRegistry>,
    child_middleware: &[Arc<dyn AgentMiddleware>],
    view: Option<Arc<dyn ModelViewBoundary>>,
) -> Agent {
    let model = spec
        .model
        .clone()
        .expect("subagent needs a model (inheritance not yet supported)");
    let model = Arc::new(ProgressModel {
        inner: model,
        registry: registry.clone(),
        task_id: task_id.to_string(),
        round: AtomicUsize::new(0),
    });
    let mut builder = Agent::builder(model)
        .name(spec.name.clone())
        .registry(registry.clone());
    builder = match view {
        Some(boundary) => builder.model_view_boundary(Some(boundary)),
        None => builder.system_prompt(spec.system_prompt.clone()),
    };
    if let Some(turn) = crate::TurnCancellation::current() {
        builder = builder
            .interrupt_model_on_cancel(true)
            .cancellation_session(turn.session());
    }
    for t in &spec.tools {
        builder = builder.tool(Arc::new(ProgressTool {
            inner: t.clone(),
            registry: registry.clone(),
            task_id: task_id.to_string(),
        }));
    }
    if let Some(profile) = &spec.harness_profile {
        // A task invocation gets freshly materialized profile middleware.
        // Exclusions apply after materialization, matching the main stack.
        for middleware in profile.materialize_extra_middleware() {
            if !profile
                .config
                .excluded_middleware
                .contains(middleware.name())
            {
                builder = builder.middleware(middleware);
            }
        }
        builder = builder.harness_profile(profile.clone());
    }
    for middleware in child_middleware {
        builder = builder.middleware(middleware.clone());
    }
    if !spec.interrupt_on.is_empty() {
        builder = builder.middleware(Arc::new(HitlPolicyMiddleware::new(
            spec.interrupt_on.clone(),
            spec.name.clone(),
        )));
    }
    builder.build()
}

impl SubAgentMiddleware {
    /// Store each spec; a fresh [`Agent`] handle is compiled per `task`
    /// invocation so concurrent tasks never share state. The host agent name
    /// defaults to `"agent"`; override with
    /// [`for_agent`](SubAgentMiddleware::for_agent).
    pub fn new(specs: Vec<SubAgentSpec>) -> Self {
        let specs: HashMap<String, Arc<SubAgentSpec>> = specs
            .into_iter()
            .map(|s| (s.name.clone(), Arc::new(s)))
            .collect();
        let mut prompts = Vec::with_capacity(specs.len());
        for (name, spec) in &specs {
            prompts.push((name.clone(), Arc::from(spec.system_prompt.as_str())));
        }
        Self {
            inner: Arc::new(SubAgentInner {
                child_middleware: Vec::new(),
                specs,
                agent_name: "agent".into(),
                max_background: DEFAULT_MAX_BACKGROUND,
                spawn_seq: AtomicUsize::new(0),
                prompts,
                section: OnceLock::new(),
            }),
        }
    }

    /// Install host-supplied middleware in every fresh delegated agent. This
    /// does not alter the subagent specification or its tool/permission set.
    pub fn child_middleware(mut self, middleware: Arc<dyn AgentMiddleware>) -> Self {
        let inner = Arc::get_mut(&mut self.inner)
            .expect("child_middleware must be called before the middleware is shared");
        inner.child_middleware.push(middleware);
        self
    }

    /// Set the host agent name used as the `from` field on notifications.
    pub fn for_agent(mut self, name: impl Into<String>) -> Self {
        let inner = Arc::get_mut(&mut self.inner)
            .expect("for_agent must be called before the middleware is shared");
        inner.agent_name = name.into();
        self
    }

    /// Override the concurrent delegated-agent cap (default 5).
    pub fn max_background(mut self, cap: usize) -> Self {
        let inner = Arc::get_mut(&mut self.inner)
            .expect("max_background must be called before the middleware is shared");
        inner.max_background = cap.max(1);
        self
    }
}

#[async_trait]
impl AgentMiddleware for SubAgentMiddleware {
    fn name(&self) -> &str {
        "subagents"
    }

    fn tools(&self) -> Vec<(ToolDef, Arc<dyn Tool>)> {
        let task = Arc::new(TaskTool {
            inner: self.inner.clone(),
        });
        let send = Arc::new(SendMessageTool {
            inner: self.inner.clone(),
        });
        vec![
            (task.def(), task as Arc<dyn Tool>),
            (send.def(), send as Arc<dyn Tool>),
        ]
    }

    async fn prepare_model(&self, st: &mut AgentState) -> Result<(), AgentError> {
        if self.inner.specs.is_empty() {
            return Ok(());
        }
        let mut listing: Vec<(&str, &str)> = self
            .inner
            .specs
            .iter()
            .map(|(name, spec)| (name.as_str(), spec.description.as_str()))
            .collect();
        listing.sort_by(|a, b| a.0.cmp(b.0));
        let mut available = String::new();
        for (name, desc) in listing {
            available.push_str(&format!("- {name}: {desc}\n"));
        }
        let mut section = TASK_TOOL_DESCRIPTION.replace("{available_agents}", available.trim_end());
        section.push_str(&format!(
            "\n6. No more than {} delegated agents may run concurrently; wait for a running task to finish before launching another.",
            self.inner.max_background
        ));
        st.system_extras.push(section);
        Ok(())
    }

    /// On an agent with a source view the same section is added as parts,
    /// and both tools take their view calls. A task runs on an agent with a
    /// boundary of the same conversation, so every middleware a delegated
    /// agent would get must have made its own statement: without that, this
    /// middleware makes none.
    fn view_support(&self) -> ViewSupport {
        let stated = |middleware: &Arc<dyn AgentMiddleware>| {
            matches!(
                middleware.view_support(),
                ViewSupport::Preserving | ViewSupport::Tracked
            )
        };
        for middleware in &self.inner.child_middleware {
            if !stated(middleware) {
                return ViewSupport::Unadapted;
            }
        }
        for spec in self.inner.specs.values() {
            let Some(profile) = &spec.harness_profile else {
                continue;
            };
            for middleware in profile.materialize_extra_middleware() {
                if !profile
                    .config
                    .excluded_middleware
                    .contains(middleware.name())
                    && !stated(&middleware)
                {
                    return ViewSupport::Unadapted;
                }
            }
        }
        ViewSupport::Tracked
    }

    async fn prepare_model_with_view(
        &self,
        prepare: &mut TrackedPrepare<'_>,
    ) -> Result<(), AgentError> {
        if self.inner.specs.is_empty() {
            return Ok(());
        }
        let section = self.inner.section.get_or_init(|| {
            // The text of `prepare_model`, piece by piece: the application's
            // own words, and what the configuration says about each agent
            // and about the limit.
            let mut listing: Vec<(&str, &str)> = self
                .inner
                .specs
                .iter()
                .map(|(name, spec)| (name.as_str(), spec.description.as_str()))
                .collect();
            listing.sort_by(|a, b| a.0.cmp(b.0));
            let mut available = String::new();
            for (name, desc) in listing {
                available.push_str(&format!("- {name}: {desc}\n"));
            }
            let (before, after) = TASK_TOOL_DESCRIPTION
                .split_once("{available_agents}")
                .unwrap_or((TASK_TOOL_DESCRIPTION, ""));
            vec![
                SourcePart::fixed(Arc::from(before)),
                SourcePart::read("delegated agents", Arc::from(available.trim_end())),
                SourcePart::fixed(Arc::from(format!("{after}\n6. No more than "))),
                SourcePart::read(
                    "delegated agent limit",
                    Arc::from(self.inner.max_background.to_string()),
                ),
                SourcePart::fixed(Arc::from(
                    " delegated agents may run concurrently; wait for a running task to finish before launching another.",
                )),
            ]
        });
        prepare.add_system(section.clone())
    }
}

// ---- the `task` tool ---------------------------------------------------------

struct TaskTool {
    inner: Arc<SubAgentInner>,
}

#[async_trait]
impl Tool for TaskTool {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "task".into(),
            description: format!(
                "Launch an ephemeral subagent to handle a complex, multi-step independent task with an isolated context window. Provide a detailed, autonomous task description and the subagent_type to use. At most {} delegated agents may run concurrently.",
                self.inner.max_background
            ),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "description": {
                        "type": "string",
                        "description": "A highly detailed, autonomous task description for the subagent to perform, including exactly what to return."
                    },
                    "subagent_type": {
                        "type": "string",
                        "description": "The type of subagent to delegate to."
                    },
                    "run_in_background": {
                        "type": "boolean",
                        "description": "If true, run the subagent in the background and return immediately; you will be notified when it completes."
                    }
                },
                "required": ["description", "subagent_type"]
            }),
        }
    }

    async fn call(
        &self,
        args: serde_json::Value,
        ctx: &mut ToolCtx<'_>,
    ) -> Result<ToolOutcome, AgentError> {
        if crate::TurnCancellation::current().is_some_and(|turn| turn.is_cancelled()) {
            let mut meta = crate::ToolMeta::new();
            meta.error_kind = Some(crate::ToolErrorKind::Cancelled);
            return Ok(ToolOutcome::err("Task cancelled before startup.").with_meta(meta));
        }
        let description = args
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let subagent_type = args
            .get("subagent_type")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let run_in_background = args
            .get("run_in_background")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let Some(spec) = self.inner.specs.get(&subagent_type) else {
            return Ok(ToolOutcome::ok(format!(
                "We cannot invoke subagent {subagent_type} because it does not exist, the only allowed types are {}",
                self.inner.allowed()
            )));
        };
        // Hard concurrency cap across both foreground and background delegated
        // tasks. Returning an error is safer than the previous foreground
        // fallback, which could exceed the configured parallelization limit.
        let running = ctx
            .registry
            .list()
            .iter()
            .filter(|(_, status)| *status == AgentStatus::Running)
            .count();
        if running >= self.inner.max_background {
            return Ok(ToolOutcome::err(format!(
                "delegated-agent capacity reached ({running}/{} running); wait for an existing task to finish before starting another",
                self.inner.max_background
            )));
        }

        // Every foreground/background invocation gets a stable task id so the
        // registry and UI can observe it while the parent awaits the child.
        let n = self.inner.spawn_seq.fetch_add(1, Ordering::Relaxed);
        let unique_id = format!("{subagent_type}#{n}");
        // Fresh handle per invocation: own state/inbox/pending, never shared.
        let handle = compile(
            spec,
            &unique_id,
            ctx.registry.clone(),
            &self.inner.child_middleware,
        );

        // The child inherits the parent's state (files/custom); `run_task`
        // clears messages/todos/system_extras itself.
        let inherited = ctx.state.clone();

        if run_in_background {
            return Ok(self.spawn_background(
                &subagent_type,
                unique_id,
                description,
                inherited,
                handle,
                ctx,
            ));
        }

        Ok(self
            .run_foreground(
                &subagent_type,
                &unique_id,
                &description,
                inherited,
                handle,
                ctx,
            )
            .await)
    }

    /// The same task on an agent with a source view. The delegated agent
    /// gets a boundary of the same conversation, the description goes to it
    /// as the text the call wrote, and what it ends with comes back with its
    /// source. Nothing is passed on as recorded text: where the conversation
    /// has no source for a piece, the task is not started.
    async fn call_with_view(
        &self,
        args: serde_json::Value,
        ctx: &mut ToolCtx<'_>,
        tracked: &TrackedDispatch,
    ) -> Result<ToolOutcome, AgentError> {
        let Some(boundary) = tracked.boundary() else {
            return Ok(ToolOutcome::err(
                "This conversation has no source for the reply that asked for this task.",
            ));
        };
        if crate::TurnCancellation::current().is_some_and(|turn| turn.is_cancelled()) {
            let mut meta = crate::ToolMeta::new();
            meta.error_kind = Some(crate::ToolErrorKind::Cancelled);
            return Ok(ToolOutcome::err("Task cancelled before startup.").with_meta(meta));
        }
        let described = args.get("description").and_then(|v| v.as_str());
        let description = described.unwrap_or("").to_string();
        let named = args.get("subagent_type").and_then(|v| v.as_str());
        let subagent_type = named.unwrap_or("").to_string();
        let run_in_background = args
            .get("run_in_background")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let Some(spec) = self.inner.specs.get(&subagent_type) else {
            static OPEN: OnceLock<Arc<str>> = OnceLock::new();
            static CLOSE: OnceLock<Arc<str>> = OnceLock::new();
            let allowed = self.inner.allowed();
            let outcome = ToolOutcome::ok(format!(
                "We cannot invoke subagent {subagent_type} because it does not exist, the only allowed types are {allowed}"
            ));
            // The name is the call's own text; the rest is the tool's.
            let mut parts = vec![OutcomePart::Fixed(piece(
                &OPEN,
                "We cannot invoke subagent ",
            ))];
            if named.is_some() {
                parts.push(OutcomePart::Argument {
                    pointer: "/subagent_type".into(),
                    text: Arc::from(subagent_type.as_str()),
                });
            }
            parts.push(OutcomePart::Fixed(piece(
                &CLOSE,
                " because it does not exist, the only allowed types are ",
            )));
            parts.push(OutcomePart::Read {
                what: "delegated agents".into(),
                text: Arc::from(allowed),
            });
            tracked.outcome_is(parts);
            return Ok(outcome);
        };
        let running = ctx
            .registry
            .list()
            .iter()
            .filter(|(_, status)| *status == AgentStatus::Running)
            .count();
        if running >= self.inner.max_background {
            return Ok(ToolOutcome::err(format!(
                "delegated-agent capacity reached ({running}/{} running); wait for an existing task to finish before starting another",
                self.inner.max_background
            )));
        }

        let n = self.inner.spawn_seq.fetch_add(1, Ordering::Relaxed);
        let unique_id = format!("{subagent_type}#{n}");
        // The description is the text this call wrote. A call without one
        // starts the task with an empty input, as it always did.
        static EMPTY: OnceLock<Arc<str>> = OnceLock::new();
        let input = match described.and_then(|text| tracked.argument("/description", text.into())) {
            Some(part) => boundary.argument_source(&part).await?,
            None => {
                boundary
                    .host_message(&SourcePart::fixed(piece(&EMPTY, "")))
                    .await?
            }
        };
        // The delegated agent's own system text, kept apart from this
        // agent's, as host text read from its definition.
        let mut system = Vec::new();
        for (name, prompt) in &self.inner.prompts {
            if *name == subagent_type && !prompt.is_empty() {
                system.push(SourcePart::read("delegated agent prompt", prompt.clone()));
            }
        }
        let view = boundary.child_boundary(&system).await?;
        let agent = Arc::new(child(
            spec,
            &unique_id,
            ctx.registry.clone(),
            &self.inner.child_middleware,
            Some(view),
        ));
        let inherited = ctx.state.clone();

        if run_in_background {
            return Ok(self.spawn_background_with_view(
                &subagent_type,
                TaskWithView {
                    unique_id,
                    description,
                    input,
                    inherited,
                    agent,
                },
                ctx,
            ));
        }

        ctx.registry.insert(&unique_id, AgentStatus::Running);
        ctx.registry
            .set_progress(&unique_id, task_preview(&description));
        let (result, reply) = match agent
            .run_task_with_view(&description, input, inherited, None)
            .await
        {
            Ok(task) => (Ok(task.result), task.reply),
            Err(error) => (Err(error), None),
        };
        if let (Ok(TaskResult::Done(outcome)), Some(reply)) = (&result, reply) {
            // The final text is the text of the message the delegated agent
            // ended with, and comes back as that.
            tracked.outcome_is(vec![OutcomePart::Source {
                source: reply,
                text: Arc::from(outcome.final_text.as_str()),
            }]);
        }
        Ok(foreground_end(&subagent_type, &unique_id, result, ctx))
    }
}

/// One task that is about to run on an agent with a source view.
struct TaskWithView {
    unique_id: String,
    description: String,
    /// The source of the description, which is the agent's input.
    input: SourceRef,
    inherited: AgentState,
    agent: Arc<Agent>,
}

/// What a foreground task of an agent with a source view comes back as:
/// the bookkeeping and the outcome of `run_foreground`.
fn foreground_end(
    subagent_type: &str,
    task_id: &str,
    result: Result<TaskResult, AgentError>,
    ctx: &mut ToolCtx<'_>,
) -> ToolOutcome {
    match result {
        Ok(TaskResult::Done(outcome)) => {
            let cancelled =
                crate::TurnCancellation::current().is_some_and(|turn| turn.is_cancelled());
            ctx.registry
                .set_progress(task_id, if cancelled { "cancelled" } else { "completed" });
            ctx.registry.set_status(
                task_id,
                if cancelled {
                    AgentStatus::Failed
                } else {
                    AgentStatus::Done
                },
            );
            for (k, v) in outcome.state_updates.files {
                ctx.state.files.insert(k, v);
            }
            if cancelled {
                let mut meta = crate::ToolMeta::new();
                meta.error_kind = Some(crate::ToolErrorKind::Cancelled);
                ToolOutcome::err(outcome.final_text).with_meta(meta)
            } else {
                ToolOutcome::ok(outcome.final_text)
            }
        }
        Ok(TaskResult::Suspended(_)) => {
            ctx.registry.set_progress(task_id, "waiting for approval");
            ctx.registry.set_status(task_id, AgentStatus::Suspended);
            ToolOutcome::err(format!(
                "subagent {subagent_type} requires approval for a gated tool; \
                 run it in the background (run_in_background=true) so the approval \
                 can be surfaced, or approve interactively (not yet supported foreground)"
            ))
        }
        Err(e) => {
            ctx.registry.set_progress(task_id, format!("failed · {e}"));
            ctx.registry.set_status(task_id, AgentStatus::Failed);
            ToolOutcome::err(format!("subagent {subagent_type} failed: {e}"))
        }
    }
}

/// How a background task of an agent with a source view ended: the kind of
/// its notification, the registry status, and the parts the notification's
/// text is made of. `joined` is `Err(panicked)` when the task did not return.
fn background_end(
    joined: Result<Result<crate::agent::TrackedTask, AgentError>, bool>,
    cancelled: bool,
    spawn_name: &str,
) -> (NotificationKind, AgentStatus, Option<Vec<SourcePart>>) {
    let failed = |text: String| {
        // The host's own account of a failure.
        let parts = vec![SourcePart::read(
            "delegated agent failure",
            Arc::from(text.as_str()),
        )];
        (
            NotificationKind::Completed(text),
            AgentStatus::Failed,
            Some(parts),
        )
    };
    match joined {
        Ok(Ok(task)) => match task.result {
            TaskResult::Done(outcome) => {
                // The text of the message the agent ended with, as that.
                let parts = task.reply.map(|source| {
                    vec![SourcePart::of_source(
                        source,
                        Arc::from(outcome.final_text.as_str()),
                    )]
                });
                let status = if cancelled {
                    AgentStatus::Failed
                } else {
                    AgentStatus::Done
                };
                (
                    NotificationKind::Completed(outcome.final_text),
                    status,
                    parts,
                )
            }
            TaskResult::Suspended(suspension) => {
                let call = &suspension.request.tool_call;
                let summary = &suspension.request.summary;
                let arguments = call.args.to_string();
                // The policy gate's summary is the tool's name and the JSON
                // of the call's arguments. Those arguments are text of the
                // reply that asked for the call. Any other summary is the
                // host's own text.
                let parts = match task.reply {
                    Some(reply) if *summary == format!("{} {arguments}", call.name) => {
                        static BETWEEN: OnceLock<Arc<str>> = OnceLock::new();
                        vec![
                            SourcePart::read("tool name", Arc::from(call.name.as_str())),
                            SourcePart::fixed(piece(&BETWEEN, " ")),
                            SourcePart {
                                text: Arc::from(arguments),
                                from: PartFrom::CallJson {
                                    reply,
                                    call_id: call.id.clone(),
                                    pointer: String::new(),
                                },
                            },
                        ]
                    }
                    _ => vec![SourcePart::read(
                        "approval request",
                        Arc::from(summary.as_str()),
                    )],
                };
                (
                    NotificationKind::NeedsApproval(suspension),
                    AgentStatus::Suspended,
                    Some(parts),
                )
            }
        },
        Ok(Err(e)) => failed(format!("subagent {spawn_name} failed: {e}")),
        Err(panicked) => failed(format!(
            "subagent {spawn_name} failed: {}",
            if panicked { "panicked" } else { "cancelled" }
        )),
    }
}

impl TaskTool {
    /// Run a subagent to completion inline, merge back its file changes, and
    /// return its final text (or an error outcome).
    async fn run_foreground(
        &self,
        subagent_type: &str,
        task_id: &str,
        description: &str,
        inherited: AgentState,
        handle: Arc<dyn AgentHandle>,
        ctx: &mut ToolCtx<'_>,
    ) -> ToolOutcome {
        ctx.registry.insert(task_id, AgentStatus::Running);
        ctx.registry
            .set_progress(task_id, task_preview(description));
        match handle.run_task(description, inherited).await {
            Ok(TaskResult::Done(outcome)) => {
                let cancelled =
                    crate::TurnCancellation::current().is_some_and(|turn| turn.is_cancelled());
                ctx.registry
                    .set_progress(task_id, if cancelled { "cancelled" } else { "completed" });
                ctx.registry.set_status(
                    task_id,
                    if cancelled {
                        AgentStatus::Failed
                    } else {
                        AgentStatus::Done
                    },
                );
                for (k, v) in outcome.state_updates.files {
                    ctx.state.files.insert(k, v);
                }
                if cancelled {
                    let mut meta = crate::ToolMeta::new();
                    meta.error_kind = Some(crate::ToolErrorKind::Cancelled);
                    ToolOutcome::err(outcome.final_text).with_meta(meta)
                } else {
                    ToolOutcome::ok(outcome.final_text)
                }
            }
            // A foreground subagent hit a HITL gate. Inline approval of a nested
            // subagent's gated tool is not yet wired (Plan 3 scope), so we
            // decline rather than silently running the gated tool: the safety
            // property holds — the gated tool has NOT run.
            Ok(TaskResult::Suspended(_)) => {
                ctx.registry.set_progress(task_id, "waiting for approval");
                ctx.registry.set_status(task_id, AgentStatus::Suspended);
                ToolOutcome::err(format!(
                    "subagent {subagent_type} requires approval for a gated tool; \
                     run it in the background (run_in_background=true) so the approval \
                     can be surfaced, or approve interactively (not yet supported foreground)"
                ))
            }
            Err(e) => {
                ctx.registry.set_progress(task_id, format!("failed · {e}"));
                ctx.registry.set_status(task_id, AgentStatus::Failed);
                ToolOutcome::err(format!("subagent {subagent_type} failed: {e}"))
            }
        }
    }

    /// Spawn a subagent on a Tokio task under a UNIQUE registry id
    /// (`{subagent_type}#{n}`), register it as `Running` with an inbox
    /// **before** spawning (so a fast child's `set_status(Done)` can never race
    /// ahead of the entry existing), then return immediately. On completion the
    /// spawned task flips the registry status and posts a notification onto the
    /// shared queue the ctx was built with (so it reaches the app):
    /// `Completed`/`Done` on success, `Completed`/`Failed` on error, or
    /// `NeedsApproval`/`Suspended` if a gated tool paused the child.
    fn spawn_background(
        &self,
        subagent_type: &str,
        unique_id: String,
        description: String,
        inherited: AgentState,
        handle: Arc<dyn AgentHandle>,
        ctx: &mut ToolCtx<'_>,
    ) -> ToolOutcome {
        let registry = ctx.registry.clone();
        // The SHARED queue — its receiver is the app's, so completion reaches it.
        let notifications = ctx.notifications.clone();
        let (inbox_tx, inbox_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let cancellation = crate::TurnCancellation::current().map(|turn| turn.background());

        // FIX: seed the registry entry BEFORE spawning. `set_status` is a no-op
        // when the entry is absent, so a subagent that finishes before the entry
        // exists would otherwise leave the status stuck on `Running`.
        ctx.registry.insert_cancellable_entry(
            unique_id.clone(),
            AgentEntry {
                status: AgentStatus::Running,
                progress: task_preview(&description),
                join: None,
                inbox: Some(inbox_tx),
            },
            cancellation.clone(),
        );

        let spawn_name = unique_id.clone();
        let parent_name = self.inner.agent_name.clone();
        // Retain the outer JoinHandle in the registry so the task drawer can
        // cancel a running background agent. The child runs on an INNER task
        // whose JoinHandle the outer task awaits: a panic anywhere in the child
        // still surfaces as a join error and finalizes the registry entry.
        //
        // Both tasks are spawned as the one erased type every background
        // task of this module uses, so the inner task leaves its result in a
        // slot instead of returning it.
        let outer: ErasedTask = Box::pin(async move {
            let child_cancellation = cancellation.clone();
            let finished = Arc::new(Mutex::new(None));
            let slot = finished.clone();
            let inner: ErasedTask = Box::pin(async move {
                let work = handle.run_task_with_inbox(&description, inherited, Some(inbox_rx));
                let result = match child_cancellation {
                    Some(turn) => turn.scope(work).await,
                    None => work.await,
                };
                *slot.lock().unwrap() = Some(result);
            });
            let mut child = AbortOnDrop::new(tokio::spawn(inner));
            let joined = child.join().await.map(|()| {
                let result = finished.lock().unwrap().take();
                result.expect("a task that returned left its result")
            });
            let (kind, status) = match joined {
                Ok(Ok(TaskResult::Done(outcome)))
                    if cancellation
                        .as_ref()
                        .is_some_and(|turn| turn.is_cancelled()) =>
                {
                    (
                        NotificationKind::Completed(outcome.final_text),
                        AgentStatus::Failed,
                    )
                }
                Ok(Ok(TaskResult::Done(outcome))) => (
                    NotificationKind::Completed(outcome.final_text),
                    AgentStatus::Done,
                ),
                Ok(Ok(TaskResult::Suspended(s))) => {
                    (NotificationKind::NeedsApproval(s), AgentStatus::Suspended)
                }
                Ok(Err(e)) => (
                    NotificationKind::Completed(format!("subagent {spawn_name} failed: {e}")),
                    AgentStatus::Failed,
                ),
                // Join error: the child panicked (or was aborted).
                Err(e) => (
                    NotificationKind::Completed(format!(
                        "subagent {spawn_name} failed: {}",
                        if e.is_panic() {
                            "panicked"
                        } else {
                            "cancelled"
                        }
                    )),
                    AgentStatus::Failed,
                ),
            };
            let progress = match status {
                AgentStatus::Done => "completed".to_string(),
                AgentStatus::Failed => "failed".to_string(),
                AgentStatus::Suspended => "waiting for approval".to_string(),
                AgentStatus::Running => "running".to_string(),
            };
            let progress = if cancellation
                .as_ref()
                .is_some_and(|turn| turn.is_cancelled())
                && status == AgentStatus::Failed
            {
                "cancelled".to_string()
            } else {
                progress
            };
            registry.set_progress(&spawn_name, progress);
            registry.set_status(&spawn_name, status);
            notifications.push(Notification {
                from: spawn_name.clone(),
                to: parent_name,
                kind,
            });
        });
        let join = tokio::spawn(outer);
        ctx.registry.set_join(&unique_id, join);

        ToolOutcome::ok(format!(
            "Started background agent '{unique_id}' (type: {subagent_type}). \
             You will be notified when it completes."
        ))
    }
}

impl TaskTool {
    /// [`spawn_background`](Self::spawn_background) for an agent with a
    /// source view: the same registration before the spawn, the same two
    /// tasks, the same statuses and the same notification. The inbox takes
    /// parts, and the notification carries the parts its text is made of.
    fn spawn_background_with_view(
        &self,
        subagent_type: &str,
        task: TaskWithView,
        ctx: &mut ToolCtx<'_>,
    ) -> ToolOutcome {
        let TaskWithView {
            unique_id,
            description,
            input,
            inherited,
            agent,
        } = task;
        let registry = ctx.registry.clone();
        let notifications = ctx.notifications.clone();
        let inbox = crate::subagent::ViewInbox::default();
        let cancellation = crate::TurnCancellation::current().map(|turn| turn.background());
        ctx.registry.insert_cancellable_entry(
            unique_id.clone(),
            AgentEntry {
                status: AgentStatus::Running,
                progress: task_preview(&description),
                join: None,
                inbox: None,
            },
            cancellation.clone(),
        );
        ctx.registry.set_view_inbox(&unique_id, inbox.clone());

        let spawn_name = unique_id.clone();
        let parent_name = self.inner.agent_name.clone();
        // The task's result travels in a slot, so that both tasks are of the
        // one erased type.
        let finished = Arc::new(Mutex::new(None));
        let slot = finished.clone();
        let child_cancellation = cancellation.clone();
        let work: ErasedTask = Box::pin(async move {
            let work = agent.run_task_with_view(&description, input, inherited, Some(inbox));
            let result = match child_cancellation {
                Some(turn) => turn.scope(work).await,
                None => work.await,
            };
            *slot.lock().unwrap() = Some(result);
        });
        let outer: ErasedTask = Box::pin(async move {
            let mut child = AbortOnDrop::new(tokio::spawn(work));
            let joined = match child.join().await {
                // A task that returned left its result in the slot.
                Ok(()) => finished.lock().unwrap().take().ok_or(false),
                Err(e) => Err(e.is_panic()),
            };
            let cancelled = cancellation
                .as_ref()
                .is_some_and(|turn| turn.is_cancelled());
            let (kind, status, parts) = background_end(joined, cancelled, &spawn_name);
            let progress = match status {
                AgentStatus::Failed if cancelled => "cancelled",
                AgentStatus::Done => "completed",
                AgentStatus::Failed => "failed",
                AgentStatus::Suspended => "waiting for approval",
                AgentStatus::Running => "running",
            };
            registry.set_progress(&spawn_name, progress);
            registry.set_status(&spawn_name, status);
            let notification = Notification {
                from: spawn_name.clone(),
                to: parent_name,
                kind,
            };
            match parts {
                Some(parts) => notifications.push_with_parts(notification, parts),
                None => notifications.push(notification),
            }
        });
        let join = tokio::spawn(outer);
        ctx.registry.set_join(&unique_id, join);

        ToolOutcome::ok(format!(
            "Started background agent '{unique_id}' (type: {subagent_type}). \
             You will be notified when it completes."
        ))
    }
}

// ---- the `send_message` tool -------------------------------------------------

struct SendMessageTool {
    inner: Arc<SubAgentInner>,
}

#[async_trait]
impl Tool for SendMessageTool {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "send_message".into(),
            description: "Send a free-form message to another agent (e.g. a running background subagent). The message is queued for the recipient.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "to": {"type": "string", "description": "The name of the recipient agent."},
                    "message": {"type": "string", "description": "The message content to deliver."}
                },
                "required": ["to", "message"]
            }),
        }
    }

    async fn call(
        &self,
        args: serde_json::Value,
        ctx: &mut ToolCtx<'_>,
    ) -> Result<ToolOutcome, AgentError> {
        let to = args
            .get("to")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let message = args
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        // Post onto the shared notification queue (peer/app observation).
        ctx.notifications.push(Notification {
            from: self.inner.agent_name.clone(),
            to: to.clone(),
            kind: NotificationKind::Message(message.clone()),
        });

        // If `to` names an agent with a registry inbox, also deliver there
        // (consumed at its next run — see module doc on deferred injection).
        if ctx.registry.deliver(&to, message) {
            Ok(ToolOutcome::ok(format!(
                "message sent to {to} (delivered to inbox)"
            )))
        } else {
            Ok(ToolOutcome::ok(format!("message sent to {to}")))
        }
    }

    /// The same message on an agent with a source view. It is posted and
    /// delivered as the text this call wrote, with its source, and only to
    /// an inbox that takes parts: the recorded text alone goes nowhere.
    async fn call_with_view(
        &self,
        args: serde_json::Value,
        ctx: &mut ToolCtx<'_>,
        tracked: &TrackedDispatch,
    ) -> Result<ToolOutcome, AgentError> {
        let Some(boundary) = tracked.boundary() else {
            return Ok(ToolOutcome::err(
                "This conversation has no source for the reply that asked for this message.",
            ));
        };
        let named = args.get("to").and_then(|v| v.as_str());
        let to = named.unwrap_or("").to_string();
        let written = args.get("message").and_then(|v| v.as_str());
        let message = written.unwrap_or("").to_string();
        static EMPTY: OnceLock<Arc<str>> = OnceLock::new();
        let part = match written {
            Some(text) => {
                let text: Arc<str> = Arc::from(text);
                match tracked.argument("/message", text.clone()) {
                    Some(part) => {
                        SourcePart::of_source(boundary.argument_source(&part).await?, text)
                    }
                    None => SourcePart::fixed(piece(&EMPTY, "")),
                }
            }
            None => SourcePart::fixed(piece(&EMPTY, "")),
        };
        ctx.notifications.push_with_parts(
            Notification {
                from: self.inner.agent_name.clone(),
                to: to.clone(),
                kind: NotificationKind::Message(message),
            },
            vec![part.clone()],
        );
        let delivered = ctx.registry.deliver_parts(&to, vec![part]);

        // The recipient's name is the call's own text; the rest is the
        // tool's.
        static SENT: OnceLock<Arc<str>> = OnceLock::new();
        static DELIVERED: OnceLock<Arc<str>> = OnceLock::new();
        let mut parts = vec![OutcomePart::Fixed(piece(&SENT, "message sent to "))];
        if named.is_some() {
            parts.push(OutcomePart::Argument {
                pointer: "/to".into(),
                text: Arc::from(to.as_str()),
            });
        }
        let outcome = if delivered {
            parts.push(OutcomePart::Fixed(piece(
                &DELIVERED,
                " (delivered to inbox)",
            )));
            ToolOutcome::ok(format!("message sent to {to} (delivered to inbox)"))
        } else {
            ToolOutcome::ok(format!("message sent to {to}"))
        };
        tracked.outcome_is(parts);
        Ok(outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subagent::{AgentStatus, NotificationKind, SubAgentSpec, notification_channel};
    use crate::testing::ScriptedModel;
    use crate::{Agent, Msg, TurnResult};
    use std::sync::Arc;
    use std::time::Duration;

    fn gp_spec(model: Arc<dyn crate::model::ModelProvider>) -> SubAgentSpec {
        SubAgentSpec {
            name: "general-purpose".into(),
            description: "gp".into(),
            system_prompt: "you search".into(),
            tools: vec![],
            model: Some(model),
            harness_profile: None,
            interrupt_on: vec![],
            max_depth: 1,
        }
    }

    #[test]
    fn default_and_overridden_parallel_caps_are_stable() {
        let default = SubAgentMiddleware::new(Vec::new());
        assert_eq!(default.inner.max_background, 5);
        let overridden = SubAgentMiddleware::new(Vec::new()).max_background(9);
        assert_eq!(overridden.inner.max_background, 9);
        let zero = SubAgentMiddleware::new(Vec::new()).max_background(0);
        assert_eq!(zero.inner.max_background, 1);
    }

    #[tokio::test]
    async fn parallel_cap_rejects_new_foreground_or_background_tasks() {
        let main_model = Arc::new(ScriptedModel::new(vec![
            Msg::Assistant {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall {
                    id: "c1".into(),
                    name: "task".into(),
                    args: serde_json::json!({
                        "description":"extra work",
                        "subagent_type":"general-purpose"
                    }),
                }],
            },
            Msg::assistant("capacity acknowledged"),
        ]));
        let sub = gp_spec(Arc::new(ScriptedModel::new(vec![Msg::assistant(
            "should not run",
        )])));
        let registry = Arc::new(crate::subagent::AgentRegistry::default());
        registry.insert("already-running", AgentStatus::Running);
        let agent = Agent::builder(main_model)
            .name("main")
            .registry(registry)
            .middleware(Arc::new(
                SubAgentMiddleware::new(vec![sub])
                    .for_agent("main")
                    .max_background(1),
            ))
            .build();

        agent.run_turn("go").await.unwrap();
        let state = agent.state.lock().unwrap();
        assert!(state.messages.iter().any(|message| matches!(
            message,
            Msg::Tool { content, is_error: true, .. }
                if content.contains("capacity reached (1/1 running)")
        )));
    }

    // A FOREGROUND subagent whose spec gates `execute` (interrupt_on) must
    // DECLINE the task when the child hits the gate — the gated tool must not
    // run, and the parent gets a clear is_error result. This is the safety
    // contract the app relies on to close the delegation HITL bypass.
    #[tokio::test]
    async fn foreground_gated_subagent_declines_instead_of_executing() {
        use crate::testing::NoopTool;

        let main_model = Arc::new(ScriptedModel::new(vec![
            Msg::Assistant {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall {
                    id: "c1".into(),
                    name: "task".into(),
                    args: serde_json::json!({"description":"run rm","subagent_type":"gated"}),
                }],
            },
            Msg::assistant("ok"),
        ]));
        let sub_model = Arc::new(ScriptedModel::new(vec![Msg::Assistant {
            content: String::new(),
            tool_calls: vec![crate::state::ToolCall {
                id: "s1".into(),
                name: "execute".into(),
                args: serde_json::json!({"command":"rm -rf /tmp/x"}),
            }],
        }]));
        let sub = SubAgentSpec {
            name: "gated".into(),
            description: "gated".into(),
            system_prompt: "you run commands".into(),
            tools: vec![Arc::new(NoopTool::new("execute"))],
            model: Some(sub_model),
            harness_profile: None,
            interrupt_on: vec!["execute".into()],
            max_depth: 1,
        };

        let agent = Agent::builder(main_model)
            .name("main")
            .middleware(Arc::new(SubAgentMiddleware::new(vec![sub])))
            .build();
        agent.run_turn("go").await.unwrap();
        let st = agent.state.lock().unwrap();
        assert!(
            st.messages
                .iter()
                .any(|m| matches!(m, Msg::Tool{is_error: true, content, ..}
                if content.contains("requires approval"))),
            "gated delegated tool must be declined, not silently executed"
        );
    }

    struct ProfileMarker;

    #[async_trait]
    impl AgentMiddleware for ProfileMarker {
        fn name(&self) -> &str {
            "profile_marker"
        }

        async fn prepare_model(&self, state: &mut AgentState) -> Result<(), AgentError> {
            state.system_extras.push("profile middleware marker".into());
            Ok(())
        }
    }

    #[tokio::test]
    async fn subagent_materializes_its_harness_middleware_per_compile() {
        let model = Arc::new(ScriptedModel::new(vec![Msg::assistant("done")]));
        let profile = crate::profiles::HarnessProfile {
            extra_middleware: vec![crate::profiles::NamedMiddlewareFactory::new(
                "profile_marker",
                || Arc::new(ProfileMarker),
            )],
            ..Default::default()
        };
        let spec = SubAgentSpec {
            name: "profiled".into(),
            description: "profiled".into(),
            system_prompt: "base".into(),
            tools: vec![],
            model: Some(model.clone()),
            harness_profile: Some(profile),
            interrupt_on: vec![],
            max_depth: 1,
        };

        let handle = compile(
            &spec,
            "general-purpose#0",
            Arc::new(crate::subagent::AgentRegistry::default()),
            &[],
        );
        handle
            .run_task("work", AgentState::default())
            .await
            .unwrap();
        let requests = model.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].system.contains("profile middleware marker"));
    }

    #[tokio::test]
    async fn task_tool_delegates_to_subagent_foreground() {
        let main_model = Arc::new(ScriptedModel::new(vec![
            Msg::Assistant {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall {
                    id: "c1".into(),
                    name: "task".into(),
                    args: serde_json::json!({"description":"find X","subagent_type":"general-purpose"}),
                }],
            },
            Msg::assistant("subagent said: found it"),
        ]));
        let sub_model = Arc::new(ScriptedModel::new(vec![Msg::assistant("found it")]));
        let sub = gp_spec(sub_model);
        let agent = Agent::builder(main_model)
            .name("main")
            .middleware(Arc::new(SubAgentMiddleware::new(vec![sub])))
            .build();
        let out = agent.run_turn("go").await.unwrap();
        match out {
            TurnResult::Done(o) => assert!(o.final_text().contains("found it")),
            _ => panic!(),
        }
        let st = agent.state.lock().unwrap();
        assert!(
            st.messages
                .iter()
                .any(|m| matches!(m, Msg::Tool{content, ..} if content.contains("found it")))
        );
    }

    #[tokio::test]
    async fn task_unknown_subagent_returns_friendly_string() {
        let main_model = Arc::new(ScriptedModel::new(vec![
            Msg::Assistant {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall {
                    id: "c1".into(),
                    name: "task".into(),
                    args: serde_json::json!({"description":"x","subagent_type":"nope"}),
                }],
            },
            Msg::assistant("ok"),
        ]));
        let sub = gp_spec(Arc::new(ScriptedModel::new(vec![Msg::assistant("x")])));
        let agent = Agent::builder(main_model)
            .name("main")
            .middleware(Arc::new(SubAgentMiddleware::new(vec![sub])))
            .build();
        agent.run_turn("go").await.unwrap();
        let st = agent.state.lock().unwrap();
        assert!(st.messages.iter().any(|m| matches!(m, Msg::Tool{content, ..}
            if content.contains("cannot invoke subagent nope") && content.contains("`general-purpose`"))));
    }

    #[tokio::test]
    async fn send_message_pushes_notification() {
        let main_model = Arc::new(ScriptedModel::new(vec![
            Msg::Assistant {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall {
                    id: "c1".into(),
                    name: "send_message".into(),
                    args: serde_json::json!({"to":"researcher","message":"hello"}),
                }],
            },
            Msg::assistant("sent"),
        ]));
        let sub = gp_spec(Arc::new(ScriptedModel::new(vec![Msg::assistant("x")])));
        let (queue, mut receiver) = notification_channel();
        let agent = Agent::builder(main_model)
            .name("main")
            .notifications(queue.clone())
            .middleware(Arc::new(SubAgentMiddleware::new(vec![sub])))
            .build();
        agent.run_turn("go").await.unwrap();
        let notes = receiver.drain();
        assert!(notes.iter().any(|n| n.to == "researcher"
            && matches!(&n.kind, NotificationKind::Message(m) if m == "hello")));
    }

    #[tokio::test]
    async fn background_task_can_be_cancelled_through_registry() {
        use crate::model::{ModelRequest, ModelResponse, Usage};
        use std::sync::atomic::{AtomicBool, Ordering};

        struct SlowModel {
            completed: Arc<AtomicBool>,
        }
        #[async_trait]
        impl crate::model::ModelProvider for SlowModel {
            async fn complete(&self, _req: ModelRequest) -> Result<ModelResponse, AgentError> {
                tokio::time::sleep(Duration::from_secs(30)).await;
                self.completed.store(true, Ordering::SeqCst);
                Ok(ModelResponse {
                    message: Msg::assistant("late"),
                    usage: Usage::default(),
                })
            }
        }

        let main_model = Arc::new(ScriptedModel::new(vec![
            Msg::Assistant {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall {
                    id: "c1".into(),
                    name: "task".into(),
                    args: serde_json::json!({
                        "description":"slow work",
                        "subagent_type":"general-purpose",
                        "run_in_background": true
                    }),
                }],
            },
            Msg::assistant("started"),
        ]));
        let completed = Arc::new(AtomicBool::new(false));
        let sub = gp_spec(Arc::new(SlowModel {
            completed: completed.clone(),
        }));
        let registry = Arc::new(crate::subagent::AgentRegistry::default());
        let agent = Agent::builder(main_model)
            .name("main")
            .registry(registry.clone())
            .middleware(Arc::new(SubAgentMiddleware::new(vec![sub])))
            .build();

        agent.run_turn("go").await.unwrap();
        assert!(registry.abort("general-purpose#0"));
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            registry.status("general-purpose#0"),
            Some(AgentStatus::Failed)
        );
        assert!(
            !completed.load(Ordering::SeqCst),
            "cancelled child must not finish"
        );
    }

    #[tokio::test]
    async fn background_task_returns_immediately_and_notifies_on_completion() {
        let main_model = Arc::new(ScriptedModel::new(vec![
            Msg::Assistant {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall {
                    id: "c1".into(),
                    name: "task".into(),
                    args: serde_json::json!({
                        "description":"do async work",
                        "subagent_type":"general-purpose",
                        "run_in_background": true
                    }),
                }],
            },
            Msg::assistant("kicked off background"),
        ]));
        let sub_model = Arc::new(ScriptedModel::new(vec![Msg::assistant("bg done")]));
        let sub = gp_spec(sub_model);

        let (queue, mut receiver) = notification_channel();
        let agent = Agent::builder(main_model)
            .name("main")
            .notifications(queue.clone())
            .middleware(Arc::new(SubAgentMiddleware::new(vec![sub])))
            .build();

        agent.run_turn("go").await.unwrap();

        // Immediate tool result must announce the background start.
        {
            let st = agent.state.lock().unwrap();
            let tool_msg = st
                .messages
                .iter()
                .find_map(|m| match m {
                    Msg::Tool { content, .. } => Some(content.clone()),
                    _ => None,
                })
                .expect("expected a tool result message");
            assert!(
                tool_msg.starts_with("Started background agent"),
                "got: {tool_msg}"
            );
        }

        // Poll for the completion notification (spawned task runs concurrently).
        let mut completed = None;
        for _ in 0..200 {
            for n in receiver.drain() {
                if let NotificationKind::Completed(text) = n.kind {
                    completed = Some((n.from, text));
                }
            }
            if completed.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let (from, text) = completed.expect("expected a Completed notification within timeout");
        // Unique per-spawn id: {subagent_type}#{n}.
        assert_eq!(from, "general-purpose#0");
        assert!(text.contains("bg done"), "got: {text}");
    }

    #[tokio::test]
    async fn background_completion_is_fed_back_into_parent_turn() {
        use crate::model::{ModelRequest, ModelResponse, Usage};
        use std::sync::Mutex;
        use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
        use tokio::sync::Notify;

        struct ChildModel {
            done: Arc<Notify>,
        }
        #[async_trait]
        impl crate::model::ModelProvider for ChildModel {
            async fn complete(&self, _req: ModelRequest) -> Result<ModelResponse, AgentError> {
                self.done.notify_one();
                Ok(ModelResponse {
                    message: Msg::assistant("background evidence"),
                    usage: Usage::default(),
                })
            }
        }

        struct ParentModel {
            calls: AtomicUsize,
            requests: Mutex<Vec<ModelRequest>>,
            child_done: Arc<Notify>,
        }
        #[async_trait]
        impl crate::model::ModelProvider for ParentModel {
            async fn complete(&self, req: ModelRequest) -> Result<ModelResponse, AgentError> {
                self.requests.lock().unwrap().push(req);
                let call = self.calls.fetch_add(1, AtomicOrdering::SeqCst);
                let message = match call {
                    0 => Msg::Assistant {
                        content: String::new(),
                        tool_calls: vec![crate::state::ToolCall {
                            id: "c1".into(),
                            name: "task".into(),
                            args: serde_json::json!({
                                "description":"research",
                                "subagent_type":"general-purpose",
                                "run_in_background":true
                            }),
                        }],
                    },
                    1 => {
                        self.child_done.notified().await;
                        tokio::time::sleep(Duration::from_millis(10)).await;
                        Msg::assistant("draft before child feedback")
                    }
                    _ => Msg::assistant("final with child feedback"),
                };
                Ok(ModelResponse {
                    message,
                    usage: Usage::default(),
                })
            }
        }

        let child_done = Arc::new(Notify::new());
        let parent = Arc::new(ParentModel {
            calls: AtomicUsize::new(0),
            requests: Mutex::new(Vec::new()),
            child_done: child_done.clone(),
        });
        let sub = gp_spec(Arc::new(ChildModel { done: child_done }));
        let (queue, _ui_receiver) = notification_channel();
        let agent = Agent::builder(parent.clone())
            .name("main")
            .notifications(queue)
            .middleware(Arc::new(
                SubAgentMiddleware::new(vec![sub]).for_agent("main"),
            ))
            .build();

        let output = agent.run_turn("go").await.unwrap();
        assert!(matches!(output, TurnResult::Done(_)));
        let requests = parent.requests.lock().unwrap();
        assert_eq!(
            requests.len(),
            3,
            "completion should force a parent re-sample"
        );
        assert!(requests[2].messages.iter().any(|message| {
            matches!(message, Msg::User { content } if content.contains("background evidence"))
        }));
    }

    #[tokio::test]
    async fn send_message_delivers_to_running_agent_inbox() {
        use crate::subagent::{AgentEntry, AgentRegistry};

        // A registry pre-populated with a running agent that has an inbox.
        let registry = Arc::new(AgentRegistry::default());
        let (inbox_tx, mut inbox_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        registry.insert_entry(
            "researcher",
            AgentEntry {
                status: AgentStatus::Running,
                progress: "waiting".into(),
                join: None,
                inbox: Some(inbox_tx),
            },
        );

        let main_model = Arc::new(ScriptedModel::new(vec![
            Msg::Assistant {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall {
                    id: "c1".into(),
                    name: "send_message".into(),
                    args: serde_json::json!({"to":"researcher","message":"ping"}),
                }],
            },
            Msg::assistant("sent"),
        ]));
        let sub = gp_spec(Arc::new(ScriptedModel::new(vec![Msg::assistant("x")])));
        let agent = Agent::builder(main_model)
            .name("main")
            .registry(registry.clone())
            .middleware(Arc::new(SubAgentMiddleware::new(vec![sub])))
            .build();
        agent.run_turn("go").await.unwrap();

        // The message reached the registry inbox.
        assert_eq!(inbox_rx.try_recv().ok(), Some("ping".to_string()));

        // And the tool result reports the delivery.
        let st = agent.state.lock().unwrap();
        assert!(
            st.messages
                .iter()
                .any(|m| matches!(m, Msg::Tool{content, ..}
            if content.contains("delivered to inbox")))
        );
    }

    // FIX 1: a fast background subagent that finishes must leave the registry
    // entry at `Done`, not stuck on `Running`. This regresses the spawn-before-
    // insert race where `set_status(Done)` ran before the entry existed.
    #[tokio::test]
    async fn background_subagent_status_reaches_done() {
        use crate::subagent::AgentRegistry;

        let main_model = Arc::new(ScriptedModel::new(vec![
            Msg::Assistant {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall {
                    id: "c1".into(),
                    name: "task".into(),
                    args: serde_json::json!({
                        "description":"quick","subagent_type":"general-purpose","run_in_background":true
                    }),
                }],
            },
            Msg::assistant("kicked off"),
        ]));
        let sub = gp_spec(Arc::new(ScriptedModel::new(vec![Msg::assistant(
            "bg done",
        )])));
        let registry = Arc::new(AgentRegistry::default());
        let (queue, mut receiver) = notification_channel();
        let agent = Agent::builder(main_model)
            .name("main")
            .registry(registry.clone())
            .notifications(queue.clone())
            .middleware(Arc::new(SubAgentMiddleware::new(vec![sub])))
            .build();
        agent.run_turn("go").await.unwrap();

        // Drain the completion notification, then assert the registry status.
        let mut done = false;
        for _ in 0..200 {
            for n in receiver.drain() {
                if matches!(n.kind, NotificationKind::Completed(_)) {
                    done = true;
                }
            }
            if done && registry.status("general-purpose#0") == Some(AgentStatus::Done) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(
            registry.status("general-purpose#0"),
            Some(AgentStatus::Done),
            "background subagent status must reach Done, not stay Running"
        );
    }

    // FIX 3: a gated background subagent (interrupt_on: ["execute"]) whose model
    // calls execute must post a NeedsApproval notification and land at Suspended,
    // not be mis-reported as Failed.
    #[tokio::test]
    async fn background_subagent_suspension_surfaces_needs_approval() {
        use crate::subagent::AgentRegistry;
        use crate::testing::NoopTool;

        let main_model = Arc::new(ScriptedModel::new(vec![
            Msg::Assistant {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall {
                    id: "c1".into(),
                    name: "task".into(),
                    args: serde_json::json!({
                        "description":"do gated work","subagent_type":"gated","run_in_background":true
                    }),
                }],
            },
            Msg::assistant("kicked off"),
        ]));
        // The subagent model calls the gated `execute` tool on its first round.
        let sub_model = Arc::new(ScriptedModel::new(vec![Msg::Assistant {
            content: String::new(),
            tool_calls: vec![crate::state::ToolCall {
                id: "s1".into(),
                name: "execute".into(),
                args: serde_json::json!({"command":"rm -rf /"}),
            }],
        }]));
        let sub = SubAgentSpec {
            name: "gated".into(),
            description: "gated agent".into(),
            system_prompt: "you run commands".into(),
            tools: vec![Arc::new(NoopTool::new("execute"))],
            model: Some(sub_model),
            harness_profile: None,
            interrupt_on: vec!["execute".into()],
            max_depth: 1,
        };

        let registry = Arc::new(AgentRegistry::default());
        let (queue, mut receiver) = notification_channel();
        let agent = Agent::builder(main_model)
            .name("main")
            .registry(registry.clone())
            .notifications(queue.clone())
            .middleware(Arc::new(SubAgentMiddleware::new(vec![sub])))
            .build();
        agent.run_turn("go").await.unwrap();

        let mut needs_approval = None;
        for _ in 0..200 {
            for n in receiver.drain() {
                if let NotificationKind::NeedsApproval(s) = n.kind {
                    needs_approval = Some((n.from, s));
                }
            }
            if needs_approval.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let (from, s) = needs_approval.expect("expected a NeedsApproval notification");
        assert_eq!(from, "gated#0");
        assert_eq!(s.request.tool_call.name, "execute");
        assert_eq!(registry.status("gated#0"), Some(AgentStatus::Suspended));
    }

    /// A model that blocks every `complete` on a semaphore until the test
    /// releases it, recording how many calls have arrived — lets a test hold
    /// two background children mid-round deterministically.
    struct BlockingModel {
        gate: tokio::sync::Semaphore,
        arrived: std::sync::atomic::AtomicUsize,
    }
    #[async_trait]
    impl crate::model::ModelProvider for BlockingModel {
        async fn complete(
            &self,
            _req: crate::model::ModelRequest,
        ) -> Result<crate::model::ModelResponse, crate::AgentError> {
            self.arrived
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let _permit = self
                .gate
                .acquire()
                .await
                .map_err(|e| crate::AgentError::Model(format!("gate closed: {e}")))?;
            Ok(crate::model::ModelResponse {
                message: Msg::assistant("done"),
                usage: crate::model::Usage::default(),
            })
        }
    }

    // FRESH-HANDLE: two same-type background children run concurrently. With a
    // shared compiled handle, the second child's `set_inbox` overwrites (drops)
    // the first child's receiver, so `send_message` delivery to the first child
    // silently dies. A fresh handle per task keeps both inboxes alive.
    #[tokio::test]
    async fn concurrent_same_type_background_children_keep_independent_inboxes() {
        use crate::subagent::AgentRegistry;

        let main_model = Arc::new(ScriptedModel::new(vec![
            Msg::Assistant {
                content: String::new(),
                tool_calls: vec![
                    crate::state::ToolCall {
                        id: "c1".into(),
                        name: "task".into(),
                        args: serde_json::json!({
                            "description":"a","subagent_type":"general-purpose","run_in_background":true
                        }),
                    },
                    crate::state::ToolCall {
                        id: "c2".into(),
                        name: "task".into(),
                        args: serde_json::json!({
                            "description":"b","subagent_type":"general-purpose","run_in_background":true
                        }),
                    },
                ],
            },
            Msg::assistant("kicked off two"),
        ]));
        let blocking = Arc::new(BlockingModel {
            gate: tokio::sync::Semaphore::new(0),
            arrived: std::sync::atomic::AtomicUsize::new(0),
        });
        let sub = gp_spec(blocking.clone());
        let registry = Arc::new(AgentRegistry::default());
        let (queue, mut receiver) = notification_channel();
        let agent = Agent::builder(main_model)
            .name("main")
            .registry(registry.clone())
            .notifications(queue.clone())
            .middleware(Arc::new(SubAgentMiddleware::new(vec![sub])))
            .build();
        agent.run_turn("go").await.unwrap();

        // Wait until BOTH children are inside their model call (i.e. past
        // set_inbox) so any inbox clobbering has already happened.
        for _ in 0..500 {
            if blocking.arrived.load(std::sync::atomic::Ordering::SeqCst) >= 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(
            blocking.arrived.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "both children must be running"
        );
        assert_eq!(
            registry.progress("general-purpose#0").as_deref(),
            Some("thinking · round 1")
        );
        assert_eq!(
            registry.progress("general-purpose#1").as_deref(),
            Some("thinking · round 1")
        );

        // BOTH running children must be deliverable — with a shared handle the
        // first child's receiver was dropped by the second's set_inbox.
        assert!(
            registry.deliver("general-purpose#1", "to-b".into()),
            "second child's inbox must be live"
        );
        assert!(
            registry.deliver("general-purpose#0", "to-a".into()),
            "first child's inbox must STILL be live while it runs"
        );

        // Release the children and drain their completions.
        blocking.gate.add_permits(2);
        let mut completed = 0;
        for _ in 0..500 {
            completed += receiver
                .drain()
                .into_iter()
                .filter(|n| matches!(n.kind, NotificationKind::Completed(_)))
                .count();
            if completed >= 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(completed, 2);
    }

    /// A model whose `complete` panics — simulates any panic inside a child.
    struct PanickingModel;
    #[async_trait]
    impl crate::model::ModelProvider for PanickingModel {
        async fn complete(
            &self,
            _req: crate::model::ModelRequest,
        ) -> Result<crate::model::ModelResponse, crate::AgentError> {
            panic!("child blew up");
        }
    }

    // PANIC-SAFETY: a panicking background child must flip its registry entry
    // to Failed and post a notification — not stay `Running` forever as a
    // zombie the user can never observe finishing.
    #[tokio::test]
    async fn panicking_background_child_is_reported_failed_not_stuck_running() {
        use crate::subagent::AgentRegistry;

        let main_model = Arc::new(ScriptedModel::new(vec![
            Msg::Assistant {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall {
                    id: "c1".into(),
                    name: "task".into(),
                    args: serde_json::json!({
                        "description":"boom","subagent_type":"general-purpose","run_in_background":true
                    }),
                }],
            },
            Msg::assistant("kicked off"),
        ]));
        let sub = gp_spec(Arc::new(PanickingModel));
        let registry = Arc::new(AgentRegistry::default());
        let (queue, mut receiver) = notification_channel();
        let agent = Agent::builder(main_model)
            .name("main")
            .registry(registry.clone())
            .notifications(queue.clone())
            .middleware(Arc::new(SubAgentMiddleware::new(vec![sub])))
            .build();
        agent.run_turn("go").await.unwrap();

        let mut notified = false;
        for _ in 0..500 {
            if receiver
                .drain()
                .iter()
                .any(|n| matches!(&n.kind, NotificationKind::Completed(t) if t.contains("failed")))
            {
                notified = true;
            }
            if notified && registry.status("general-purpose#0") == Some(AgentStatus::Failed) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(notified, "a failure notification must be posted");
        assert_eq!(
            registry.status("general-purpose#0"),
            Some(AgentStatus::Failed),
            "a panicked child must be marked Failed, not left Running"
        );
    }

    // FIX 4: two background tasks of the SAME subagent_type must register under
    // distinct ids, so both are tracked (no collision on one key).
    #[tokio::test]
    async fn two_same_type_background_tasks_get_distinct_entries() {
        use crate::subagent::AgentRegistry;

        // The main model fires two background `task` calls in one round.
        let main_model = Arc::new(ScriptedModel::new(vec![
            Msg::Assistant {
                content: String::new(),
                tool_calls: vec![
                    crate::state::ToolCall {
                        id: "c1".into(),
                        name: "task".into(),
                        args: serde_json::json!({
                            "description":"a","subagent_type":"general-purpose","run_in_background":true
                        }),
                    },
                    crate::state::ToolCall {
                        id: "c2".into(),
                        name: "task".into(),
                        args: serde_json::json!({
                            "description":"b","subagent_type":"general-purpose","run_in_background":true
                        }),
                    },
                ],
            },
            Msg::assistant("kicked off two"),
        ]));
        let sub = gp_spec(Arc::new(ScriptedModel::new(vec![
            Msg::assistant("one"),
            Msg::assistant("two"),
        ])));
        let registry = Arc::new(AgentRegistry::default());
        let (queue, _receiver) = notification_channel();
        let agent = Agent::builder(main_model)
            .name("main")
            .registry(registry.clone())
            .notifications(queue.clone())
            .middleware(Arc::new(SubAgentMiddleware::new(vec![sub])))
            .build();
        agent.run_turn("go").await.unwrap();

        // Both unique ids are present in the registry.
        let names: Vec<String> = registry.list().into_iter().map(|(n, _)| n).collect();
        assert!(
            names.contains(&"general-purpose#0".to_string())
                && names.contains(&"general-purpose#1".to_string()),
            "expected two distinct entries, got: {names:?}"
        );
    }
}
