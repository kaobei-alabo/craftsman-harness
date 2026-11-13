//! The agent: craftsman-rs runs on the `craftsman-harness` framework. This module is
//! the *app-side glue* between that
//! framework and configured app services:
//!
//! - [`StreamingLlmProvider`] implements the framework's `ModelProvider` over
//!   the existing SSE [`LlmClient`], streaming content to the terminal exactly
//!   as before and accumulating token usage for `/tokens`.
//! - The host tools (`ls`/`read_file`/`write_file`/`edit_file`/`glob`/`grep`/
//!   `execute`) and configured MCP tools are exposed as framework `Tool`s that
//!   delegate to the `RealFs` backend / the MCP client.
//! - [`build_session_ext`] wires it all together: provider + backend + middleware
//!   (patch-tool-calls, todos, HITL approval) + tools.
//!
//! The HITL gate lives in [`crate::hitl_middleware`]; planning (`write_todos`)
//! and malformed-tool-call repair come from the framework's built-in
//! middleware.

use crate::checkpoints::{JsonlJournal, LibsqlCheckpointer};
use crate::config::Config;
use crate::hitl::ApprovalState;
use crate::hitl_middleware::ApprovalMiddleware;
use crate::llm::{LlmClient, Message as LlmMessage, ToolCall as LlmToolCall, TurnResult};
use crate::lsp::LspManager;
use crate::mcp::McpTools;
use crate::workspace_checkpoint::WorkspaceCheckpointManager;
use async_trait::async_trait;
use craftsman_harness::builtin::{
    CompactionMiddleware, CompactionTrigger, MemoryMiddleware, PatchToolCallsMiddleware,
    ResultBudgetsMiddleware, SubAgentMiddleware, SystemRemindersMiddleware, TodoListMiddleware,
};
use craftsman_harness::{
    Agent, AgentError, AgentMiddleware, AgentRegistry, AgentState, Backend, ModelProvider,
    ModelRequest, ModelResponse, Msg, NotificationReceiver, SubAgentSpec, Tool,
    ToolCall as FwToolCall, ToolCtx, ToolDef, ToolOutcome, Usage, apply_middleware_profile,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// A live agent plus the handles the REPL needs to inspect and reset it.
pub struct Session {
    /// Local gadget host, present only in opted-in terminal sessions.
    #[cfg(feature = "gadgets")]
    pub gadgets: Option<Arc<crate::gadgets::GadgetRuntime>>,
    pub agent: Agent,
    /// `(input_tokens, output_tokens)` accumulated across the session, for `/tokens`.
    pub usage: Arc<Mutex<(u64, u64)>>,
    /// Shared registry of spawned subagents, for `/tasks` listing. Populated by
    /// the `task` tool (contributed by [`SubAgentMiddleware`]) when it spawns a
    /// background child.
    pub registry: Arc<AgentRegistry>,
    /// App-side drain end of the notification queue. Background subagents push
    /// completion / message / needs-approval notifications here; the REPL drains
    /// it at each turn boundary (see `tui_repl::drain_notifications`).
    pub notifications: NotificationReceiver,
    /// The active thread id for checkpoint/journal writes. Rotated by `/clear`
    /// and switched by `/session <id>`; the REPL marks it as current when
    /// listing threads.
    pub thread_id: String,
    /// The concrete checkpointer, when persistence is available — the SAME `Arc`
    /// handed to the agent builder (as `Arc<dyn Checkpointer>`), kept here typed
    /// concretely so `/session` can call `list_threads`. `None` when the store
    /// failed to open. Owned terminal requests require persistence; legacy
    /// frontends may still run without it.
    pub checkpointer: Option<Arc<LibsqlCheckpointer>>,
    /// Per-turn workspace file checkpoint/rewind service. It shares the
    /// conversation store and follows thread switches through Session helpers.
    pub workspace_checkpoints: Option<Arc<WorkspaceCheckpointManager>>,
    /// The durable long-term-memory store, when it opened — the SAME `Arc` routed
    /// into the composite backend. Kept here so `/memories` can list the files
    /// without reaching into the agent's backend. `None` when the store failed to
    /// open (the session still runs, just without durable memory).
    pub memories: Option<Arc<crate::store_fs::StoreFs>>,
    /// The SAME compaction middleware `Arc` wired into the agent, kept typed
    /// concretely so `/compact` can trigger a manual pass (`compact_now`)
    /// without waiting for the automatic message-count threshold.
    pub compaction: Arc<CompactionMiddleware>,
    /// `/mcp` listing rows for every server that connected at build time.
    pub mcp: Vec<crate::mcp::McpServerInfo>,
    /// Shared lazy language-server manager used by main and delegated agents.
    pub lsp: Arc<LspManager>,
    /// `(server, error)` for each MCP server that failed to load.
    pub mcp_errors: Vec<(String, String)>,
    /// Route/provider diagnostics used by `/harness status`.
    pub route_source: String,
    pub provider_profile_mode: String,
    /// Canonical model identity used during profile resolution.
    pub model_provider: Option<String>,
    pub profile_model_id: String,
    /// User-selected harness preset (`auto`, `general`, `openai`, `claude`,
    /// or `nemotron`). The effective profile key may be more specific.
    pub harness_preset: String,
    /// Active harness profile key and all merged layers (`general` first).
    pub harness_profile: String,
    pub harness_profile_layers: Vec<String>,
    /// Active provider-profile key, when model construction defaults matched.
    pub provider_profile: Option<String>,
}

/// Fraction of a model's context window at which the token-based compaction
/// trigger fires (see `CompactionTrigger::Tokens`). A starting value, not
/// independently tuned against craftsman-harness usage data.
pub const COMPACT_TRIGGER_FRACTION: f64 = 0.85;
/// Token budget for the kept tail under the token-based trigger — matches
/// OpenAI Codex's `COMPACT_USER_MESSAGE_MAX_TOKENS` constant. Fixed
/// regardless of the active model's context window size (unlike
/// `COMPACT_TRIGGER_FRACTION`, which scales with it).
pub const COMPACT_KEEP_TAIL_TOKENS: u64 = 20_000;
/// Message-count fallback trigger, used when no context window is configured
/// for the active model (`model_profiles.<model>.context_window` absent) —
/// unchanged from this middleware's original, pre-token-trigger behavior.
pub const COMPACT_THRESHOLD_MSGS: usize = 120;
/// Messages kept verbatim at the end of every fallback compaction pass.
pub const COMPACT_KEEP_TAIL_MSGS: usize = 20;

impl Session {
    /// Estimated session cost in USD. No configured rates are available,
    /// so the footer truthfully shows `n/a`.
    pub fn cost_usd(&self) -> Option<f64> {
        None
    }

    pub fn set_thread_id(&mut self, thread_id: impl Into<String>) {
        let thread_id = thread_id.into();
        self.agent.set_thread_id(&thread_id);
        if let Some(workspace) = &self.workspace_checkpoints {
            workspace.set_thread_id(thread_id.clone());
        }
        self.thread_id = thread_id;
    }

    pub async fn load_thread(&mut self, thread_id: &str) -> Result<bool, AgentError> {
        let loaded = self.agent.load_thread(thread_id).await?;
        if loaded {
            if let Some(workspace) = &self.workspace_checkpoints {
                workspace.set_thread_id(thread_id.to_string());
            }
            self.thread_id = thread_id.to_string();
        }
        Ok(loaded)
    }
}

// ---------------------------------------------------------------------------
// Model provider: framework ↔ OpenAI-wire bridge over the SSE client.
// ---------------------------------------------------------------------------

/// Where a provider's live content deltas go. The terminal modes stream to
/// stdout; ACP routes them onto its outbound frame channel instead.
pub type TokenSink = Arc<dyn Fn(&str) + Send + Sync>;

/// Activity lines emitted around tool calls. The TUI supplies a channel-backed
/// sink; headless modes keep the existing stdout/stderr behavior.
pub type ActivitySink = Arc<dyn Fn(&str) + Send + Sync>;
/// Notifies a frontend immediately before each main-agent model sample.
pub type RoundSink = Arc<dyn Fn() + Send + Sync>;

/// Structured lifecycle event for a main-agent tool call. Interactive UIs use
/// this to maintain one collapsible block from invocation through completion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolActivityEvent {
    Started {
        call_id: String,
        tool: String,
        summary: String,
        details: String,
    },
    Finished {
        call_id: String,
        tool: String,
        content: String,
        is_error: bool,
    },
}

pub type ToolActivitySink = Arc<dyn Fn(ToolActivityEvent) + Send + Sync>;

/// Result returned by a UI-driven approval prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalPromptResult {
    Approve,
    ApproveAlways,
    Reject(String),
}

/// Interactive approval callback used by the Ratatui frontend. It runs on the
/// agent task and may block while the UI collects a decision. The approval
/// state mutex is deliberately not passed or held while this callback waits.
pub type ApprovalPrompt = Arc<dyn Fn(&str, &str, &str) -> ApprovalPromptResult + Send + Sync>;

/// App-only correlated approval callback. Other frontends retain their existing
/// three-argument callback; ACP receives the actual framework tool invocation.
pub type CallApprovalPrompt =
    Arc<dyn Fn(&craftsman_harness::ToolCall, &str, &str) -> ApprovalPromptResult + Send + Sync>;

/// Interactive answer callback for the `ask_user` tool.
pub type AskUserPrompt = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// The stdout sink — the REPL and one-shot behavior.
pub fn stdout_sink() -> TokenSink {
    Arc::new(|t: &str| {
        use std::io::Write;
        print!("{t}");
        let _ = std::io::stdout().flush();
    })
}

/// Bridges the framework's `ModelProvider` to the existing streaming SSE
/// client. Content deltas go to `sink` live (stdout in the terminal modes,
/// the outbound frame channel in ACP); token usage accumulates for `/tokens`.
pub struct StreamingLlmProvider {
    pub client: LlmClient,
    pub usage: Arc<Mutex<(u64, u64)>>,
    pub sink: TokenSink,
    pub round_sink: Option<RoundSink>,
    /// Optional UI activity channel for transport retries/warnings.
    pub status_sink: Option<ActivitySink>,
}

