//! `craftsman acp-serve` — Agent Client Protocol (ACP) agent over stdio.
//!
//! Newline-delimited JSON-RPC 2.0 on stdin/stdout, implementing the ACP agent
//! side as specified at <https://agentclientprotocol.com>:
//!
//! | Method | Direction | Notes |
//! |---|---|---|
//! | `initialize` | client → agent | negotiates `protocolVersion` + `agentCapabilities` |
//! | `session/new` | client → agent | requires `cwd`; returns the agent-chosen `sessionId` |
//! | `session/load` | client → agent | replays the conversation, then returns an object |
//! | `session/prompt` | client → agent | streams `session/update`, returns a `stopReason` |
//! | `session/cancel` | client → agent | notification; the pending prompt returns `cancelled` |
//! | `session/request_permission` | agent → client | `options[].optionId` / `outcome.optionId` |
//!
//! Everything Craftsman-specific rides the spec's two extension points, so a
//! standard ACP client works unmodified:
//!
//! * `_meta.craftsman.*` — thread id, token usage, cost.
//! * `_craftsman/*` methods — `session/request_input` (ask-user), `session/set_model`,
//!   `session/list`. A client that does not implement them just errors, and the
//!   agent degrades instead of hanging.
//!
//! A2UI surfaces are ordinary ACP `resource` content blocks carrying
//! `application/vnd.craftsman.a2ui+json`.
//!
//! One ACP session maps 1:1 onto one craftsman-harness conversation thread, so the
//! `sessionId` *is* the thread id and `session/load` resumes from libsql.

use crate::a2ui::{FrameSlot, RenderUiTool, SYSTEM_PROMPT_SUFFIX};
use crate::agent::{
    self, ApprovalPromptResult, AskUserPrompt, CallApprovalPrompt, Session, SessionExtras,
    TokenSink, ToolActivityEvent, ToolActivitySink,
};
use crate::checkpoints::LibsqlCheckpointer;
use crate::config::Config;
use crate::hitl::ApprovalState;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};

/// The ACP major version this agent implements.
pub const ACP_PROTOCOL_VERSION: u32 = 1;
/// The Craftsman ACP Profile revision advertised in `_meta.craftsman.profile`.
pub const CRAFTSMAN_ACP_PROFILE: &str = "v1";
/// A2UI surfaces travel as this resource mime type.
pub const A2UI_MIME_TYPE: &str = "application/vnd.craftsman.a2ui+json";

/// Craftsman extension methods advertised so a client can feature-detect instead
/// of guessing.
const CRAFTSMAN_EXTENSIONS: [&str; 3] = [
    "_craftsman/session/request_input",
    "_craftsman/session/set_model",
    "_craftsman/session/list",
];

/// Permission option ids offered to the client. The `kind` values are the ACP
/// UI hints; the ids are ours and come back verbatim in `outcome.optionId`.
const OPTION_ALLOW_ONCE: &str = "allow-once";
const OPTION_ALLOW_ALWAYS: &str = "allow-always";
const OPTION_REJECT: &str = "reject-once";

/// Outbound channel to the stdout writer task.
pub type OutboundTx = tokio::sync::mpsc::UnboundedSender<Value>;

struct PendingPermission {
    session_id: String,
    sender: std::sync::mpsc::Sender<ApprovalPromptResult>,
}
struct PendingQuestion {
    session_id: String,
    sender: std::sync::mpsc::Sender<Option<String>>,
}

/// One session owns its runtime, directory, configuration and output slot.
struct AcpSession {
    runtime: Session,
    cfg: Config,
    cwd: PathBuf,
    frame_slot: FrameSlot,
    prompts_open: Arc<Mutex<bool>>,
    approval: Arc<Mutex<ApprovalState>>,
}

struct InFlightPrompt {
    session_id: String,
    prompt_id: Value,
    agent: craftsman_harness::Agent,
    cancelled: Arc<std::sync::atomic::AtomicBool>,
    prompts_open: Arc<Mutex<bool>>,
}

const MAX_SESSIONS: usize = 64;

pub struct AcpServerState {
    pub cfg: Config,
    pub outbound_tx: OutboundTx,
    pending_permissions: Arc<Mutex<HashMap<String, PendingPermission>>>,
    pending_questions: Arc<Mutex<HashMap<String, PendingQuestion>>>,
    sessions: HashMap<String, AcpSession>,
    current_session_id: Option<String>,
    in_flight_prompt: Arc<Mutex<Option<InFlightPrompt>>>,
    turn_tasks: Vec<tokio::task::JoinHandle<()>>,
    req_counter: Arc<AtomicU64>,
    initialized: bool,
}

impl AcpServerState {
    pub fn new(cfg: Config, outbound_tx: OutboundTx, _workspace_root: Option<PathBuf>) -> Self {
        Self {
            cfg,
            outbound_tx,
            pending_permissions: Arc::new(Mutex::new(HashMap::new())),
            pending_questions: Arc::new(Mutex::new(HashMap::new())),
            sessions: HashMap::new(),
            current_session_id: None,
            in_flight_prompt: Arc::new(Mutex::new(None)),
            turn_tasks: Vec::new(),
            req_counter: Arc::new(AtomicU64::new(1)),
            initialized: false,
        }
    }

    fn busy(&self) -> bool {
        self.in_flight_prompt.lock().unwrap().is_some()
    }

    fn current(&self) -> Option<&AcpSession> {
        self.current_session_id
            .as_ref()
            .and_then(|id| self.sessions.get(id))
    }

    async fn create_session(
        &mut self,
        id: String,
        cwd: PathBuf,
        cfg: Config,
    ) -> Result<(), String> {
        if self.sessions.contains_key(&id) {
            return Err("session already exists; use session/load".into());
        }
        if self.sessions.len() >= MAX_SESSIONS {
            return Err("session limit reached".into());
        }
        let built = build_acp_session(self, &id, &cwd, &cfg, None)
            .await
            .ok_or("could not initialize the requested agent session")?;
        if let Some(cp) = &built.runtime.checkpointer {
            use craftsman_harness::Checkpointer;
            match cp.load_latest(&id).await {
                Ok(None) => {}
                Ok(Some(_)) => {
                    built.runtime.lsp.shutdown().await;
                    return Err("session already saved; use session/load".into());
                }
                Err(_) => {
                    built.runtime.lsp.shutdown().await;
                    return Err("could not check session identity".into());
                }
            }
        }
        if built.runtime.mcp_errors.iter().any(|(name, _)| {
            cfg.mcp_servers
                .iter()
                .any(|s| s.get("name").and_then(Value::as_str) == Some(name))
        }) {
            built.runtime.lsp.shutdown().await;
            return Err("session MCP server could not connect".into());
        }
        self.sessions.insert(id.clone(), built);
        self.current_session_id = Some(id);
        Ok(())
    }

    async fn load_session(&mut self, id: &str, cwd: PathBuf, cfg: Config) -> Result<(), String> {
        if let Some(session) = self.sessions.get(id) {
            if session.cfg.mcp_servers != cfg.mcp_servers {
                return Err("session MCP configuration does not match".into());
            }
            if session.cwd != cwd {
                return Err("session working directory does not match".into());
            }
            self.current_session_id = Some(id.to_owned());
            return Ok(());
        }
        if self.sessions.len() >= MAX_SESSIONS {
            return Err("session limit reached".into());
        }
        let mut built = build_acp_session(self, id, &cwd, &cfg, None)
            .await
            .ok_or("could not initialize the requested agent session")?;
        if !built.runtime.mcp_errors.is_empty() {
            built.runtime.lsp.shutdown().await;
            return Err("session MCP server could not connect".into());
        }
        let loaded = match built.runtime.load_thread(id).await {
            Ok(loaded) => loaded,
            Err(_) => {
                built.runtime.lsp.shutdown().await;
                return Err("could not load session".into());
            }
        };
        if !loaded {
            built.runtime.lsp.shutdown().await;
            return Err("session not found".into());
        }
        self.sessions.insert(id.to_owned(), built);
        self.current_session_id = Some(id.to_owned());
        Ok(())
    }

    /// Request a real cooperative stop before releasing any UI waiter. The
    /// owning turn emits its final response after actual completion and drain.
    pub fn cancel_in_flight(&self) -> bool {
        let active = self.in_flight_prompt.lock().unwrap();
        let Some(prompt) = active.as_ref() else {
            return false;
        };
        prompt.cancelled.store(true, Ordering::Release);
        prompt.agent.request_cancel();
        *prompt.prompts_open.lock().unwrap() = false;
        let sid = prompt.session_id.clone();
        drop(active);
        self.reject_waiters(Some(&sid));
        true
    }

    fn reject_waiters(&self, session_id: Option<&str>) {
        self.pending_permissions
            .lock()
            .unwrap()
            .retain(|_, waiter| {
                if session_id.is_none_or(|sid| sid == waiter.session_id) {
                    let _ = waiter
                        .sender
                        .send(ApprovalPromptResult::Reject("turn cancelled".into()));
                    false
                } else {
                    true
                }
            });
        self.pending_questions.lock().unwrap().retain(|_, waiter| {
            if session_id.is_none_or(|sid| sid == waiter.session_id) {
                let _ = waiter.sender.send(None);
                false
            } else {
                true
            }
        });
    }

    async fn close(&mut self) {
        self.cancel_in_flight();
        for session in self.sessions.values() {
            *session.prompts_open.lock().unwrap() = false;
            session.runtime.agent.request_cancel();
        }
        self.reject_waiters(None);
        for task in self.turn_tasks.drain(..) {
            let _ = task.await;
        }
        for session in self.sessions.values() {
            for task in session.runtime.registry.take_background_joins() {
                let _ = task.await;
            }
            session.runtime.lsp.shutdown().await;
        }
    }
}

// ── JSON-RPC emit helpers ───────────────────────────────────────────────────

fn respond(tx: &OutboundTx, id: Value, result: Value) {
    let _ = tx.send(json!({ "jsonrpc": "2.0", "id": id, "result": result }));
}

fn respond_error(tx: &OutboundTx, id: Value, code: i32, message: &str) {
    let _ = tx.send(json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message },
    }));
}

fn notify(tx: &OutboundTx, method: &str, params: Value) {
    let _ = tx.send(json!({ "jsonrpc": "2.0", "method": method, "params": params }));
}

/// Send one `session/update` notification.
fn session_update(tx: &OutboundTx, session_id: &str, update: Value) {
    notify(
        tx,
        "session/update",
        json!({ "sessionId": session_id, "update": update }),
    );
}

fn text_block(text: &str) -> Value {
    json!({ "type": "text", "text": text })
}

/// ACP `ToolKind` for a craftsman-harness tool name — a UI hint only.
fn tool_kind(tool: &str) -> &'static str {
    match tool {
        "read_file" | "cat" | "ls" | "glob" => "read",
        "grep" | "search" => "search",
        "write_file" | "edit_file" | "apply_patch" | "str_replace" => "edit",
        "delete_file" | "rm" => "delete",
        "move_file" | "mv" => "move",
        "execute" | "bash" | "shell" | "run" => "execute",
        "think" | "todo" | "plan" => "think",
        "web_fetch" | "web_search" | "fetch" => "fetch",
        _ => "other",
    }
}

// ── Session construction ────────────────────────────────────────────────────

/// Build a session whose sinks emit ACP `session/update` notifications.
async fn build_acp_session(
    state: &AcpServerState,
    session_id: &str,
    cwd: &PathBuf,
    cfg: &Config,
    previous_approval: Option<Arc<Mutex<ApprovalState>>>,
) -> Option<AcpSession> {
    let messages = Arc::new(AtomicU64::new(0));
    let prompts_open = Arc::new(Mutex::new(false));
    let tool_ids = Arc::new(Mutex::new(HashMap::<String, String>::new()));
    let frame_slot: FrameSlot = Arc::new(Mutex::new(None));
    let approval = previous_approval.unwrap_or_else(|| {
        Arc::new(Mutex::new(
            ApprovalState {
                mode: cfg.permission_mode,
                auto_approve: cfg.auto_approve,
                auto_approve_network: cfg.auto_approve_web(),
                shell_allow_list: cfg.shell_allow_list.clone(),
            }
            .with_env_allow_list(),
        ))
    });

    let tx = state.outbound_tx.clone();
    let sid = session_id.to_owned();
    let message_ids = messages.clone();
    let token_sink: TokenSink = Arc::new(move |token: &str| {
        let session_id = sid.clone();
        session_update(
            &tx,
            &session_id,
            json!({
                "sessionUpdate": "agent_message_chunk",
                "messageId": format!("msg-{}", message_ids.load(Ordering::Relaxed)),
                "content": text_block(token),
            }),
        );
    });

    let tx = state.outbound_tx.clone();
    let sid = session_id.to_owned();
    let tool_activity_sink: ToolActivitySink = Arc::new(move |event: ToolActivityEvent| {
        let session_id = sid.clone();
        let update = match event {
            ToolActivityEvent::Started {
                call_id,
                tool,
                summary,
                details,
            } => {
                let title = if summary.trim().is_empty() {
                    tool.clone()
                } else {
                    summary
                };
                // `details` is a human string for most tools and JSON for some;
                // ACP's `rawInput` is an object either way.
                let raw_input = serde_json::from_str::<Value>(&details)
                    .ok()
                    .filter(Value::is_object)
                    .unwrap_or_else(|| json!({ "details": details }));
                json!({
                    "sessionUpdate": "tool_call",
                    "toolCallId": call_id,
                    "title": title,
                    "kind": tool_kind(&tool),
                    "status": "in_progress",
                    "rawInput": raw_input,
                    "_meta": { "craftsman": { "tool": tool } },
                })
            }
            ToolActivityEvent::Finished {
                call_id,
                tool,
                content,
                is_error,
            } => {
                let mut update = json!({
                    "sessionUpdate": "tool_call_update",
                    "toolCallId": call_id,
                    "status": if is_error { "failed" } else { "completed" },
                    "_meta": { "craftsman": { "tool": tool } },
                });
                if !content.trim().is_empty() {
                    update["content"] =
                        json!([{ "type": "content", "content": text_block(&content) }]);
                }
                update
            }
        };
        session_update(&tx, &session_id, update);
    });

    let main_tool_sink: ToolActivitySink = {
        let sink = tool_activity_sink.clone();
        let ids = tool_ids.clone();
        let counter = state.req_counter.clone();
        Arc::new(move |mut event| {
            match &mut event {
                ToolActivityEvent::Started { call_id, .. } => {
                    let presentation = format!("tool-{}", counter.fetch_add(1, Ordering::Relaxed));
                    ids.lock()
                        .unwrap()
                        .insert(call_id.clone(), presentation.clone());
                    *call_id = presentation;
                }
                ToolActivityEvent::Finished { call_id, .. } => {
                    if let Some(presentation) = ids.lock().unwrap().remove(call_id) {
                        *call_id = presentation;
                    }
                }
            }
            sink(event);
        })
    };

    let tx = state.outbound_tx.clone();
    let sid = session_id.to_owned();
    let pending = state.pending_permissions.clone();
    let counter = state.req_counter.clone();
    let permission_open = prompts_open.clone();
    let call_approval_prompt: CallApprovalPrompt = Arc::new(
        move |call: &craftsman_harness::ToolCall, summary: &str, details: &str| {
            let open = permission_open.lock().unwrap();
            if !*open {
                return ApprovalPromptResult::Reject("turn cancelled or inactive".into());
            }
            let tool = call.name.as_str();
            let req_id = format!("perm-{}", counter.fetch_add(1, Ordering::Relaxed));
            let session_id = sid.clone();
            let (waiter_tx, waiter_rx) = std::sync::mpsc::channel();
            pending.lock().unwrap().insert(
                req_id.clone(),
                PendingPermission {
                    session_id: session_id.clone(),
                    sender: waiter_tx,
                },
            );

            let raw_input = serde_json::from_str::<Value>(details)
                .ok()
                .filter(Value::is_object)
                .unwrap_or_else(|| json!({ "details": details }));
            let request = json!({
                "jsonrpc": "2.0",
                "id": req_id,
                "method": "session/request_permission",
                "params": {
                    "sessionId": session_id,
                    "toolCall": {
                        "toolCallId": call.id,
                        "title": if summary.trim().is_empty() { tool } else { summary },
                        "kind": tool_kind(tool),
                        "status": "pending",
                        "rawInput": raw_input,
                    },
                    "options": [
                        { "optionId": OPTION_ALLOW_ONCE, "name": "Allow once", "kind": "allow_once" },
                        { "optionId": OPTION_ALLOW_ALWAYS, "name": "Allow always", "kind": "allow_always" },
                        { "optionId": OPTION_REJECT, "name": "Reject", "kind": "reject_once" },
                    ],
                    "_meta": { "craftsman": { "tool": tool } },
                },
            });
            if tx.send(request).is_err() {
                pending.lock().unwrap().remove(&req_id);
                return ApprovalPromptResult::Reject("client disconnected".into());
            }
            drop(open);
            // Blocking wait: the client's response, or `cancel_in_flight`
            // draining the map, wakes this up.
            tokio::task::block_in_place(|| waiter_rx.recv())
                .unwrap_or_else(|_| ApprovalPromptResult::Reject("turn cancelled".into()))
        },
    );

    let tx = state.outbound_tx.clone();
    let sid = session_id.to_owned();
    let pending = state.pending_questions.clone();
    let counter = state.req_counter.clone();
    let question_open = prompts_open.clone();
    let ask_user_prompt: AskUserPrompt = Arc::new(move |question: &str| {
        let open = question_open.lock().unwrap();
        if !*open {
            return None;
        }
        let req_id = format!("ask-{}", counter.fetch_add(1, Ordering::Relaxed));
        let session_id = sid.clone();
        let (waiter_tx, waiter_rx) = std::sync::mpsc::channel();
        pending.lock().unwrap().insert(
            req_id.clone(),
            PendingQuestion {
                session_id: session_id.clone(),
                sender: waiter_tx,
            },
        );

        // ACP has no ask-user method; this is the documented Craftsman extension.
        // A client that does not implement it returns an error, which parses to
        // `None` and lets the agent continue without an answer.
        let request = json!({
            "jsonrpc": "2.0",
            "id": req_id,
            "method": "_craftsman/session/request_input",
            "params": { "sessionId": session_id, "question": question },
        });
        if tx.send(request).is_err() {
            pending.lock().unwrap().remove(&req_id);
            return None;
        }
        drop(open);
        tokio::task::block_in_place(|| waiter_rx.recv())
            .ok()
            .flatten()
    });

    let extras = SessionExtras {
        token_sink: Some(token_sink),
        tool_activity_sink: Some(main_tool_sink),
        call_approval_prompt: Some({
            let prompt = call_approval_prompt.clone();
            Arc::new(move |call, summary, details| {
                let Some(id) = tool_ids.lock().unwrap().get(&call.id).cloned() else {
                    return ApprovalPromptResult::Reject(
                        "tool invocation is not registered".into(),
                    );
                };
                let mut presented = call.clone();
                presented.id = id;
                prompt(&presented, summary, details)
            })
        }),
        delegated_tool_callbacks: Some((call_approval_prompt, tool_activity_sink)),
        ask_user_prompt: Some(ask_user_prompt),
        extra_tools: vec![Arc::new(RenderUiTool::new(frame_slot.clone()))],
        workspace_root: Some(cwd.clone()),
        interrupt_model_on_cancel: true,
        task_focused_memory: true,
        task_workflow: true,
        round_sink: Some({
            let messages = messages.clone();
            let counter = state.req_counter.clone();
            Arc::new(move || {
                messages.store(counter.fetch_add(1, Ordering::Relaxed), Ordering::Relaxed);
            })
        }),
        ..SessionExtras::default()
    };

    let runtime = agent::build_session_ext(
        cfg,
        approval.clone(),
        SYSTEM_PROMPT_SUFFIX,
        // Activity goes to stderr: stdout carries JSON-RPC only.
        true,
        false,
        Some(session_id.to_owned()),
        extras,
    )
    .await?;
    Some(AcpSession {
        runtime,
        cfg: cfg.clone(),
        cwd: cwd.clone(),
        frame_slot,
        prompts_open,
        approval,
    })
}