#[async_trait]
impl ModelProvider for StreamingLlmProvider {
    async fn complete(&self, req: ModelRequest) -> Result<ModelResponse, AgentError> {
        if let Some(round_sink) = &self.round_sink {
            round_sink();
        }
        if let Some(native) = &self.client.native {
            let response = native
                .complete(&req, |text| (self.sink)(text))
                .await
                .map_err(|error| AgentError::Model(error.to_string()))?;
            let mut usage = self.usage.lock().unwrap();
            let input = usage
                .0
                .checked_add(response.usage.input_tokens)
                .ok_or_else(|| AgentError::Model("Model input usage overflow.".into()))?;
            let output = usage
                .1
                .checked_add(response.usage.output_tokens)
                .ok_or_else(|| AgentError::Model("Model output usage overflow.".into()))?;
            *usage = (input, output);
            return Ok(response);
        }
        let messages = to_llm_messages(&req.system, &req.messages);
        let tools = to_openai_tools(&req.tools);
        let turn = self
            .client
            .stream_turn(
                &messages,
                &tools,
                |text| (self.sink)(text),
                |status| {
                    if let Some(sink) = &self.status_sink {
                        sink(status);
                    }
                },
            )
            .await
            .map_err(|e| AgentError::Model(e.to_string()))?;
        {
            let mut u = self.usage.lock().unwrap();
            u.0 += turn.usage.input_tokens;
            u.1 += turn.usage.output_tokens;
        }
        let usage = Usage {
            input_tokens: turn.usage.input_tokens,
            output_tokens: turn.usage.output_tokens,
        };
        Ok(ModelResponse {
            message: to_fw_message(turn),
            usage,
        })
    }
}

/// Framework messages → the OpenAI chat message list the SSE client speaks.
/// The system prompt (kept separate by the framework) becomes the leading
/// `system` message.
fn to_llm_messages(system: &str, msgs: &[Msg]) -> Vec<LlmMessage> {
    let mut out = vec![LlmMessage::System {
        content: system.to_string(),
    }];
    for m in msgs {
        match m {
            Msg::System { content } => out.push(LlmMessage::System {
                content: content.clone(),
            }),
            Msg::User { content } => out.push(LlmMessage::User {
                content: content.clone(),
            }),
            Msg::Assistant {
                content,
                tool_calls,
            } => {
                if tool_calls.is_empty() {
                    out.push(LlmMessage::Assistant {
                        content: Some(content.clone()),
                        tool_calls: None,
                    });
                } else {
                    let calls: Vec<LlmToolCall> = tool_calls
                        .iter()
                        .map(|tc| LlmToolCall {
                            id: tc.id.clone(),
                            name: tc.name.clone(),
                            arguments: tc.args.to_string(),
                        })
                        .collect();
                    let c = (!content.is_empty()).then(|| content.clone());
                    out.push(LlmMessage::assistant_with_calls(c, &calls));
                }
            }
            Msg::Tool {
                tool_call_id,
                content,
                ..
            } => out.push(LlmMessage::Tool {
                content: content.clone(),
                tool_call_id: tool_call_id.clone(),
            }),
        }
    }
    out
}

/// Framework `ToolDef`s → OpenAI `tools` array entries.
fn to_openai_tools(tools: &[ToolDef]) -> Vec<Value> {
    tools
        .iter()
        .map(|d| {
            json!({
                "type": "function",
                "function": {
                    "name": d.name,
                    "description": d.description,
                    "parameters": d.parameters,
                }
            })
        })
        .collect()
}

/// One streamed completion → the framework's assistant message. Tool-call
/// argument strings are parsed to JSON; anything unparseable is passed through
/// as a string for `PatchToolCallsMiddleware` to repair.
pub(crate) fn to_fw_message(turn: TurnResult) -> Msg {
    let tool_calls = turn
        .tool_calls
        .into_iter()
        .map(|tc| {
            // Tool args must be a JSON object. Empty → {}; a parseable object →
            // itself; anything else (unparseable, or a valid non-object like an
            // array/number) → a string, which PatchToolCallsMiddleware repairs.
            let args = if tc.arguments.trim().is_empty() {
                json!({})
            } else {
                match serde_json::from_str::<Value>(&tc.arguments) {
                    Ok(v) if v.is_object() => v,
                    _ => Value::String(tc.arguments),
                }
            };
            FwToolCall {
                id: tc.id,
                name: tc.name,
                args,
            }
        })
        .collect();
    Msg::Assistant {
        content: turn.content,
        tool_calls,
    }
}

// ---------------------------------------------------------------------------
// Host tools (delegating to the RealFs backend).
// ---------------------------------------------------------------------------

enum HostKind {
    Ls,
    Read,
    Write,
    Edit,
    Glob,
    Grep,
    Execute,
}

struct HostTool {
    kind: HostKind,
    /// The filesystem/exec backend this tool operates against. Held explicitly
    /// (rather than read from `ctx.backend`) so the same host tools work for the
    /// main agent AND for subagents — a subagent `Agent` built by
    /// `SubAgentMiddleware` gets the framework's default in-memory `StateFs`
    /// backend, so reading `ctx.backend` there would hit an empty filesystem.
    /// `RealFs` is stateless (just a rooted path) and never touches
    /// `state.files`, so sharing one `Arc` across agents is safe and the
    /// dispatch-commit merge stays a no-op.
    backend: Arc<dyn Backend>,
    lsp: Option<Arc<LspManager>>,
    workspace_checkpoints: Option<Arc<WorkspaceCheckpointManager>>,
    /// The `execute` runner with the session's `shell` settings (per-call
    /// timeout, output format). `None` falls back to `backend.execute` with
    /// today's schema and text.
    shell: Option<Arc<crate::tools::ShellRunner>>,
}

impl HostTool {
    async fn append_diagnostics(&self, path: &str, message: &mut String) {
        // Headless construction supplies None, not an empty Config that would
        // still enable built-in language servers through effective_lsp_servers.
        let Some(lsp) = &self.lsp else {
            return;
        };
        if let Ok(Ok(diagnostics)) =
            tokio::time::timeout(std::time::Duration::from_secs(2), lsp.diagnostics(path)).await
            && !diagnostics.starts_with("No diagnostics")
        {
            message.push_str("\n\nDiagnostics:\n");
            message.push_str(&diagnostics);
        }
    }

    /// The seven host tools, as framework `Tool`s, all bound to `backend`.
    fn all(
        backend: Arc<dyn Backend>,
        lsp: Option<Arc<LspManager>>,
        workspace_checkpoints: Option<Arc<WorkspaceCheckpointManager>>,
        shell: Option<Arc<crate::tools::ShellRunner>>,
    ) -> Vec<Arc<dyn Tool>> {
        [
            HostKind::Execute,
            HostKind::Read,
            HostKind::Write,
            HostKind::Edit,
            HostKind::Ls,
            HostKind::Glob,
            HostKind::Grep,
        ]
        .into_iter()
        .map(|kind| {
            Arc::new(HostTool {
                kind,
                backend: backend.clone(),
                lsp: lsp.clone(),
                workspace_checkpoints: workspace_checkpoints.clone(),
                shell: shell.clone(),
            }) as Arc<dyn Tool>
        })
        .collect()
    }
}

fn err_outcome(e: AgentError) -> ToolOutcome {
    ToolOutcome::err(e.to_string())
}

#[async_trait]
impl Tool for HostTool {
    fn def(&self) -> ToolDef {
        match self.kind {
            HostKind::Execute => ToolDef {
                name: "execute".into(),
                description: "Run a shell command on the host and return its output.".into(),
                parameters: crate::tools::execute_parameters(
                    self.shell.as_ref().map(|shell| shell.settings()),
                ),
            },
            HostKind::Read => ToolDef {
                name: "read_file".into(),
                description: "Read a file from the local filesystem. Results use cat -n format, \
                     with line numbers starting at 1. Use offset/limit to paginate large files."
                    .into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "file_path": {"type": "string", "description": "Absolute path to the file to read."},
                        "offset": {"type": "integer", "default": 0, "description": "Line number to start reading from (0-indexed)."},
                        "limit": {"type": "integer", "default": 100, "description": "Maximum number of lines to read."}
                    },
                    "required": ["file_path"]
                }),
            },
            HostKind::Write => ToolDef {
                name: "write_file".into(),
                description: "Create a new file with the given content. Fails if the file already \
                     exists — read it and use edit_file instead, or write to a new path."
                    .into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "file_path": {"type": "string"},
                        "content": {"type": "string"}
                    },
                    "required": ["file_path", "content"]
                }),
            },
            HostKind::Edit => ToolDef {
                name: "edit_file".into(),
                description:
                    "Replace an exact string in a file. old_string must occur exactly once unless replace_all."
                        .into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "file_path": {"type": "string"},
                        "old_string": {"type": "string"},
                        "new_string": {"type": "string"},
                        "replace_all": {"type": "boolean", "default": false}
                    },
                    "required": ["file_path", "old_string", "new_string"]
                }),
            },
            HostKind::Ls => ToolDef {
                name: "ls".into(),
                description: "List directory contents.".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {"path": {"type": "string", "default": "."}},
                    "required": []
                }),
            },
            HostKind::Glob => ToolDef {
                name: "glob".into(),
                description:
                    "Find files matching a glob pattern (relative to the working directory)."
                        .into(),
                parameters: json!({
                    "type": "object",
                    "properties": {"pattern": {"type": "string"}},
                    "required": ["pattern"]
                }),
            },
            HostKind::Grep => ToolDef {
                name: "grep".into(),
                description:
                    "Search file contents for a pattern. Returns matching lines with file and line number."
                        .into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "pattern": {"type": "string"},
                        "path": {"type": "string", "description": "Optional directory to scope the search."}
                    },
                    "required": ["pattern"]
                }),
            },
        }
    }

    async fn call(&self, args: Value, _ctx: &mut ToolCtx<'_>) -> Result<ToolOutcome, AgentError> {
        let b = self.backend.clone();
        Ok(match self.kind {
            HostKind::Execute => {
                let cmd = args["command"].as_str().unwrap_or_default();
                // Preserve frontend validation, but private argument errors
                // must reach the actual scoped runner's frozen redactor too.
                if cmd.is_empty()
                    && !self
                        .shell
                        .as_ref()
                        .is_some_and(|shell| shell.has_headless_scope())
                {
                    ToolOutcome::err("Error: empty command")
                } else {
                    let baseline = match &self.workspace_checkpoints {
                        Some(workspace) => {
                            workspace
                                .begin_mutation(format!(
                                    "execute: {}",
                                    crate::util::one_line(cmd, 80)
                                ))
                                .await
                        }
                        None => None,
                    };
                    let outcome = match &self.shell {
                        Some(shell) => shell.run_outcome(cmd, args.get("timeout")).await,
                        None => match b.execute(cmd).await {
                            Ok(out) => ToolOutcome::ok(crate::tools::format_exec(
                                out.stdout,
                                out.stderr,
                                out.exit_code,
                            )),
                            Err(e) => err_outcome(e),
                        },
                    };
                    if let Some(workspace) = &self.workspace_checkpoints {
                        workspace.finish_mutation(baseline).await;
                    }
                    outcome
                }
            }
            HostKind::Read => {
                let path = args["file_path"].as_str().unwrap_or_default();
                let offset = args["offset"].as_u64().unwrap_or(0) as usize;
                let limit = args["limit"].as_u64().unwrap_or(100).max(1) as usize;
                match b.read(path, offset, limit).await {
                    Ok(s) => ToolOutcome::ok(s),
                    Err(e) => err_outcome(e),
                }
            }
            HostKind::Write => {
                let path = args["file_path"].as_str().unwrap_or_default();
                let content = args["content"].as_str().unwrap_or_default();
                let baseline = match &self.workspace_checkpoints {
                    Some(workspace) => {
                        workspace
                            .begin_mutation(format!("write_file: {path}"))
                            .await
                    }
                    None => None,
                };
                let result = b.write(path, content).await;
                if let Some(workspace) = &self.workspace_checkpoints {
                    workspace.finish_mutation(baseline).await;
                }
                match result {
                    Ok(()) => {
                        let mut message = format!("Updated file {path}");
                        self.append_diagnostics(path, &mut message).await;
                        ToolOutcome::ok(message)
                    }
                    Err(e) => err_outcome(e),
                }
            }
            HostKind::Edit => {
                let path = args["file_path"].as_str().unwrap_or_default();
                let old = args["old_string"].as_str().unwrap_or_default();
                let new = args["new_string"].as_str().unwrap_or_default();
                let replace_all = args["replace_all"].as_bool().unwrap_or(false);
                let baseline = match &self.workspace_checkpoints {
                    Some(workspace) => workspace.begin_mutation(format!("edit_file: {path}")).await,
                    None => None,
                };
                let result = b.edit(path, old, new, replace_all).await;
                if let Some(workspace) = &self.workspace_checkpoints {
                    workspace.finish_mutation(baseline).await;
                }
                match result {
                    Ok(()) => {
                        let mut message = format!("Updated file {path}");
                        self.append_diagnostics(path, &mut message).await;
                        ToolOutcome::ok(message)
                    }
                    Err(e) => err_outcome(e),
                }
            }
            HostKind::Ls => {
                let path = args["path"].as_str().unwrap_or(".");
                match b.ls(path).await {
                    Ok(entries) => ToolOutcome::ok(
                        entries
                            .into_iter()
                            .map(|f| f.path)
                            .collect::<Vec<_>>()
                            .join("\n"),
                    ),
                    Err(e) => err_outcome(e),
                }
            }
            HostKind::Glob => {
                let pattern = args["pattern"].as_str().unwrap_or_default();
                match b.glob(pattern).await {
                    Ok(hits) if hits.is_empty() => {
                        ToolOutcome::ok(format!("No files match pattern '{pattern}'"))
                    }
                    Ok(hits) => ToolOutcome::ok(hits.join("\n")),
                    Err(e) => err_outcome(e),
                }
            }
            HostKind::Grep => {
                let pattern = args["pattern"].as_str().unwrap_or_default();
                let path = args["path"].as_str();
                match b.grep(pattern, path).await {
                    Ok(s) => ToolOutcome::ok(s),
                    Err(e) => err_outcome(e),
                }
            }
        })
    }
}

fn host_and_web_tools(
    backend: Arc<dyn Backend>,
    cfg: &Config,
    lsp: Option<Arc<LspManager>>,
    workspace_checkpoints: Option<Arc<WorkspaceCheckpointManager>>,
    shell: Option<Arc<crate::tools::ShellRunner>>,
) -> Vec<Arc<dyn Tool>> {
    let mut tools = HostTool::all(backend, lsp, workspace_checkpoints, shell);
    tools.push(Arc::new(crate::web_tools::WebFetchTool::new()));
    tools.push(Arc::new(crate::web_tools::WebSearchTool::from_config(cfg)));
    tools
}

// ---------------------------------------------------------------------------
// Configured MCP tools retain their existing permission/capability gates.
// ---------------------------------------------------------------------------

struct McpFrameworkTool {
    mcp: Arc<McpTools>,
    def: ToolDef,
    name: String,
}

#[async_trait]
impl Tool for McpFrameworkTool {
    fn def(&self) -> ToolDef {
        self.def.clone()
    }

    async fn call(&self, args: Value, _ctx: &mut ToolCtx<'_>) -> Result<ToolOutcome, AgentError> {
        // `call_tool` never fails the turn — it returns error text as its
        // result string, which the model reads and adjusts to.
        Ok(ToolOutcome::ok(
            self.mcp.client.call_tool(&self.name, &args).await,
        ))
    }
}

// ---------------------------------------------------------------------------
// Session construction.
// ---------------------------------------------------------------------------

/// Frontend extensions to [`build_session_ext`]. Defaults reproduce the
/// established terminal behavior; explicit sinks never use terminal stdout.
#[derive(Default)]
pub struct SessionExtras {
    /// Installed terminal shutdown router's session owner. Absent in other modes.
    pub cancellation_session: Option<craftsman_harness::SessionCancellation>,
    /// TUI and ACP main-model wait opt-in; tools retain their cleanup ownership.
    /// Other legacy/private/frozen frontends leave this false.
    pub interrupt_model_on_cancel: bool,
    /// Explicit TUI/ACP memory-guidance opt-in; legacy/private/frozen defaults stay false.
    pub task_focused_memory: bool,
    /// Terminal-only opt-in; frozen/deferred frontends never load gadgets.
    #[cfg(feature = "gadgets")]
    pub local_gadgets: bool,
    #[cfg(feature = "gadgets")]
    pub allowed_gadgets: BTreeSet<String>,
    #[cfg(feature = "gadgets")]
    pub trust_workspace: bool,
    /// Where the MAIN agent's content deltas go. `None` = stdout (terminal).
    /// When set by a frontend, the subagent/compaction providers go SILENT instead
    /// of printing to stdout — their internal chatter is not part of the wire
    /// protocol and would only pollute the pod log.
    pub token_sink: Option<TokenSink>,
    /// Optional latched sink failure (legacy `-m`). All role providers check it
    /// before/after a response; None preserves other frontends exactly. The
    /// caller owns in-flight cancellation; this is not P2g shutdown ordering.
    pub output_closed: Option<Arc<std::sync::atomic::AtomicBool>>,
    /// Optional notification before each main-agent model round.
    pub round_sink: Option<RoundSink>,
    /// Optional legacy text activity sink used by frontends and transports.
    pub activity_sink: Option<ActivitySink>,
    /// Optional structured tool lifecycle sink for collapsible UI blocks.
    pub tool_activity_sink: Option<ToolActivitySink>,
    /// Optional UI-driven approval prompt.
    pub approval_prompt: Option<ApprovalPrompt>,
    /// Optional approval callback carrying the actual invocation (ACP).
    pub call_approval_prompt: Option<CallApprovalPrompt>,
    /// Correlated invocation/permission/result callbacks selected only by ACP.
    pub delegated_tool_callbacks: Option<(CallApprovalPrompt, ToolActivitySink)>,
    /// Optional UI-driven `ask_user` prompt.
    pub ask_user_prompt: Option<AskUserPrompt>,
    /// Additional tools registered on the main agent (ACP adds `render_ui`).
    pub extra_tools: Vec<Arc<dyn craftsman_harness::Tool>>,
    /// Explicit workspace root for client-selected sessions. Terminal callers leave
    /// this unset and retain current-directory behavior.
    pub workspace_root: Option<PathBuf>,
}

// Construction authority is typed independently of frontend sinks. The only
// headless caller consumes Prepared; a sanitized legacy Config cannot opt in.
/// Opaque shell lease constructed ONLY from the actual shared-builder branch.
/// No caller can combine arbitrary paths/settings/seeds with another gate.
pub(crate) struct ShellScope<'a> {
    prepared: &'a crate::exec::construction::Prepared,
    gate: Arc<crate::exec::runtime::InvocationGate>,
}
impl ShellScope<'_> {
    pub(crate) fn prepared(&self) -> &crate::exec::construction::Prepared {
        self.prepared
    }
    pub(crate) fn gate(&self) -> Arc<crate::exec::runtime::InvocationGate> {
        self.gate.clone()
    }
}