// ── Parameter / response parsing ────────────────────────────────────────────

/// Map an ACP `RequestPermissionOutcome` to the agent's approval result.
///
/// Only a standard selected outcome carrying an offered option can approve.
pub fn parse_approval_result(msg: &Value) -> ApprovalPromptResult {
    if let Some(error) = msg.get("error") {
        let reason = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("permission request failed");
        return ApprovalPromptResult::Reject(reason.to_string());
    }
    let result = msg.get("result").unwrap_or(&Value::Null);

    if let Some(outcome) = result.get("outcome") {
        return match outcome.get("outcome").and_then(Value::as_str) {
            Some("selected") => match outcome.get("optionId").and_then(Value::as_str) {
                Some(option) => decision_for_option(option),
                None => ApprovalPromptResult::Reject("no option selected".into()),
            },
            Some("cancelled") => ApprovalPromptResult::Reject("turn cancelled".into()),
            _ => ApprovalPromptResult::Reject(String::new()),
        };
    }

    ApprovalPromptResult::Reject(String::new())
}

/// Only exact offered option IDs grant permission; unknown values fail closed.
fn decision_for_option(option: &str) -> ApprovalPromptResult {
    match option {
        OPTION_ALLOW_ONCE => ApprovalPromptResult::Approve,
        OPTION_ALLOW_ALWAYS => ApprovalPromptResult::ApproveAlways,
        _ => ApprovalPromptResult::Reject(String::new()),
    }
}

/// Answer text from a `_craftsman/session/request_input` response.
pub fn parse_question_result(msg: &Value) -> Option<String> {
    if msg.get("error").is_some() {
        return None;
    }
    let result = msg.get("result")?;
    if let Some(answer) = result.as_str() {
        return Some(answer.to_string());
    }
    for key in ["answer", "text", "input"] {
        if let Some(answer) = result.get(key).and_then(Value::as_str) {
            return Some(answer.to_string());
        }
    }
    None
}

/// Flatten an ACP `prompt: ContentBlock[]` into the text the agent consumes.
///
/// `resource`/`resource_link` blocks contribute their text so embedded context
/// is not silently dropped; a bare string is tolerated for hand-written clients.
pub fn extract_prompt_text(params: &Value) -> Result<String, String> {
    let prompt = params
        .get("prompt")
        .ok_or_else(|| "missing 'prompt'".to_string())?;

    let mut parts: Vec<String> = Vec::new();
    match prompt {
        Value::String(text) => parts.push(text.clone()),
        Value::Array(blocks) => {
            for block in blocks {
                if let Some(text) = block.as_str() {
                    parts.push(text.to_string());
                    continue;
                }
                if let Some(text) = block.get("text").and_then(Value::as_str) {
                    parts.push(text.to_string());
                    continue;
                }
                if let Some(text) = block
                    .get("resource")
                    .and_then(|r| r.get("text"))
                    .and_then(Value::as_str)
                {
                    parts.push(text.to_string());
                    continue;
                }
                if let Some(uri) = block
                    .get("uri")
                    .or_else(|| block.get("resource").and_then(|r| r.get("uri")))
                    .and_then(Value::as_str)
                {
                    parts.push(uri.to_string());
                }
            }
        }
        Value::Object(_) => {
            if let Some(text) = prompt.get("text").and_then(Value::as_str) {
                parts.push(text.to_string());
            }
        }
        _ => {}
    }

    let text = parts.join("\n");
    if text.trim().is_empty() {
        return Err("'prompt' carried no text content".into());
    }
    Ok(text)
}

/// `sessionId` from params. `session_id` is tolerated for hand-written clients.
pub fn extract_session_id(params: &Value) -> Result<String, String> {
    params
        .get("sessionId")
        .or_else(|| params.get("session_id"))
        .and_then(Value::as_str)
        .filter(|id| safe_session_id(id))
        .map(str::to_owned)
        .ok_or_else(|| "invalid or missing 'sessionId'".into())
}