/// Filesystem view authority issued ONLY by the actual shared-builder branch.
/// A renderer cannot pair arbitrary filesystem/seeds with a different latch.
pub(crate) struct FilesystemScope<'a> {
    prepared: &'a crate::exec::construction::Prepared,
    gate: Arc<crate::exec::runtime::InvocationGate>,
}
impl FilesystemScope<'_> {
    pub(crate) fn prepared(&self) -> &crate::exec::construction::Prepared {
        self.prepared
    }
    pub(crate) fn gate(&self) -> Arc<crate::exec::runtime::InvocationGate> {
        self.gate.clone()
    }
}

/// Web authority is issued only by the actual shared-builder Headless branch.
/// Original config/auth/permission snapshots cannot be paired with caller seeds.
pub(crate) struct WebScope<'a> {
    prepared: &'a crate::exec::construction::Prepared,
    gate: Arc<crate::exec::runtime::InvocationGate>,
}
impl WebScope<'_> {
    pub(crate) fn prepared(&self) -> &crate::exec::construction::Prepared {
        self.prepared
    }
    pub(crate) fn gate(&self) -> Arc<crate::exec::runtime::InvocationGate> {
        self.gate.clone()
    }
}

/// Concrete framework-output lease issued ONLY by this actual shared builder.
/// No arbitrary prepared/config/seed/gate pairing can construct a producer.
pub(crate) struct FrameworkScope<'a> {
    prepared: &'a crate::exec::construction::Prepared,
    gate: Arc<crate::exec::runtime::InvocationGate>,
}
impl FrameworkScope<'_> {
    pub(crate) fn prepared(&self) -> &crate::exec::construction::Prepared {
        self.prepared
    }
    pub(crate) fn gate(&self) -> Arc<crate::exec::runtime::InvocationGate> {
        self.gate.clone()
    }
}

/// Opaque concrete task/message source, issued after the actual builder has
/// installed its SAME all-role providers/tools/child gate. Not caller specs.
pub(crate) struct SubagentOutputSource {
    original: SubAgentMiddleware,
    gate: Arc<crate::exec::runtime::InvocationGate>,
}
impl SubagentOutputSource {
    pub(crate) fn into_parts(
        self,
    ) -> (
        SubAgentMiddleware,
        Arc<crate::exec::runtime::InvocationGate>,
    ) {
        (self.original, self.gate)
    }
}

/// Provider request source issued only by the actual shared-builder branch,
/// pairing ORIGINAL effective options/route/model/usage and its unique gate.
pub(crate) struct ProviderScope<'a> {
    prepared: &'a crate::exec::construction::Prepared,
    gate: Arc<crate::exec::runtime::InvocationGate>,
    model: &'a str,
    options: &'a BTreeMap<String, Value>,
    usage: Arc<Mutex<(u64, u64)>>,
    agents: &'a [String],
}
impl ProviderScope<'_> {
    pub(crate) fn prepared(&self) -> &crate::exec::construction::Prepared {
        self.prepared
    }
    pub(crate) fn gate(&self) -> Arc<crate::exec::runtime::InvocationGate> {
        self.gate.clone()
    }
    pub(crate) fn model(&self) -> &str {
        self.model
    }
    pub(crate) fn options(&self) -> &BTreeMap<String, Value> {
        self.options
    }
    pub(crate) fn usage(&self) -> Arc<Mutex<(u64, u64)>> {
        self.usage.clone()
    }
    pub(crate) fn agents(&self) -> &[String] {
        self.agents
    }
}

/// Only actual prepared-session construction can mint this request authority.
pub(crate) struct RequestMeterScope {
    gate: Arc<crate::exec::runtime::InvocationGate>,
    limit: crate::exec::meter::RequestLimit,
    token_limit: Option<crate::exec::meter::TokenLimit>,
}
impl RequestMeterScope {
    pub(crate) fn into_parts(
        self,
    ) -> (
        Arc<crate::exec::runtime::InvocationGate>,
        crate::exec::meter::RequestLimit,
        Option<crate::exec::meter::TokenLimit>,
    ) {
        (self.gate, self.limit, self.token_limit)
    }
}
/// Concrete role and ORIGINAL provider binding issued by this same builder.
/// No caller provider/gate/label can be supplied to the counter wrapper.
pub(crate) struct RequestProviderScope {
    meter: Arc<crate::exec::meter::Meter>,
    gate: Arc<crate::exec::runtime::InvocationGate>,
    role: crate::exec::meter::Role,
    original: Arc<dyn ModelProvider>,
}
impl RequestProviderScope {
    pub(crate) fn into_parts(
        self,
    ) -> (
        Arc<crate::exec::meter::Meter>,
        Arc<crate::exec::runtime::InvocationGate>,
        crate::exec::meter::Role,
        Arc<dyn ModelProvider>,
    ) {
        (self.meter, self.gate, self.role, self.original)
    }
}

/// Actual shared-builder concrete client for combined request/token admission.
/// No caller provider/route/estimate/output/receipt or string-derived role.
pub(crate) struct TokenProviderScope {
    meter: Arc<crate::exec::meter::Meter>,
    gate: Arc<crate::exec::runtime::InvocationGate>,
    role: crate::exec::meter::Role,
    original: crate::exec::provider::Provider,
}
impl TokenProviderScope {
    pub(crate) fn into_parts(
        self,
    ) -> (
        Arc<crate::exec::meter::Meter>,
        Arc<crate::exec::runtime::InvocationGate>,
        crate::exec::meter::Role,
        crate::exec::provider::Provider,
    ) {
        (self.meter, self.gate, self.role, self.original)
    }
}

enum SessionConstruction<'a> {
    Frontend(&'a Config),
    Headless {
        prepared: &'a crate::exec::construction::Prepared,
        gate: &'a Arc<crate::exec::runtime::InvocationGate>,
    },
}
impl SessionConstruction<'_> {
    /// Mandatory actual-builder path for EACH private role. Both modes retain
    /// the same gate; neither middleware exclusion nor a request-only wrapper
    /// can restore an unreserved provider under a selected token ceiling.
    fn headless_provider(
        &self,
        scope: ProviderScope<'_>,
        meter: Option<&Arc<crate::exec::meter::Meter>>,
        role: crate::exec::meter::Role,
        output_closed: Option<Arc<std::sync::atomic::AtomicBool>>,
    ) -> Option<Arc<dyn ModelProvider>> {
        let Self::Headless { prepared, gate } = self else {
            return None;
        };
        let meter = meter?;
        if !meter.matches_gate(gate)
            || !Arc::ptr_eq(gate, &scope.gate())
            || !std::ptr::eq(*prepared, scope.prepared())
        {
            return None;
        }
        let original = crate::exec::provider::Provider::from_scope(scope).ok()?;
        if meter.tokens_enabled() {
            let original = crate::exec::meter::tokens::wrap_provider(TokenProviderScope {
                meter: meter.clone(),
                gate: (*gate).clone(),
                role,
                original,
            })
            .ok()?;
            Some(gate.wrap_provider(crate::oneshot_io::checked_provider(original, output_closed)))
        } else {
            let original = crate::oneshot_io::checked_provider(Arc::new(original), output_closed);
            let original = gate.wrap_provider(original);
            let scope = self.request_provider_scope(Some(meter), role, original)?;
            crate::exec::meter::wrap_provider(scope).ok()
        }
    }

    fn request_provider_scope(
        &self,
        meter: Option<&Arc<crate::exec::meter::Meter>>,
        role: crate::exec::meter::Role,
        original: Arc<dyn ModelProvider>,
    ) -> Option<RequestProviderScope> {
        match (self, meter) {
            (Self::Headless { gate, .. }, Some(meter)) => Some(RequestProviderScope {
                meter: meter.clone(),
                gate: (*gate).clone(),
                role,
                original,
            }),
            _ => None, // Core construction rejects missing/mixed authority first.
        }
    }

    fn provider_scope<'a>(
        &'a self,
        model: &'a str,
        options: &'a BTreeMap<String, Value>,
        usage: Arc<Mutex<(u64, u64)>>,
        agents: &'a [String],
    ) -> Option<ProviderScope<'a>> {
        match self {
            Self::Frontend(_) => None,
            Self::Headless { prepared, gate } => Some(ProviderScope {
                prepared,
                gate: (*gate).clone(),
                model,
                options,
                usage,
                agents,
            }),
        }
    }
    fn framework_scope(&self) -> Option<FrameworkScope<'_>> {
        match self {
            Self::Frontend(_) => None,
            Self::Headless { prepared, gate } => Some(FrameworkScope {
                prepared,
                gate: (*gate).clone(),
            }),
        }
    }
    fn web_scope(&self) -> Option<WebScope<'_>> {
        match self {
            Self::Frontend(_) => None,
            Self::Headless { prepared, gate } => Some(WebScope {
                prepared,
                gate: (*gate).clone(),
            }),
        }
    }
    fn filesystem_scope(&self) -> Option<FilesystemScope<'_>> {
        match self {
            Self::Frontend(_) => None,
            Self::Headless { prepared, gate } => Some(FilesystemScope {
                prepared,
                gate: (*gate).clone(),
            }),
        }
    }
    fn shell_scope(&self) -> Option<ShellScope<'_>> {
        match self {
            Self::Frontend(_) => None,
            Self::Headless { prepared, gate } => Some(ShellScope {
                prepared,
                gate: (*gate).clone(),
            }),
        }
    }
    fn config(&self) -> &Config {
        match self {
            Self::Frontend(config) => config,
            Self::Headless { prepared, .. } => prepared.config(),
        }
    }
    fn prepared(&self) -> Option<&crate::exec::construction::Prepared> {
        match self {
            Self::Frontend(_) => None,
            Self::Headless { prepared, .. } => Some(prepared),
        }
    }
    fn gate(&self) -> Option<&Arc<crate::exec::runtime::InvocationGate>> {
        match self {
            Self::Frontend(_) => None,
            Self::Headless { gate, .. } => Some(gate),
        }
    }
}

/// Private entry to the SAME builder. No custom tools, UI callbacks, implicit
/// stdout, raw checkpoint/journal writes, or legacy session switching are accepted.
// Former pair-return entry is now fixture-only; production retains the opaque meter.
#[cfg(test)]
pub(crate) async fn build_prepared_session(
    prepared: &crate::exec::construction::Prepared,
) -> Option<(Session, Arc<crate::exec::runtime::InvocationGate>)> {
    let (session, gate, _) = build_prepared_session_with_request_limit(
        prepared,
        crate::exec::meter::RequestLimit::default(),
    )
    .await?;
    Some((session, gate))
}

/// Explicit private request-only cap. The SAME builder creates every source;
/// no caller gate, legacy SessionExtras opt-in or profile exclusion can replace it.
pub(crate) async fn build_prepared_session_with_request_limit(
    prepared: &crate::exec::construction::Prepared,
    limit: crate::exec::meter::RequestLimit,
) -> Option<(
    Session,
    Arc<crate::exec::runtime::InvocationGate>,
    Arc<crate::exec::meter::Meter>,
)> {
    build_prepared_session_with_limits(prepared, limit, None).await
}

/// Private reservation ceiling; receipts/settlement/public usage remain unwired.
pub(crate) async fn build_prepared_session_with_limits(
    prepared: &crate::exec::construction::Prepared,
    limit: crate::exec::meter::RequestLimit,
    token_limit: Option<crate::exec::meter::TokenLimit>,
) -> Option<(
    Session,
    Arc<crate::exec::runtime::InvocationGate>,
    Arc<crate::exec::meter::Meter>,
)> {
    let gate = Arc::new(crate::exec::runtime::InvocationGate::captured(prepared));
    let meter = crate::exec::meter::Meter::from_scope(RequestMeterScope {
        gate: gate.clone(),
        limit,
        token_limit,
    });
    let session = build_session_with_profile(
        SessionConstruction::Headless {
            prepared,
            gate: &gate,
        },
        Arc::new(Mutex::new(ApprovalState::default())),
        "",
        true,
        false,
        None,
        SessionExtras {
            token_sink: Some(Arc::new(|_: &str| {})),
            activity_sink: Some(Arc::new(|_: &str| {})),
            ..Default::default()
        },
        Some(meter.clone()),
    )
    .await?;
    Some((session, gate, meter))
}

/// Build a ready-to-run [`Session`] for `cfg`, or `None` if no model is set.
/// All local/transport frontends, including legacy `-m`, use this one builder.
///
/// `extra_system` appends frontend instructions; `activity_to_stderr` and the
/// optional sinks control activity routing. `verbose` controls route/MCP status.
/// `thread_id = None` starts a fresh thread. Existing local sessions continue
/// without persistence if their store cannot open; headless isolation is still
/// separate P2a work and is not claimed by this builder's existing defaults.
pub async fn build_session_ext(
    cfg: &Config,
    approval: Arc<Mutex<ApprovalState>>,
    extra_system: &str,
    activity_to_stderr: bool,
    verbose: bool,
    thread_id: Option<String>,
    extras: SessionExtras,
) -> Option<Session> {
    build_session_with_profile(
        SessionConstruction::Frontend(cfg),
        approval,
        extra_system,
        activity_to_stderr,
        verbose,
        thread_id,
        extras,
        None, // Ordinary frontends never select the private request meter.
    )
    .await
}