/// `_meta.craftsman.threadId`, the client's own conversation id when it has one.
fn replay_history(tx: &OutboundTx, sid: &str, messages: &[craftsman_harness::Msg]) {
    use craftsman_harness::Msg;
    let mut calls: HashMap<String, String> = HashMap::new();
    for (index, message) in messages.iter().enumerate() {
        match message {
            Msg::User { content } | Msg::Assistant { content, .. } if !content.is_empty() => {
                let kind = if matches!(message, Msg::User { .. }) {
                    "user_message_chunk"
                } else {
                    "agent_message_chunk"
                };
                session_update(
                    tx,
                    sid,
                    json!({"sessionUpdate":kind,"messageId":format!("replay-message-{index}"),"content":text_block(content)}),
                );
            }
            _ => {}
        }
        match message {
            Msg::Assistant { tool_calls, .. } => {
                for (part, call) in tool_calls.iter().enumerate() {
                    let id = format!("replay-tool-{index}-{part}");
                    calls.insert(call.id.clone(), id.clone());
                    session_update(
                        tx,
                        sid,
                        json!({"sessionUpdate":"tool_call","toolCallId":id,"title":call.name,"kind":tool_kind(&call.name),"status":"pending","rawInput":call.args}),
                    );
                }
            }
            Msg::Tool {
                tool_call_id,
                content,
                is_error,
            } => {
                if let Some(id) = calls.remove(tool_call_id) {
                    session_update(
                        tx,
                        sid,
                        json!({"sessionUpdate":"tool_call_update","toolCallId":id,"status":if *is_error {"failed"} else {"completed"},"content":[{"type":"content","content":text_block(content)}]}),
                    );
                } else {
                    session_update(
                        tx,
                        sid,
                        json!({"sessionUpdate":"agent_message_chunk","messageId":format!("replay-orphan-{index}"),"content":text_block(content),"_meta":{"craftsman":{"orphanedToolResult":true}}}),
                    );
                }
            }
            _ => {}
        }
    }
}

fn safe_session_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id != "."
        && id != ".."
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

fn requested_thread_id(params: &Value) -> Result<Option<String>, String> {
    let Some(value) = params.pointer("/_meta/craftsman/threadId") else {
        return Ok(None);
    };
    let id = value
        .as_str()
        .filter(|id| safe_session_id(id))
        .ok_or("invalid threadId")?;
    Ok(Some(id.to_owned()))
}

/// This adapter implements v1 only; an unsupported client version negotiates v1.
fn negotiate_protocol_version(params: &Value) -> u64 {
    let requested = params
        .get("protocolVersion")
        .and_then(Value::as_u64)
        .unwrap_or(u64::from(ACP_PROTOCOL_VERSION));
    if requested == u64::from(ACP_PROTOCOL_VERSION) {
        requested
    } else {
        u64::from(ACP_PROTOCOL_VERSION)
    }
}

/// Validate client-owned session inputs before filesystem/config/session work.
fn session_inputs(base: &Config, params: &Value) -> Result<(PathBuf, Config), String> {
    let cwd = params
        .get("cwd")
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
        .ok_or("missing absolute 'cwd'")?;
    let path = PathBuf::from(cwd);
    if !path.is_absolute() {
        return Err("'cwd' must be absolute".into());
    }
    let cwd = path
        .canonicalize()
        .map_err(|_| "session working directory is unavailable")?;
    if !cwd.is_dir() {
        return Err("'cwd' must name a directory".into());
    }
    let servers = params
        .get("mcpServers")
        .and_then(Value::as_array)
        .ok_or("missing 'mcpServers' array")?;
    if servers.len() > 64 {
        return Err("too many session MCP servers".into());
    }
    if params
        .get("additionalDirectories")
        .is_some_and(|v| !v.is_null() && v.as_array().is_none_or(|dirs| !dirs.is_empty()))
    {
        return Err("additional session directories are not supported".into());
    }
    if params.get("systemPrompt").is_some_and(|v| !v.is_null()) {
        return Err("client system prompts are not supported".into());
    }
    let mut cfg = base.clone();
    let mut names: HashSet<String> = cfg
        .mcp_servers
        .iter()
        .filter_map(|v| v.get("name").and_then(Value::as_str).map(str::to_owned))
        .collect();
    for server in servers {
        if server
            .get("type")
            .is_some_and(|v| v.as_str() != Some("stdio"))
            || server.get("url").is_some()
        {
            return Err("only standard stdio session MCP servers are supported".into());
        }
        let name = server
            .get("name")
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
            .ok_or("MCP server needs a name")?;
        if !names.insert(name.to_owned()) {
            return Err("duplicate MCP server name".into());
        }
        let command = server
            .get("command")
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
            .ok_or("only standard stdio session MCP servers are supported")?;
        let args = server
            .get("args")
            .and_then(Value::as_array)
            .ok_or("MCP server needs an args array")?;
        if args.iter().any(|v| !v.is_string()) {
            return Err("MCP args must be strings".into());
        }
        let env = server
            .get("env")
            .and_then(Value::as_array)
            .ok_or("MCP server needs an env array")?;
        let mut values = serde_json::Map::new();
        for item in env {
            let key = item
                .get("name")
                .and_then(Value::as_str)
                .filter(|v| !v.is_empty())
                .ok_or("MCP env needs a name")?;
            let value = item
                .get("value")
                .and_then(Value::as_str)
                .ok_or("MCP env values must be strings")?;
            if value.contains("${") {
                return Err("session MCP env values must be literal".into());
            }
            if values
                .insert(key.into(), Value::String(value.into()))
                .is_some()
            {
                return Err("duplicate MCP env name".into());
            }
        }
        cfg.mcp_servers.push(
            json!({"name":name,"transport":"stdio","command":command,"args":args,"env":values,"_craftsman_cwd":cwd}),
        );
    }
    Ok((cwd, cfg))
}

fn valid_rpc_id(id: &Value) -> bool {
    id.is_null() || id.is_string() || id.as_i64().is_some() || id.as_u64().is_some()
}

// ── Message dispatch ────────────────────────────────────────────────────────

/// Handle one inbound JSON-RPC message.
#[allow(clippy::too_many_lines)] // one match arm per protocol method reads better than 12 indirections
pub async fn handle_message(state: &mut AcpServerState, msg: Value) {
    let id_opt = msg.get("id").cloned();
    if !msg.is_object()
        || msg.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
        || id_opt.as_ref().is_some_and(|id| !valid_rpc_id(id))
        || msg.get("method").is_some_and(|method| !method.is_string())
    {
        respond_error(
            &state.outbound_tx,
            id_opt.filter(valid_rpc_id).unwrap_or(Value::Null),
            -32600,
            "invalid request",
        );
        return;
    }
    let method_opt = msg.get("method").and_then(Value::as_str);
    if method_opt.is_none() {
        if let Some(id) = &id_opt {
            if msg.get("result").is_some() || msg.get("error").is_some() {
                resolve_pending_response(state, id, &msg);
            } else {
                respond_error(&state.outbound_tx, Value::Null, -32600, "invalid request");
            }
        }
        if id_opt.is_none() {
            respond_error(&state.outbound_tx, Value::Null, -32600, "invalid request");
        }
        return;
    }
    let Some(method) = method_opt else { return };
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    let tx = state.outbound_tx.clone();
    if msg.get("params").is_some_and(|v| !v.is_object()) {
        if let Some(id) = id_opt {
            respond_error(&tx, id, -32602, "params must be an object");
        }
        return;
    }

    match method {
        "initialize" => {
            let Some(id) = id_opt else { return };
            let Some(version) = params
                .get("protocolVersion")
                .and_then(Value::as_u64)
                .filter(|v| *v <= 65535)
            else {
                respond_error(&tx, id, -32602, "invalid protocolVersion");
                return;
            };
            state.initialized = true;
            let _ = version;
            respond(
                &tx,
                id,
                json!({
                    "protocolVersion": negotiate_protocol_version(&params),
                    "agentCapabilities": {
                        "loadSession": true,
                        "promptCapabilities": {
                            "image": false,
                            "audio": false,
                            "embeddedContext": true,
                        },
                    },
                    "authMethods": [],
                    "agentInfo": {
                        "name": "craftsman-harness",
                        "version": env!("CARGO_PKG_VERSION"),
                    },
                    "_meta": {
                        "craftsman": {
                            "profile": CRAFTSMAN_ACP_PROFILE,
                            "harness": "craftsman-harness",
                            "a2uiMimeType": A2UI_MIME_TYPE,
                            "extensions": CRAFTSMAN_EXTENSIONS,
                        }
                    },
                }),
            );
        }

        "session/new" => {
            let Some(id) = id_opt else { return };
            if !state.initialized {
                respond_error(&tx, id, -32600, "initialize first");
                return;
            }
            if state.busy() {
                respond_error(&tx, id, -32001, "a prompt is active");
                return;
            }
            let (cwd, cfg) = match session_inputs(&state.cfg, &params) {
                Ok(v) => v,
                Err(e) => return respond_error(&tx, id, -32602, &e),
            };
            let session_id = match requested_thread_id(&params) {
                Ok(id) => id.unwrap_or_else(LibsqlCheckpointer::new_thread_id),
                Err(e) => return respond_error(&tx, id, -32602, &e),
            };
            if let Err(e) = state.create_session(session_id.clone(), cwd, cfg).await {
                respond_error(&tx, id, -32603, &e);
                return;
            }
            respond(
                &tx,
                id,
                json!({"sessionId":session_id,"_meta":{"craftsman":{"threadId":session_id}}}),
            );
        }
        "session/load" => {
            let Some(id) = id_opt else { return };
            if !state.initialized {
                respond_error(&tx, id, -32600, "initialize first");
                return;
            }
            if state.busy() {
                respond_error(&tx, id, -32001, "a prompt is active");
                return;
            }
            let session_id = match extract_session_id(&params) {
                Ok(v) if !v.is_empty() => v,
                _ => return respond_error(&tx, id, -32602, "invalid sessionId"),
            };
            let (cwd, cfg) = match session_inputs(&state.cfg, &params) {
                Ok(v) => v,
                Err(e) => return respond_error(&tx, id, -32602, &e),
            };
            if let Err(e) = state.load_session(&session_id, cwd, cfg).await {
                respond_error(&tx, id, -32603, &e);
                return;
            }
            let runtime = &state.sessions[&session_id].runtime;
            replay_history(
                &tx,
                &session_id,
                &runtime.agent.state.lock().unwrap().messages,
            );
            respond(&tx, id, json!({}));
        }
        "session/prompt" => {
            let Some(id) = id_opt else { return };
            if !state.initialized {
                respond_error(&tx, id, -32600, "initialize first");
                return;
            }
            if state.busy() {
                respond_error(&tx, id, -32001, "a prompt is active");
                return;
            }
            let session_id = match extract_session_id(&params) {
                Ok(v) => v,
                Err(e) => return respond_error(&tx, id, -32602, &e),
            };
            if !state.sessions.contains_key(&session_id) {
                respond_error(&tx, id, -32602, "unknown sessionId");
                return;
            }
            let prompt_text = match extract_prompt_text(&params) {
                Ok(v) => v,
                Err(e) => return respond_error(&tx, id, -32602, &e),
            };
            state.current_session_id = Some(session_id.clone());
            start_prompt_turn(state, id, session_id, prompt_text);
        }

        "session/cancel" => {
            let matches_target = extract_session_id(&params).is_ok_and(|target| {
                state
                    .in_flight_prompt
                    .lock()
                    .unwrap()
                    .as_ref()
                    .is_some_and(|prompt| prompt.session_id == target)
            });
            if matches_target {
                state.cancel_in_flight();
            }
        }

        "_craftsman/session/set_model" => {
            let Some(id) = id_opt else { return };
            if state.busy() {
                respond_error(&tx, id, -32001, "a prompt is active");
                return;
            }
            if !state.initialized {
                return respond_error(&tx, id, -32600, "initialize first");
            }
            if params.get("sessionId").is_some() {
                let sid = match extract_session_id(&params) {
                    Ok(sid) if state.sessions.contains_key(&sid) => sid,
                    _ => return respond_error(&tx, id, -32602, "unknown sessionId"),
                };
                state.current_session_id = Some(sid);
            }
            let model = params
                .get("modelId")
                .or_else(|| params.get("model"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_string();
            if model.is_empty() {
                respond_error(&tx, id, -32602, "missing 'modelId'");
                return;
            }
            match set_model(state, model.clone()).await {
                Ok(()) => respond(&tx, id, json!({ "modelId": model })),
                Err(e) => respond_error(&tx, id, -32603, &e),
            }
        }

        "_craftsman/session/list" => {
            let Some(id) = id_opt else { return };
            respond(&tx, id, json!({ "sessions": list_sessions(state).await }));
        }

        // Notifications the client may send after `initialize`.
        "initialized" | "notifications/initialized" => {}

        unknown => {
            // Unknown *notifications* are ignored; unknown requests are errors.
            if let Some(id) = id_opt {
                respond_error(&tx, id, -32601, &format!("method not found: {unknown}"));
            }
        }
    }
}

/// Route a client response to whichever server-initiated request is waiting.
fn resolve_pending_response(state: &AcpServerState, id: &Value, msg: &Value) {
    let key = match id {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => return,
    };
    if let Some(waiter) = state.pending_permissions.lock().unwrap().remove(&key) {
        let _ = waiter.sender.send(parse_approval_result(msg));
        return;
    }
    if let Some(waiter) = state.pending_questions.lock().unwrap().remove(&key) {
        let _ = waiter.sender.send(parse_question_result(msg));
    }
}

/// Spawn the turn task and record it as in-flight.
fn start_prompt_turn(
    state: &mut AcpServerState,
    prompt_id: Value,
    session_id: String,
    prompt_text: String,
) {
    let Some(session) = state.sessions.get(&session_id) else {
        respond_error(&state.outbound_tx, prompt_id, -32602, "unknown sessionId");
        return;
    };
    let agent = session.runtime.agent.clone();
    let usage = session.runtime.usage.clone();
    let usage_before = *usage.lock().unwrap();
    let frame_slot = session.frame_slot.clone();
    let prompts_open = session.prompts_open.clone();
    *prompts_open.lock().unwrap() = true;
    let tx = state.outbound_tx.clone();
    let in_flight = state.in_flight_prompt.clone();
    let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (a2ui_tx, mut a2ui_rx) = tokio::sync::mpsc::unbounded_channel::<Value>();
    *frame_slot.lock().unwrap() = Some(a2ui_tx);
    let a2ui_task = {
        let tx = tx.clone();
        let sid = session_id.clone();
        tokio::spawn(async move {
            while let Some(frame) = a2ui_rx.recv().await {
                let Some(messages) = frame.get("messages") else {
                    continue;
                };
                let Ok(payload) = serde_json::to_string(messages) else {
                    continue;
                };
                session_update(
                    &tx,
                    &sid,
                    json!({
                        "sessionUpdate":"agent_message_chunk",
                        "content":{"type":"resource","resource":{"uri":"craftsman:a2ui","mimeType":A2UI_MIME_TYPE,"text":payload}},
                        "_meta":{"craftsman":{"a2ui":true}}
                    }),
                );
            }
        })
    };
    let (start_tx, start_rx) = tokio::sync::oneshot::channel::<()>();
    *state.in_flight_prompt.lock().unwrap() = Some(InFlightPrompt {
        session_id: session_id.clone(),
        prompt_id: prompt_id.clone(),
        agent: agent.clone(),
        cancelled: cancelled.clone(),
        prompts_open: prompts_open.clone(),
    });
    let task = tokio::spawn(async move {
        let _ = start_rx.await;
        let outcome = if cancelled.load(Ordering::Acquire) {
            None
        } else {
            Some(agent.run_turn(&prompt_text).await)
        };
        *prompts_open.lock().unwrap() = false;
        *frame_slot.lock().unwrap() = None;
        let _ = a2ui_task.await;
        let usage_after = *usage.lock().unwrap();
        let mut active = in_flight.lock().unwrap();
        let is_cancelled = cancelled.load(Ordering::Acquire);
        let reason = if is_cancelled {
            "cancelled"
        } else {
            match outcome {
                Some(Ok(
                    craftsman_harness::TurnResult::Done(_)
                    | craftsman_harness::TurnResult::Suspended(_),
                )) => "end_turn",
                Some(Err(craftsman_harness::AgentError::MaxRounds(_))) => {
                    agent::rollback_dangling_user(&agent);
                    "max_turn_requests"
                }
                Some(Err(_)) | None => {
                    agent::rollback_dangling_user(&agent);
                    respond_error(&tx, prompt_id.clone(), -32603, "agent turn failed");
                    if active.as_ref().is_some_and(|p| p.prompt_id == prompt_id) {
                        active.take();
                    }
                    return;
                }
            }
        };
        respond(
            &tx,
            prompt_id.clone(),
            json!({"stopReason":reason,"_meta":{"craftsman":{"usage":{
                "inputTokens":usage_after.0.saturating_sub(usage_before.0),"outputTokens":usage_after.1.saturating_sub(usage_before.1)
            }}}}),
        );
        if active.as_ref().is_some_and(|p| p.prompt_id == prompt_id) {
            active.take();
        }
    });
    state.turn_tasks.retain(|task| !task.is_finished());
    state.turn_tasks.push(task);
    let _ = start_tx.send(());
}

/// Switch the configured model, rebuilding the session on the same thread.
/// Rolls back to the previous model when the rebuild fails.
async fn set_model(state: &mut AcpServerState, model: String) -> Result<(), String> {
    let id = state
        .current_session_id
        .clone()
        .ok_or("no selected session")?;
    let entry = state.sessions.get(&id).ok_or("unknown session")?;
    if entry
        .runtime
        .registry
        .details()
        .iter()
        .any(|task| task.status == craftsman_harness::AgentStatus::Running)
    {
        return Err("wait for session background tasks before changing model".into());
    }
    if entry.cfg.model.as_deref() == Some(&model) {
        return Ok(());
    }
    let mut cfg = entry.cfg.clone();
    cfg.model = Some(model);
    let cwd = entry.cwd.clone();
    let approval = entry.approval.clone();
    let mut rebuilt = build_acp_session(state, &id, &cwd, &cfg, Some(approval))
        .await
        .ok_or("agent rebuild failed for that model")?;
    if !rebuilt.runtime.mcp_errors.is_empty() {
        rebuilt.runtime.lsp.shutdown().await;
        return Err("session MCP server could not reconnect".into());
    }
    let previous = &state.sessions[&id].runtime;
    previous.agent.request_cancel();
    for task in previous.registry.take_background_joins() {
        let _ = task.await;
    }
    previous.lsp.shutdown().await;
    if let Err(error) = rebuilt.runtime.agent.inherit_conversation(&previous.agent) {
        rebuilt.runtime.lsp.shutdown().await;
        return Err(error.to_string());
    }
    *rebuilt.runtime.usage.lock().unwrap() = *previous.usage.lock().unwrap();
    rebuilt.runtime.set_thread_id(id.clone());
    state.sessions.insert(id, rebuilt);
    Ok(())
}

async fn list_sessions(state: &AcpServerState) -> Vec<Value> {
    use craftsman_harness::Checkpointer;
    let Some(checkpointer) = state
        .current()
        .and_then(|s| s.runtime.checkpointer.as_ref())
    else {
        return Vec::new();
    };
    checkpointer.list_threads().await.unwrap_or_default().into_iter().map(|thread| json!({
        "sessionId":thread.id,"title":thread.title,"createdAt":thread.created,"updatedAt":thread.updated
    })).collect()
}

// ── Transport ───────────────────────────────────────────────────────────────

/// Run the ACP agent over any newline-delimited reader/writer pair.
pub async fn run_with_io<R, W>(
    cfg: Config,
    reader: R,
    mut writer: W,
    workspace_root: Option<PathBuf>,
) -> anyhow::Result<()>
where
    R: AsyncBufRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (outbound_tx, mut outbound_rx) = tokio::sync::mpsc::unbounded_channel::<Value>();
    let mut state = AcpServerState::new(cfg, outbound_tx.clone(), workspace_root);

    // One writer task keeps frames whole and ordered across turn tasks.
    let writer_task = tokio::spawn(async move {
        while let Some(msg) = outbound_rx.recv().await {
            let Ok(line) = serde_json::to_string(&msg) else {
                continue;
            };
            if writer.write_all(line.as_bytes()).await.is_err()
                || writer.write_all(b"\n").await.is_err()
                || writer.flush().await.is_err()
            {
                break;
            }
        }
    });

    let mut lines = reader.lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(line) {
            Ok(msg) => handle_message(&mut state, msg).await,
            Err(_) => {
                let _ = outbound_tx.send(json!({
                    "jsonrpc": "2.0",
                    "id": Value::Null,
                    "error": { "code": -32700, "message": "parse error" },
                }));
            }
        }
    }

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    let close = tokio::time::timeout_at(deadline, state.close()).await;
    drop(state);
    drop(outbound_tx);
    if close.is_err() {
        writer_task.abort();
        let _ = writer_task.await;
        anyhow::bail!("ACP shutdown timed out; work may still be completing");
    }
    let mut writer_task = writer_task;
    if tokio::time::timeout_at(deadline, &mut writer_task)
        .await
        .is_err()
    {
        writer_task.abort();
        let _ = writer_task.await;
        anyhow::bail!("ACP output drain timed out; final frames may be undelivered");
    }
    Ok(())
}