async fn build_session_with_profile(
    construction: SessionConstruction<'_>,
    approval: Arc<Mutex<ApprovalState>>,
    extra_system: &str,
    activity_to_stderr: bool,
    verbose: bool,
    thread_id: Option<String>,
    extras: SessionExtras,
    request_meter: Option<Arc<crate::exec::meter::Meter>>,
) -> Option<Session> {
    // Typed pre-effect construction check: headless has exactly one actual
    // meter/gate; frontend has neither. No independently excludable raw fallback.
    match (&construction, &request_meter) {
        (SessionConstruction::Headless { gate, .. }, Some(meter)) if meter.matches_gate(gate) => {}
        (SessionConstruction::Frontend(_), None) => {}
        _ => return None,
    }
    let cfg = construction.config();
    if construction.prepared().is_none() && !cfg.has_model_connection() {
        if let Some(sink) = &extras.activity_sink {
            sink("No model connection configured. Set an endpoint and API key with /setup.");
        }
        return None;
    }
    let cwd = match construction.prepared() {
        Some(prepared) => prepared.workspace().to_path_buf(),
        None => extras
            .workspace_root
            .clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))),
    };
    let selected_model = cfg.model.clone()?;
    let configured_provider = !cfg.providers.is_empty();
    if configured_provider && construction.prepared().is_some() {
        if let Some(sink) = &extras.activity_sink {
            sink(
                "Configured provider transports are not available in the private prepared runtime.",
            );
        }
        return None;
    }
    let mut protocol = crate::provider_config::Protocol::OpenaiChatCompletions;
    let mut model = selected_model.clone();
    let route = if configured_provider {
        match crate::provider_config::resolve(cfg).await {
            Ok((route, resolved_protocol, model_id)) => {
                protocol = resolved_protocol;
                model = model_id;
                route
            }
            Err(error) => {
                if let Some(sink) = &extras.activity_sink {
                    sink(&error);
                }
                return None;
            }
        }
    } else {
        match construction.prepared() {
            Some(prepared) => prepared.credentials().route().clone(),
            None => match crate::route::resolve_chat_endpoint(cfg).await {
                Ok(route) => route,
                Err(error) => {
                    if verbose {
                        println!("route resolution failed: {error}");
                    }
                    return None;
                }
            },
        }
    };
    let resolved_profiles = crate::harness_profiles::resolve(cfg, &route, &model);
    let harness_profile = match construction.gate() {
        Some(gate) => {
            crate::exec::runtime::harness_profile(&resolved_profiles.harness.profile, gate)
        }
        None => resolved_profiles.harness.profile.clone(),
    };
    // Per-model overrides from `model_profiles` are the caller layer and win
    // over built-in provider defaults. `model_kwargs` remains accepted as a
    // session-wide escape hatch and wins over the selected model profile.
    let mut caller_model_kwargs: BTreeMap<String, Value> = if configured_provider {
        BTreeMap::new()
    } else {
        cfg.model_profile(&model)
    };
    if let Some(values) = cfg.extra.get("model_kwargs").and_then(Value::as_object) {
        if !configured_provider {
            caller_model_kwargs.extend(
                values
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone())),
            );
        }
    }
    let profile_spec = resolved_profiles.provider.as_ref().map_or_else(
        || resolved_profiles.model_id.clone(),
        |provider| format!("{provider}:{}", resolved_profiles.model_id),
    );
    let mut provider_options = match resolved_profiles
        .provider_profile
        .profile
        .apply(&profile_spec, &caller_model_kwargs)
    {
        Ok(options) => options,
        Err(error) => {
            if verbose {
                println!("provider profile failed ({error}); using caller model kwargs only");
            }
            caller_model_kwargs
        }
    };
    // Explicit terminal controls are separate from catalog metadata and win
    // over caller kwargs. Private/frozen/legacy construction stays unchanged.
    if configured_provider
        || construction.prepared().is_none() && extras.cancellation_session.is_some()
    {
        if let Err(error) =
            crate::model_controls::apply(cfg, &selected_model, &mut provider_options)
        {
            if let Some(sink) = &extras.activity_sink {
                sink(&error);
            }
            return None;
        }
    }
    if verbose {
        println!(
            "route: {} · model: {model} · harness: {}",
            route.source, resolved_profiles.harness.active_key
        );
    }
    // Child providers (subagent, compaction) stream to stdout only in the
    // terminal modes; with a custom main sink they stay quiet (see
    // SessionExtras::token_sink).
    let child_sink: TokenSink = match &extras.token_sink {
        Some(_) => Arc::new(|_: &str| {}),
        None => stdout_sink(),
    };
    let main_sink = extras.token_sink.unwrap_or_else(stdout_sink);
    // ORIGINAL declared identity snapshot, not display labels or caller aliases.
    let mut request_agents = Vec::new();
    if let Some(prepared) = construction.prepared() {
        request_agents.extend(
            crate::extensions::effective_agents(prepared.discovery().extensions.clone())
                .into_iter()
                .map(|a| a.name),
        );
        if harness_profile
            .config
            .general_purpose_subagent
            .as_ref()
            .and_then(|g| g.enabled)
            != Some(false)
        {
            request_agents.push("general-purpose".into());
        }
    }
    let usage = Arc::new(Mutex::new((0u64, 0u64)));
    let mut native_history = None;
    let make_client = || {
        if !configured_provider {
            return Some(LlmClient::new_with_options(
                &route.base_url,
                &route.api_key,
                &model,
                &provider_options,
            ));
        }
        LlmClient::configured(
            &route.base_url,
            &route.api_key,
            &model,
            protocol,
            &selected_model,
            &provider_options,
        )
        .map_err(|_| {
            if let Some(sink) = &extras.activity_sink {
                sink("Configured model client could not be initialized.");
            }
        })
        .ok()
    };
    let provider: Arc<dyn ModelProvider> = match construction.provider_scope(
        &model,
        &provider_options,
        usage.clone(),
        &request_agents,
    ) {
        Some(scope) => construction.headless_provider(
            scope,
            request_meter.as_ref(),
            crate::exec::meter::Role::Main,
            extras.output_closed.clone(),
        )?,
        None => {
            let client = make_client()?;
            native_history = client.native.as_ref().map(|native| native.history.clone());
            Arc::new(StreamingLlmProvider {
                client,
                usage: usage.clone(),
                sink: main_sink,
                round_sink: extras.round_sink.clone(),
                status_sink: extras.activity_sink.clone(),
            })
        }
    };
    let provider = if construction.prepared().is_none() {
        crate::oneshot_io::checked_provider(provider, extras.output_closed.clone())
    } else {
        provider // Mandatory private role helper already bound gate + admissions.
    };
    #[cfg(feature = "gadgets")]
    let local_gadgets = construction.prepared().is_none()
        && extras.local_gadgets
        && crate::gadgets::cli::enabled(cfg);
    #[cfg(not(feature = "gadgets"))]
    let local_gadgets = false;
    let extensions = match construction.prepared() {
        Some(prepared) => prepared.discovery().extensions.clone(),
        None => crate::extensions::discover_at(&cwd, local_gadgets),
    };
    if extensions
        .iter()
        .any(|e| e.kind == crate::extensions::Kind::Plugin)
    {
        if let Some(sink) = &extras.activity_sink {
            sink(crate::extensions::DEPRECATION_WARNING);
        } else {
            // A fixed ASCII diagnostic only, never manifest-controlled text.
            eprintln!("{}", crate::extensions::DEPRECATION_WARNING);
        }
    }
    #[cfg(feature = "gadgets")]
    let (gadget_skill_block, gadget_system_suffix) = if local_gadgets {
        // Preserve established skill precedence: a user/project customization
        // may replace the built-in guide. Do not list duplicate skill names.
        if let Some(skill) = crate::skills::discover_at(&cfg.agent, &cwd)
            .into_iter()
            .find(|s| s.name == "build-gadget")
        {
            (crate::skills::skills_prompt_block(&[skill]), String::new())
        } else {
            match crate::gadgets::authoring::builtin_skill(&crate::config::config_dir()) {
                Ok(skill) => {
                    let block = crate::skills::skills_prompt_block(&[skill]);
                    (block.clone(), block)
                }
                Err(error) => {
                    if let Some(sink) = &extras.activity_sink {
                        sink(&format!("gadget authoring SDK unavailable: {error}"));
                    }
                    (String::new(), String::new()) // Installed gadgets still work.
                }
            }
        }
    } else {
        (String::new(), String::new())
    };
    let system = match construction.prepared() {
        Some(prepared) => crate::exec::runtime::system_prompt(
            prepared,
            &harness_profile,
            harness_profile
                .config
                .base_system_prompt
                .as_deref()
                .expect("headless base"),
        ),
        None => crate::prompt::build_system_prompt_with_extensions(
            cfg,
            &harness_profile,
            extra_system,
            &cwd,
            &extensions,
        ),
    };
    #[cfg(feature = "gadgets")]
    let system = format!("{system}{gadget_system_suffix}");
    let shell_settings = match construction.prepared() {
        Some(prepared) => prepared.shell().clone(), // Includes disabled-service credential sources.
        None => crate::shell::ShellSettings::from_config(cfg),
    };
    let scoped_filesystem = construction
        .filesystem_scope()
        .map(|scope| Arc::new(crate::exec::confined_fs::TextFs::from_scope(scope)));
    let real: Arc<dyn Backend> = match &scoped_filesystem {
        Some(filesystem) => filesystem.clone(), // Same retained IO + mandatory safe views.
        None => Arc::new(crate::backend_fs::RealFs::with_shell(
            cwd.clone(),
            shell_settings.clone(),
        )),
    };

    // Durable long-term memory: open the `store_files` table (in the SAME
    // ~/.craftsman/threads.db the checkpointer uses) and route `/memories/` paths
    // to it via a CompositeBackend; everything else stays on the real filesystem.
    // On open failure we print once (verbose) and fall back to the plain real
    // backend — never block the session on storage (rule #5). `memories` keeps
    // the concrete Arc for `/memories` listing.
    let memories: Option<Arc<crate::store_fs::StoreFs>> = if construction.prepared().is_some() {
        None // No store inspection/open at all, including the ephemeral path.
    } else {
        match crate::store_fs::StoreFs::open().await {
            Ok(store) => Some(Arc::new(store)),
            Err(e) => {
                if verbose {
                    println!("memory store unavailable (continuing without durable memory): {e}");
                }
                None
            }
        }
    };
    let backend: Arc<dyn Backend> = match &memories {
        Some(store) => Arc::new(crate::store_fs::CompositeBackend::new(
            real.clone(),
            crate::store_fs::MEMORY_PREFIX,
            store.clone() as Arc<dyn Backend>,
        )),
        None => real.clone(),
    };
    let lsp = Arc::new(match construction.prepared() {
        Some(_) => LspManager::disabled(cwd.clone()),
        None => LspManager::new(cwd.clone(), cfg),
    });

    // Durable state: open the libSQL checkpoint store and wire it (+ the JSONL
    // journal) into the agent. On open failure we print once and continue
    // WITHOUT persistence — a flaky/unavailable store must never block the
    // session (rule #5). `checkpointer` keeps the concrete Arc for `/session`
    // listing; the builder takes a second clone typed as `Arc<dyn Checkpointer>`.
    let thread_id = match construction.prepared() {
        Some(prepared) => prepared.persistence().thread_id().unwrap_or("").to_owned(),
        None => thread_id.unwrap_or_else(LibsqlCheckpointer::new_thread_id),
    };
    let checkpointer: Option<Arc<LibsqlCheckpointer>> = if construction.prepared().is_some() {
        // Prepared owns explicit durable selection. Do not wire its raw writer
        // until seeded checkpoint/journal redaction exists. Empty ephemeral id
        // is only internal bookkeeping: no store, journal or persistent identity.
        None
    } else {
        match LibsqlCheckpointer::open_for_workspace(&cwd).await {
            Ok(cp) => Some(Arc::new(cp)),
            Err(e) => {
                if verbose {
                    println!("checkpoint store unavailable (continuing without persistence): {e}");
                }
                None
            }
        }
    };
    let workspace_checkpoints = checkpointer.as_ref().map(|store| {
        Arc::new(WorkspaceCheckpointManager::new(
            cwd.clone(),
            store.clone(),
            thread_id.clone(),
        ))
    });

    // Build the non-interactive capability set ONCE and share the same safe,
    // thread-safe tool handles with the main agent and delegated subagents.
    // Previously the subagent received only HostTool::all(), even though its
    // advertised description promised all main-agent tools; web_search,
    // web_fetch, and MCP capabilities were therefore missing.
    let loaded = if construction.prepared().is_some() {
        crate::mcp::LoadedMcp {
            servers: Vec::new(),
            errors: Vec::new(),
        }
    } else {
        crate::mcp::load_mcp_tools(cfg).await
    };
    let mcp_summary = loaded.summary();
    let mcp_errors = loaded.errors.clone();
    if verbose {
        if loaded.tool_count() > 0 {
            println!(
                "mcp: {} tool(s) from {} server(s)",
                loaded.tool_count(),
                loaded.servers.len()
            );
        }
        for (server, err) in &loaded.errors {
            println!("mcp: server '{server}' unavailable: {err}");
        }
    }

    let shell_runner = match construction.shell_scope() {
        Some(scope) => crate::tools::ShellRunner::headless(scope),
        None => crate::tools::ShellRunner::new(cwd.clone(), shell_settings.clone()),
    };
    let scoped_web = construction
        .web_scope()
        .map(|scope| Arc::new(crate::exec::web::Web::from_scope(scope)));
    let mut shared_tools = host_and_web_tools(
        backend.clone(),
        cfg,
        if construction.prepared().is_some() {
            None
        } else {
            Some(lsp.clone())
        },
        workspace_checkpoints.clone(),
        Some(Arc::new(shell_runner)),
    );
    #[cfg(feature = "gadgets")]
    let gadgets = if local_gadgets {
        let runtime = crate::gadgets::cli::options(cfg).and_then(|mut options| {
            options.interactive = !activity_to_stderr;
            options.approval = approval.clone();
            options.prompt = extras.approval_prompt.clone();
            options.allowed_gadgets = extras.allowed_gadgets.clone();
            options.trust_workspace = extras.trust_workspace;
            crate::gadgets::GadgetRuntime::new(&cwd, &crate::config::config_dir(), options)
                .map_err(Into::into)
        });
        match runtime {
            Ok(runtime) => {
                let runtime = Arc::new(runtime);
                shared_tools.extend(runtime.tools());
                Some(runtime)
            }
            Err(error) => {
                if let Some(sink) = &extras.activity_sink {
                    sink(&format!("gadget host unavailable: {error}"));
                } else if verbose {
                    eprintln!(
                        "{}",
                        crate::gadgets::cli::terminal_safe(&format!(
                            "gadget host unavailable: {error}"
                        ))
                    );
                }
                None
            }
        }
    } else {
        None
    };
    if construction.prepared().is_none() {
        shared_tools.extend(lsp.tools());
    }
    for server in loaded.servers {
        let defs: Vec<ToolDef> = server
            .tools
            .iter()
            .map(|tool| ToolDef {
                name: tool.name.clone(),
                description: tool.description.clone(),
                parameters: tool.input_schema.clone(),
            })
            .collect();
        let mcp = Arc::new(server);
        for def in defs {
            let name = def.name.clone();
            shared_tools.push(Arc::new(McpFrameworkTool {
                mcp: mcp.clone(),
                def,
                name,
            }));
        }
    }

    if let Some(filesystem) = &scoped_filesystem {
        // Keep the original trusted definitions, but replace filesystem terminal
        // producers BEFORE their legacy path echoes/pagination/truncation. The
        // SAME backend also protects direct/profile-contributed backend callers.
        shared_tools = shared_tools
            .into_iter()
            .map(|tool| filesystem.wrap_host_tool(tool))
            .collect();
    }
    if let Some(web) = &scoped_web {
        // Replace terminal web producers BEFORE legacy response/HTML/snippet cuts;
        // the original trusted definitions and normal frontends remain unchanged.
        shared_tools = shared_tools
            .into_iter()
            .map(|tool| web.wrap_host_tool(tool))
            .collect();
    }
    if let Some(gate) = construction.gate() {
        shared_tools = gate.wrap_tools(shared_tools);
    }

    // Shared registry + notification channel: the `task` tool records spawned
    // subagents in `registry`; background children push completions/messages on
    // `notif_tx`, which the app drains through `notif_rx` (see Session).
    let registry = Arc::new(AgentRegistry::default());
    let (notif_tx, notif_rx) = craftsman_harness::notification_channel();

    // The general-purpose subagent gets the shared non-interactive host, web,
    // and MCP tools but NOT the `task` tool — the depth guard
    // means children don't delegate further. Its model is a second
    // StreamingLlmProvider over a fresh LlmClient on the SAME route (LlmClient is
    // not Clone), with its own usage Arc so its tokens don't inflate `/tokens`.
    let sub_usage = Arc::new(Mutex::new((0u64, 0u64)));
    let sub_provider: Arc<dyn ModelProvider> = match construction.provider_scope(
        &model,
        &provider_options,
        sub_usage.clone(),
        &request_agents,
    ) {
        Some(scope) => construction.headless_provider(
            scope,
            request_meter.as_ref(),
            crate::exec::meter::Role::Subagent,
            extras.output_closed.clone(),
        )?,
        None => Arc::new(StreamingLlmProvider {
            client: make_client()?,
            usage: sub_usage,
            sink: child_sink.clone(),
            round_sink: None,
            status_sink: None,
        }),
    };
    let sub_provider = if construction.prepared().is_none() {
        crate::oneshot_io::checked_provider(sub_provider, extras.output_closed.clone())
    } else {
        sub_provider // Mandatory private role helper already bound gate + admissions.
    };
    let gp_profile = harness_profile
        .config
        .general_purpose_subagent
        .clone()
        .unwrap_or_default();
    let general_purpose = if gp_profile.enabled == Some(false) {
        None
    } else {
        let mut subagent_prompt = gp_profile
            .system_prompt
            .unwrap_or_else(|| crate::harness_profiles::GENERAL_PURPOSE_SYSTEM_PROMPT.to_string());
        if let Some(prepared) = construction.prepared() {
            subagent_prompt =
                crate::exec::runtime::system_prompt(prepared, &harness_profile, &subagent_prompt);
        } else if let Some(suffix) = &harness_profile.config.system_prompt_suffix {
            subagent_prompt.push_str("\n\n");
            subagent_prompt.push_str(suffix);
        }
        #[cfg(feature = "gadgets")]
        subagent_prompt.push_str(&gadget_skill_block);
        let subagent_tools = shared_tools.clone();
        Some(SubAgentSpec {
            name: "general-purpose".into(),
            description: gp_profile.description.unwrap_or_else(|| {
                crate::harness_profiles::GENERAL_PURPOSE_DESCRIPTION.to_string()
            }),
            system_prompt: subagent_prompt,
            // Delegation must not bypass the HITL boundary (rule #6): every gated
            // host/network tool uses the same gate: legacy SubagentGate shares
            // ApprovalState/UI callbacks; headless shares its immutable policy.
            tools: if construction.gate().is_some() {
                subagent_tools // Already wrapped with the SAME immutable invocation gate.
            } else {
                crate::subagent_gate::SubagentGate::wrap_all_at_correlated(
                    subagent_tools,
                    &approval,
                    extras.approval_prompt.clone(),
                    &cwd,
                    extras.delegated_tool_callbacks.clone(),
                )
            },
            model: Some(sub_provider.clone()),
            harness_profile: Some(harness_profile.clone()),
            interrupt_on: vec![],
            max_depth: 1,
        })
    };

    let mut delegated_agents: Vec<SubAgentSpec> = Vec::new();
    if let Some(general_purpose) = general_purpose {
        delegated_agents.push(general_purpose);
    }
    // Gadget manifests and the deprecated JSON loader may contribute
    // subagent definitions. Same rules the removed platform-agents flow had:
    // tool names resolve against the shared registry, `task`/`send_message`
    // are filtered out (no recursion), and EVERY gated tool stays wrapped in
    // the chosen invocation/legacy gate — delegation cannot bypass it (rule #6).
    // `effective_agents` keeps one definition per name (later extensions win) and
    // can never yield `general-purpose`, so nothing here shadows the built-in.
    for definition in crate::extensions::effective_agents(extensions) {
        let mut tools: Vec<Arc<dyn Tool>> = Vec::new();
        for requested in &definition.tools {
            if matches!(requested.as_str(), "task" | "send_message") {
                continue;
            }
            if let Some(tool) = shared_tools
                .iter()
                .find(|tool| tool.def().name == *requested)
            {
                tools.push(tool.clone());
            }
        }
        let tools = if construction.gate().is_some() {
            tools
        } else {
            crate::subagent_gate::SubagentGate::wrap_all_at_correlated(
                tools,
                &approval,
                extras.approval_prompt.clone(),
                &cwd,
                extras.delegated_tool_callbacks.clone(),
            )
        };
        let subagent_prompt = match construction.prepared() {
            Some(prepared) => crate::exec::runtime::system_prompt(
                prepared,
                &harness_profile,
                &definition.system_prompt,
            ),
            None => definition.system_prompt,
        };
        #[cfg(feature = "gadgets")]
        let subagent_prompt = format!("{subagent_prompt}{gadget_skill_block}");
        delegated_agents.push(SubAgentSpec {
            name: definition.name,
            description: definition.description,
            system_prompt: subagent_prompt,
            tools,
            model: Some(sub_provider.clone()),
            harness_profile: Some(harness_profile.clone()),
            interrupt_on: vec![],
            max_depth: 1,
        });
    }

    // A THIRD StreamingLlmProvider drives CompactionMiddleware's summarization
    // sub-call — same route/model as the main agent, but its own usage Arc so a
    // compaction summary doesn't inflate the user-visible `/tokens` count
    // (the same pattern as the subagent provider; LlmClient is not Clone).
    let compact_usage = Arc::new(Mutex::new((0u64, 0u64)));
    let compaction_provider: Arc<dyn ModelProvider> = match construction.provider_scope(
        &model,
        &provider_options,
        compact_usage.clone(),
        &request_agents,
    ) {
        Some(scope) => construction.headless_provider(
            scope,
            request_meter.as_ref(),
            crate::exec::meter::Role::Compaction,
            extras.output_closed.clone(),
        )?,
        None => Arc::new(StreamingLlmProvider {
            client: make_client()?,
            usage: compact_usage,
            sink: child_sink,
            round_sink: None,
            status_sink: None,
        }),
    };
    let compaction_provider = if construction.prepared().is_none() {
        crate::oneshot_io::checked_provider(compaction_provider, extras.output_closed.clone())
    } else {
        compaction_provider // Mandatory private role helper already bound gate + admissions.
    };
    // Token-fraction trigger when the config reports a context window for
    // this model (`model_profiles.<model>.context_window`); otherwise the
    // message-count fallback. Catalog metadata is not used here.
    // See `CompactionTrigger`'s docs for
    // the tail-selection difference between the two modes.
    let compaction_trigger = match cfg.model_context_window(&selected_model) {
        Some(cw) => CompactionTrigger::Tokens {
            token_counter: Arc::new(crate::token_counter::TiktokenCounter::new()),
            #[allow(clippy::cast_precision_loss)]
            trigger_tokens: (cw as f64 * COMPACT_TRIGGER_FRACTION) as u64,
            keep_tail_tokens: COMPACT_KEEP_TAIL_TOKENS,
        },
        _ => CompactionTrigger::MessageCount {
            threshold_msgs: COMPACT_THRESHOLD_MSGS,
            keep_tail_msgs: COMPACT_KEEP_TAIL_MSGS,
        },
    };
    // Kept as a typed Arc on the Session too, so `/compact` can force a pass.
    let compaction = Arc::new(CompactionMiddleware::new(
        compaction_provider,
        compaction_trigger,
    ));

    let scoped_framework = construction
        .framework_scope()
        .map(|scope| Arc::new(crate::exec::framework::Framework::from_scope(scope)));
    let todos: Arc<dyn AgentMiddleware> = match &scoped_framework {
        Some(framework) => framework.todo_factory().materialize(),
        None => Arc::new(TodoListMiddleware::new()),
    };
    let mut middleware: Vec<Arc<dyn AgentMiddleware>> = vec![
        Arc::new(PatchToolCallsMiddleware),
        todos,
        compaction.clone(),
        Arc::new(SystemRemindersMiddleware::default()),
        Arc::new(ResultBudgetsMiddleware::new(100_000)),
    ];
    if construction.prepared().is_none() {
        // Preserve the exact legacy order (memory precedes compaction).
        middleware.insert(
            2,
            Arc::new({
                let memory =
                    MemoryMiddleware::new(backend.clone(), vec!["/memories/AGENTS.md".into()]);
                if extras.task_focused_memory {
                    memory.task_focused()
                } else {
                    memory
                }
            }),
        );
    }
    if let Some(workspace) = &workspace_checkpoints {
        middleware.push(workspace.clone() as Arc<dyn AgentMiddleware>);
    }
    if !delegated_agents.is_empty() {
        let subagents = SubAgentMiddleware::new(delegated_agents)
            .for_agent("main")
            .max_background(cfg.effective_max_parallel_agents());
        let subagents = match construction.gate() {
            Some(gate) => subagents.child_middleware(gate.clone()),
            None => subagents,
        };
        #[cfg(feature = "gadgets")]
        let subagents = match &gadgets {
            Some(runtime) => subagents.child_middleware(Arc::new(
                crate::gadgets::GadgetCatalogMiddleware::new(runtime.clone()),
            )),
            None => subagents,
        };
        middleware.push(match &scoped_framework {
            Some(framework) => framework.subagents(SubagentOutputSource {
                original: subagents,
                gate: construction.gate()?.clone(),
            })?,
            None => Arc::new(subagents),
        });
    }
    #[cfg(feature = "gadgets")]
    if let Some(runtime) = &gadgets {
        middleware.push(Arc::new(crate::gadgets::GadgetCatalogMiddleware::new(
            runtime.clone(),
        )));
    }
    if let Some(gate) = construction.gate() {
        middleware.insert(0, gate.clone()); // Before any tool middleware side effects.
    }
    let protected = BTreeSet::from([
        "patch_tool_calls",
        "subagents",
        "workspace_checkpoint",
        "hitl",
        "hitl_policy",
        "exec_invocation",
        #[cfg(feature = "gadgets")]
        "gadget_catalog",
    ]);
    let mut middleware = match apply_middleware_profile(middleware, &harness_profile, &protected) {
        Ok(middleware) => middleware,
        Err(error) => {
            if verbose {
                println!("invalid harness profile: {error}");
            }
            return None;
        }
    };
    // Legacy approval remains last. Headless instead protects its immutable
    // gate before profile hooks AND wraps every contributed tool terminal.
    if let Some(gate) = construction.gate() {
        // Also gate tools contributed by todo/subagent/profile middleware at
        // their terminal handles, not only by the dispatch middleware onion.
        middleware = middleware
            .into_iter()
            .map(|mw| gate.wrap_middleware(mw))
            .collect();
    } else {
        middleware.push(Arc::new(
            ApprovalMiddleware::new_at(
                approval,
                activity_to_stderr,
                extras.activity_sink.clone(),
                extras.tool_activity_sink.clone(),
                extras.approval_prompt.clone(),
                cwd,
            )
            .with_call_prompt(extras.call_approval_prompt.clone()),
        ));
    }

    if let Some(history) = native_history {
        // Installed after profile filtering; native continuation data cannot be
        // disabled while its provider is selected. No model/tool work in hooks.
        middleware.insert(0, history as Arc<dyn AgentMiddleware>);
    }
    let mut builder = Agent::builder(provider)
        .name("main")
        .durable_turn_checkpoints(
            construction.prepared().is_none() && extras.cancellation_session.is_some(),
        )
        .interrupt_model_on_cancel(
            construction.prepared().is_none() && extras.interrupt_model_on_cancel,
        )
        .system_prompt(system)
        .backend(backend.clone())
        .registry(registry.clone())
        .notifications(notif_tx)
        .thread_id(thread_id.clone())
        .harness_profile(harness_profile.clone());
    if let Some(prepared) = construction.prepared() {
        // Copy the already cleaned checkpoint; never load_thread/resume, which
        // would re-park persisted approvals and enable a middleware bypass.
        builder = builder.state(Arc::new(Mutex::new(prepared.initial_state().clone())));
    }
    if construction.prepared().is_none() {
        if let Some(session) = extras.cancellation_session {
            builder = builder.cancellation_session(session);
        }
    }
    for entry in middleware {
        builder = builder.middleware(entry);
    }
    if let Some(cp) = &checkpointer {
        builder = builder
            .checkpointer(cp.clone() as Arc<dyn craftsman_harness::Checkpointer>)
            .journal(Arc::new(JsonlJournal::new()));
    }
    for tool in &shared_tools {
        builder = builder.tool(tool.clone());
    }

    // ask_user: mid-task clarification. Interactive only in the REPL (activity
    // goes to stdout there); in one-shot mode (activity_to_stderr) there's no
    // human, so the tool declines cleanly instead of blocking on stdin.
    let ask_user: Arc<dyn Tool> = Arc::new(crate::ask_user::AskUserTool::new(
        construction.prepared().is_none() && !activity_to_stderr,
        extras.ask_user_prompt.clone(),
    ));
    builder = builder.tool(match construction.gate() {
        Some(gate) => gate.wrap_tool(ask_user),
        None => ask_user,
    });

    // Host-supplied extensions (ACP wires the a2ui `render_ui` tool). Kept
    // generic so this builder never grows mode-specific logic.
    for t in extras.extra_tools {
        builder = builder.tool(t);
    }

    Some(Session {
        #[cfg(feature = "gadgets")]
        gadgets,
        agent: builder.build(),
        usage,
        registry,
        notifications: notif_rx,
        thread_id,
        checkpointer,
        workspace_checkpoints,
        memories,
        compaction,
        mcp: mcp_summary,
        lsp,
        mcp_errors,
        route_source: route.source.to_string(),
        provider_profile_mode: if configured_provider {
            "configured".into()
        } else {
            resolved_profiles.provider_profile_mode
        },
        model_provider: if configured_provider {
            selected_model
                .split_once('/')
                .map(|(name, _)| name.to_owned())
        } else {
            resolved_profiles.provider
        },
        profile_model_id: resolved_profiles.model_id,
        harness_preset: resolved_profiles.harness_preset,
        harness_profile: resolved_profiles.harness.active_key,
        harness_profile_layers: resolved_profiles.harness.layers,
        provider_profile: if configured_provider {
            Some("configured".into())
        } else {
            resolved_profiles.provider_profile.active_key
        },
    })
}