/// Entrypoint for `craftsman acp-serve`.
pub async fn run(cfg: Config) -> anyhow::Result<()> {
    let stdin = tokio::io::BufReader::new(tokio::io::stdin());
    run_with_io(cfg, stdin, tokio::io::stdout(), None).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::duplex;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

    fn sse(events: &[&str]) -> String {
        let mut out = String::new();
        for event in events {
            out.push_str("data: ");
            out.push_str(event);
            out.push_str("\n\n");
        }
        out.push_str("data: [DONE]\n\n");
        out
    }

    /// Drive a server over an in-memory duplex pair, like a client would.
    struct Harness {
        writer: tokio::io::DuplexStream,
        lines: tokio::io::Lines<tokio::io::BufReader<tokio::io::DuplexStream>>,
        server: tokio::task::JoinHandle<anyhow::Result<()>>,
    }

    impl Harness {
        fn start(cfg: Config, workspace_root: Option<PathBuf>) -> Harness {
            let (writer, server_reader) = duplex(8192);
            let (server_writer, reader) = duplex(8192);
            let server = tokio::spawn(async move {
                run_with_io(
                    cfg,
                    tokio::io::BufReader::new(server_reader),
                    server_writer,
                    workspace_root,
                )
                .await
            });
            Harness {
                writer,
                lines: tokio::io::BufReader::new(reader).lines(),
                server,
            }
        }

        async fn initialize(&mut self) {
            self.send(json!({"jsonrpc":"2.0","id":"init","method":"initialize","params":{"protocolVersion":1}})).await;
            assert_eq!(self.next().await["result"]["protocolVersion"], 1);
        }

        async fn new_session(&mut self, prefix: &str, cwd: &std::path::Path) -> String {
            let id = format!("{prefix}-{}", LibsqlCheckpointer::new_thread_id());
            self.initialize().await;
            self.send(json!({"jsonrpc":"2.0","id":"new","method":"session/new","params":{"cwd":cwd,"mcpServers":[],"_meta":{"craftsman":{"threadId":id}}}})).await;
            assert_eq!(self.next().await["result"]["sessionId"], id);
            id
        }

        async fn send(&mut self, msg: Value) {
            self.writer
                .write_all(format!("{msg}\n").as_bytes())
                .await
                .unwrap();
            self.writer.flush().await.unwrap();
        }

        async fn next(&mut self) -> Value {
            let line = self
                .lines
                .next_line()
                .await
                .unwrap()
                .expect("server closed the stream");
            serde_json::from_str(&line).unwrap()
        }

        /// Read until the response to `id`, collecting notifications on the way.
        async fn until_response(&mut self, id: Value) -> (Value, Vec<Value>) {
            let mut notifications = Vec::new();
            loop {
                let msg = self.next().await;
                if msg.get("id") == Some(&id) && msg.get("method").is_none() {
                    return (msg, notifications);
                }
                notifications.push(msg);
            }
        }

        async fn finish(mut self) {
            self.writer.shutdown().await.ok();
            drop(self.writer);
            let _ = self.server.await;
        }
    }

    fn model_cfg(base_url: String, auto_approve: bool) -> Config {
        Config {
            base_url,
            api_key: "test-key".into(),
            model: Some("test-model".into()),
            auto_approve,
            ..Default::default()
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn initialize_answers_with_the_spec_capability_shape() {
        let mut harness = Harness::start(Config::default(), None);
        harness
            .send(json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {
                    "protocolVersion": 1,
                    "clientCapabilities": { "fs": { "readTextFile": false, "writeTextFile": false } },
                    "clientInfo": { "name": "test-client", "version": "1.0.0" },
                },
            }))
            .await;

        let response = harness.next().await;
        assert_eq!(response["result"]["protocolVersion"], 1);
        assert_eq!(response["result"]["agentCapabilities"]["loadSession"], true);
        assert!(response["result"]["authMethods"].is_array());
        assert_eq!(response["result"]["agentInfo"]["name"], "craftsman-harness");
        assert_eq!(response["result"]["_meta"]["craftsman"]["profile"], "v1");
        // No MCP-style aliases leak into the ACP surface.
        assert!(response["result"]["serverInfo"].is_null());
        assert!(response["result"]["capabilities"].is_null());
        harness.finish().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn initialize_caps_a_newer_client_to_the_supported_version() {
        let mut harness = Harness::start(Config::default(), None);
        harness
            .send(json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": { "protocolVersion": 99 },
            }))
            .await;
        assert_eq!(harness.next().await["result"]["protocolVersion"], 1);
        harness.finish().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unknown_request_is_method_not_found_and_a_response_is_ignored() {
        let mut harness = Harness::start(Config::default(), None);
        // A response for an id nobody is waiting on must not be answered.
        harness
            .send(json!({"jsonrpc": "2.0", "id": "stale", "result": {"outcome": {"outcome": "cancelled"}}}))
            .await;
        harness
            .send(json!({"jsonrpc": "2.0", "id": 9, "method": "does/not/exist"}))
            .await;

        let response = harness.next().await;
        assert_eq!(response["id"], 9, "the stale response was answered");
        assert_eq!(response["error"]["code"], -32601);
        harness.finish().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn invalid_json_is_a_parse_error() {
        let mut harness = Harness::start(Config::default(), None);
        harness.writer.write_all(b"not json\n").await.unwrap();
        harness.writer.flush().await.unwrap();
        assert_eq!(harness.next().await["error"]["code"], -32700);
        harness.finish().await;
    }

    #[test]
    fn permission_outcomes_require_a_standard_offered_selection() {
        // Spec shape.
        assert_eq!(
            parse_approval_result(
                &json!({"result": {"outcome": {"outcome": "selected", "optionId": "allow-once"}}})
            ),
            ApprovalPromptResult::Approve
        );
        assert_eq!(
            parse_approval_result(
                &json!({"result": {"outcome": {"outcome": "selected", "optionId": "allow-always"}}})
            ),
            ApprovalPromptResult::ApproveAlways
        );
        assert_eq!(
            parse_approval_result(
                &json!({"result": {"outcome": {"outcome": "selected", "optionId": "reject-once"}}})
            ),
            ApprovalPromptResult::Reject(String::new())
        );
        assert_eq!(
            parse_approval_result(&json!({"result": {"outcome": {"outcome": "cancelled"}}})),
            ApprovalPromptResult::Reject("turn cancelled".into())
        );

        // Legacy loose shapes must not grant permission.
        assert_eq!(
            parse_approval_result(&json!({"result": true})),
            ApprovalPromptResult::Reject(String::new())
        );
        assert_eq!(
            parse_approval_result(&json!({"result": "approve_always"})),
            ApprovalPromptResult::Reject(String::new())
        );
        assert_eq!(
            parse_approval_result(&json!({"result": {"decision": "reject", "feedback": "unsafe"}})),
            ApprovalPromptResult::Reject(String::new())
        );
        assert_eq!(
            parse_approval_result(&json!({"error": {"message": "denied by policy"}})),
            ApprovalPromptResult::Reject("denied by policy".into())
        );
    }

    #[test]
    fn prompt_content_blocks_flatten_to_text() {
        assert_eq!(
            extract_prompt_text(&json!({"prompt": [
                {"type": "text", "text": "first"},
                {"type": "resource", "resource": {"uri": "file:///a", "text": "second"}},
            ]}))
            .unwrap(),
            "first\nsecond"
        );
        assert_eq!(
            extract_prompt_text(&json!({"prompt": "bare string"})).unwrap(),
            "bare string"
        );
        assert!(extract_prompt_text(&json!({"prompt": []})).is_err());
        assert!(extract_prompt_text(&json!({})).is_err());
    }

    struct TextResponder;
    impl Respond for TextResponder {
        fn respond(&self, _req: &Request) -> ResponseTemplate {
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse(&[
                    r#"{"choices":[{"delta":{"content":"Hello from "}}]}"#,
                    r#"{"choices":[{"delta":{"content":"ACP!"}}]}"#,
                    r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
                    r#"{"choices":[],"usage":{"prompt_tokens":12,"completion_tokens":6}}"#,
                ]))
        }
    }

    async fn mock_model(responder: impl Respond + 'static) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(responder)
            .mount(&server)
            .await;
        server
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn session_new_honours_the_client_thread_id_then_streams_a_turn() {
        let model = mock_model(TextResponder).await;
        let sid = format!("thread-42-{}", LibsqlCheckpointer::new_thread_id());
        let dir = tempfile::tempdir().unwrap();
        let mut harness = Harness::start(model_cfg(model.uri(), true), None);
        harness.initialize().await;

        harness
            .send(json!({
                "jsonrpc": "2.0", "id": 1, "method": "session/new",
                "params": {
                    "cwd": dir.path(),
                    "mcpServers": [],
                    "_meta": { "craftsman": { "threadId": sid } },
                },
            }))
            .await;
        let created = harness.next().await;
        assert_eq!(
            created["result"]["sessionId"], sid,
            "the client's conversation id must be adopted, not discarded"
        );

        harness
            .send(json!({
                "jsonrpc": "2.0", "id": 2, "method": "session/prompt",
                "params": {
                    "sessionId": sid,
                    "prompt": [{"type": "text", "text": "Say hello"}],
                },
            }))
            .await;
        let (response, notifications) = harness.until_response(json!(2)).await;

        assert_eq!(response["result"]["stopReason"], "end_turn");
        assert_eq!(
            response["result"]["_meta"]["craftsman"]["usage"]["inputTokens"],
            12
        );
        assert_eq!(
            response["result"]["_meta"]["craftsman"]["usage"]["outputTokens"],
            6
        );

        let streamed: String = notifications
            .iter()
            .filter(|msg| msg["method"] == "session/update")
            .filter(|msg| msg["params"]["update"]["sessionUpdate"] == "agent_message_chunk")
            .filter_map(|msg| msg["params"]["update"]["content"]["text"].as_str())
            .collect();
        assert_eq!(streamed, "Hello from ACP!");
        assert!(
            notifications
                .iter()
                .all(|msg| msg["params"]["sessionId"] == sid)
        );
        harness.finish().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn session_load_replays_the_conversation_then_returns_an_object() {
        let model = mock_model(TextResponder).await;
        let sid = format!("thread-load-{}", LibsqlCheckpointer::new_thread_id());
        let dir = tempfile::tempdir().unwrap();
        let mut harness = Harness::start(model_cfg(model.uri(), true), None);
        harness.initialize().await;

        harness
            .send(json!({
                "jsonrpc": "2.0", "id": 1, "method": "session/new",
                "params": { "cwd": dir.path(), "mcpServers": [],
                            "_meta": { "craftsman": { "threadId": sid } } },
            }))
            .await;
        harness.next().await;
        harness
            .send(json!({
                "jsonrpc": "2.0", "id": 2, "method": "session/prompt",
                "params": { "sessionId": sid, "prompt": [{"type": "text", "text": "remember me"}] },
            }))
            .await;
        harness.until_response(json!(2)).await;

        harness
            .send(json!({
                "jsonrpc": "2.0", "id": 3, "method": "session/load",
                "params": { "sessionId": sid, "cwd": dir.path(), "mcpServers": [] },
            }))
            .await;
        let (response, replay) = harness.until_response(json!(3)).await;

        assert!(
            response["result"].is_object(),
            "session/load returns an object"
        );
        let kinds: Vec<&str> = replay
            .iter()
            .filter_map(|msg| msg["params"]["update"]["sessionUpdate"].as_str())
            .collect();
        assert!(kinds.contains(&"user_message_chunk"), "got {kinds:?}");
        assert!(kinds.contains(&"agent_message_chunk"), "got {kinds:?}");
        harness.finish().await;
    }

    struct ToolResponder;
    impl Respond for ToolResponder {
        fn respond(&self, req: &Request) -> ResponseTemplate {
            let body: Value = serde_json::from_slice(&req.body).unwrap();
            let answered = body["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["role"] == "tool");
            let events = if answered {
                sse(&[
                    r#"{"choices":[{"delta":{"content":"file written"}}]}"#,
                    r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
                    r#"{"choices":[],"usage":{"prompt_tokens":20,"completion_tokens":2}}"#,
                ])
            } else {
                sse(&[
                    r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"write_file","arguments":""}}]}}]}"#,
                    r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"file_path\": \"test.txt\", \"content\": \"hello\"}"}}]}}]}"#,
                    r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
                    r#"{"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":5}}"#,
                ])
            };
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(events)
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn tool_calls_stream_as_spec_tool_call_updates() {
        let model = mock_model(ToolResponder).await;
        let dir = tempfile::tempdir().unwrap();
        let mut harness =
            Harness::start(model_cfg(model.uri(), true), Some(dir.path().to_path_buf()));
        let sid = harness.new_session("tool-session", dir.path()).await;

        harness
            .send(json!({
                "jsonrpc": "2.0", "id": 1, "method": "session/prompt",
                "params": { "sessionId": sid, "prompt": [{"type": "text", "text": "write test.txt"}] },
            }))
            .await;
        let (_response, notifications) = harness.until_response(json!(1)).await;

        let started = notifications
            .iter()
            .find(|msg| msg["params"]["update"]["sessionUpdate"] == "tool_call")
            .expect("a tool_call update");
        assert_eq!(started["params"]["update"]["kind"], "edit");
        assert_eq!(started["params"]["update"]["status"], "in_progress");
        assert!(started["params"]["update"]["toolCallId"].is_string());

        let finished = notifications
            .iter()
            .find(|msg| msg["params"]["update"]["sessionUpdate"] == "tool_call_update")
            .expect("a tool_call_update update");
        assert_eq!(finished["params"]["update"]["status"], "completed");
        harness.finish().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn permission_request_uses_option_ids_and_accepts_an_outcome() {
        let model = mock_model(ToolResponder).await;
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = model_cfg(model.uri(), false);
        cfg.permission_mode = crate::config::PermissionMode::Manual;
        let mut harness = Harness::start(cfg, Some(dir.path().to_path_buf()));
        let sid = harness.new_session("hitl", dir.path()).await;

        harness
            .send(json!({
                "jsonrpc": "2.0", "id": 100, "method": "session/prompt",
                "params": { "sessionId": sid, "prompt": [{"type": "text", "text": "write test.txt"}] },
            }))
            .await;

        let request = loop {
            let msg = harness.next().await;
            if msg["method"] == "session/request_permission" {
                break msg;
            }
        };
        assert_eq!(request["params"]["toolCall"]["kind"], "edit");
        assert_eq!(
            request["params"]["options"][0]["optionId"],
            OPTION_ALLOW_ONCE
        );
        assert_eq!(request["params"]["options"][0]["kind"], "allow_once");

        harness
            .send(json!({
                "jsonrpc": "2.0",
                "id": request["id"],
                "result": { "outcome": { "outcome": "selected", "optionId": OPTION_ALLOW_ONCE } },
            }))
            .await;

        let (response, _) = harness.until_response(json!(100)).await;
        assert_eq!(response["result"]["stopReason"], "end_turn");
        assert!(dir.path().join("test.txt").exists());
        harness.finish().await;
    }

    struct A2uiResponder;
    impl Respond for A2uiResponder {
        fn respond(&self, req: &Request) -> ResponseTemplate {
            let body: Value = serde_json::from_slice(&req.body).unwrap();
            let answered = body["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["role"] == "tool");
            let events = if answered {
                sse(&[
                    r#"{"choices":[{"delta":{"content":"surface up"}}]}"#,
                    r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
                    r#"{"choices":[],"usage":{"prompt_tokens":25,"completion_tokens":3}}"#,
                ])
            } else {
                let args = json!({
                    "messages": [{
                        "version": "v0.9",
                        "createSurface": {
                            "surfaceId": "test-surface",
                            "catalogId": "https://a2ui.org/specification/v0_9/catalogs/basic/catalog.json",
                        },
                    }]
                });
                sse(&[
                    r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_ui","function":{"name":"render_ui","arguments":""}}]}}]}"#,
                    &format!(
                        r#"{{"choices":[{{"delta":{{"tool_calls":[{{"index":0,"function":{{"arguments":{}}}}}]}}}}]}}"#,
                        serde_json::to_string(&args.to_string()).unwrap()
                    ),
                    r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
                    r#"{"choices":[],"usage":{"prompt_tokens":15,"completion_tokens":8}}"#,
                ])
            };
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(events)
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a2ui_surfaces_ride_a_resource_content_block() {
        let model = mock_model(A2uiResponder).await;
        let dir = tempfile::tempdir().unwrap();
        let mut harness = Harness::start(model_cfg(model.uri(), true), None);
        let sid = harness.new_session("a2ui", dir.path()).await;

        harness
            .send(json!({
                "jsonrpc": "2.0", "id": 200, "method": "session/prompt",
                "params": { "sessionId": sid, "prompt": [{"type": "text", "text": "show ui"}] },
            }))
            .await;
        let (_response, notifications) = harness.until_response(json!(200)).await;

        let surface = notifications
            .iter()
            .find(|msg| {
                msg["params"]["update"]["content"]["resource"]["mimeType"] == A2UI_MIME_TYPE
            })
            .expect("an A2UI resource block");
        assert_eq!(
            surface["params"]["update"]["_meta"]["craftsman"]["a2ui"],
            true
        );
        assert!(
            surface["params"]["update"]["content"]["resource"]["text"]
                .as_str()
                .unwrap()
                .contains("createSurface")
        );
        harness.finish().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancel_notification_ends_the_turn_with_the_cancelled_stop_reason() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_delay(std::time::Duration::from_millis(500))
                    .set_body_string(sse(&[r#"{"choices":[{"delta":{"content":"late"}}]}"#])),
            )
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let mut harness = Harness::start(model_cfg(server.uri(), true), None);
        let sid = harness.new_session("cancel-me", dir.path()).await;

        harness
            .send(json!({
                "jsonrpc": "2.0", "id": 50, "method": "session/prompt",
                "params": { "sessionId": sid, "prompt": [{"type": "text", "text": "work slowly"}] },
            }))
            .await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        // Notification: no id, so nothing may come back for the cancel itself.
        harness
            .send(json!({
                "jsonrpc": "2.0", "method": "session/cancel",
                "params": { "sessionId": sid },
            }))
            .await;

        let (response, _) = harness.until_response(json!(50)).await;
        assert_eq!(response["result"]["stopReason"], "cancelled");
        harness.finish().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelling_a_permission_wait_unblocks_the_turn() {
        let model = mock_model(ToolResponder).await;
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = model_cfg(model.uri(), false);
        cfg.permission_mode = crate::config::PermissionMode::Manual;
        let mut harness = Harness::start(cfg, Some(dir.path().to_path_buf()));
        let sid = harness.new_session("cancel-hitl", dir.path()).await;

        harness
            .send(json!({
                "jsonrpc": "2.0", "id": 70, "method": "session/prompt",
                "params": { "sessionId": sid, "prompt": [{"type": "text", "text": "write test.txt"}] },
            }))
            .await;
        loop {
            if harness.next().await["method"] == "session/request_permission" {
                break;
            }
        }

        // No answer to the permission request — just cancel. The turn must end
        // rather than leave a blocked waiter behind.
        harness
            .send(json!({
                "jsonrpc": "2.0", "method": "session/cancel",
                "params": { "sessionId": sid },
            }))
            .await;

        let (response, _) = harness.until_response(json!(70)).await;
        assert_eq!(response["result"]["stopReason"], "cancelled");
        assert!(
            !dir.path().join("test.txt").exists(),
            "a cancelled permission must not grant the write"
        );
        harness.finish().await;
    }
}