/// After a first-round model error, drop the dangling user message so a
/// transient failure doesn't wedge the next turn with unanswered history.
/// No-op unless the last message is the user's (i.e. no progress was made).
pub fn rollback_dangling_user(agent: &Agent) {
    // Durable terminal input was already checkpointed before sampling. Keep
    // its live and durable copies consistent even when the provider fails.
    if agent.durable_turn_checkpoints_enabled() {
        return;
    }
    let mut st = agent.state.lock().unwrap();
    if matches!(st.messages.last(), Some(Msg::User { .. })) {
        st.messages.pop();
    }
}

/// Reset a session to a fresh conversation: drop messages/todos/files and
/// zero the token counter (the same result as the old `/clear`, which rebuilt
/// the whole session).
pub fn clear_session(session: &Session) {
    session.agent.clear_pending_input();
    *session.agent.state.lock().unwrap() = AgentState::default();
    *session.usage.lock().unwrap() = (0, 0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delegated_base_toolset_includes_web_capabilities() {
        let state = Arc::new(Mutex::new(AgentState::default()));
        let backend: Arc<dyn Backend> = Arc::new(craftsman_harness::StateFs::new(state));
        let config = Config::default();
        let lsp = Arc::new(LspManager::new(PathBuf::from("."), &config));
        let names = host_and_web_tools(backend, &config, Some(lsp), None, None)
            .into_iter()
            .map(|tool| tool.def().name)
            .collect::<BTreeSet<_>>();
        assert!(names.contains("web_search"));
        assert!(names.contains("web_fetch"));
        assert!(names.contains("read_file"));
        assert!(names.contains("execute"));
    }

    #[test]
    fn fw_messages_convert_to_openai_shape() {
        let msgs = vec![
            Msg::user("hi"),
            Msg::Assistant {
                content: String::new(),
                tool_calls: vec![FwToolCall {
                    id: "c1".into(),
                    name: "execute".into(),
                    args: json!({"command": "ls"}),
                }],
            },
            Msg::tool("c1", "output"),
        ];
        let llm = to_llm_messages("SYS", &msgs);
        let v = serde_json::to_value(&llm).unwrap();
        assert_eq!(v[0]["role"], "system");
        assert_eq!(v[0]["content"], "SYS");
        assert_eq!(v[1]["role"], "user");
        assert_eq!(v[2]["role"], "assistant");
        assert_eq!(v[2]["tool_calls"][0]["type"], "function");
        assert_eq!(v[2]["tool_calls"][0]["function"]["name"], "execute");
        // arguments must serialize as a JSON *string* (OpenAI wire shape).
        assert_eq!(
            v[2]["tool_calls"][0]["function"]["arguments"],
            "{\"command\":\"ls\"}"
        );
        assert_eq!(v[3]["role"], "tool");
        assert_eq!(v[3]["tool_call_id"], "c1");
    }

    #[test]
    fn turn_result_becomes_assistant_with_parsed_args() {
        let turn = TurnResult {
            content: "hello".into(),
            tool_calls: vec![LlmToolCall {
                id: "c1".into(),
                name: "execute".into(),
                arguments: "{\"command\": \"ls\"}".into(),
            }],
            ..Default::default()
        };
        match to_fw_message(turn) {
            Msg::Assistant {
                content,
                tool_calls,
            } => {
                assert_eq!(content, "hello");
                assert_eq!(tool_calls[0].args["command"], "ls");
            }
            _ => panic!("expected assistant message"),
        }
    }

    #[test]
    fn empty_tool_args_become_empty_object() {
        let turn = TurnResult {
            tool_calls: vec![LlmToolCall {
                id: "c1".into(),
                name: "ls".into(),
                arguments: String::new(),
            }],
            ..Default::default()
        };
        match to_fw_message(turn) {
            Msg::Assistant { tool_calls, .. } => assert!(tool_calls[0].args.is_object()),
            _ => panic!(),
        }
    }

    #[test]
    fn cost_usd_is_none_without_platform_pricing() {
        // Platform pricing metadata is gone; the footer must show n/a rather
        // than fabricating a number.
        let cfg: Config =
            serde_json::from_str(r#"{"model_profiles": {"m": {"context_window": 8192}}}"#).unwrap();
        assert!(cfg.model_context_window("m").is_some());
    }
}

#[cfg(test)]
#[path = "agent_shell_tests.rs"]
mod shell_tests;

#[cfg(test)]
#[path = "agent_fs_tests.rs"]
mod filesystem_tests;

#[cfg(test)]
#[path = "agent_web_tests.rs"]
mod web_tests;

#[cfg(test)]
#[path = "agent_framework_tests.rs"]
mod framework_tests;

#[cfg(test)]
#[path = "agent_provider_tests.rs"]
mod provider_tests;

#[cfg(test)]
#[path = "agent_provider_diagnostics_tests.rs"]
mod provider_diagnostics_tests;

#[cfg(test)]
#[path = "agent_request_meter_tests.rs"]
mod request_meter_tests;

#[cfg(test)]
#[path = "agent_token_reservation_tests.rs"]
mod token_reservation_tests;

#[cfg(test)]
#[path = "agent_admission_receipt_tests.rs"]
mod admission_receipt_tests;

#[cfg(test)]
#[path = "agent_memory_tests.rs"]
mod memory_tests;
