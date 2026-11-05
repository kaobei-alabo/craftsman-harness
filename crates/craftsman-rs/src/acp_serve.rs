//! `craftsman acp-serve` — Agent Client Protocol (ACP) agent over stdio.
//!
//! Newline-delimited JSON-RPC 2.0 on stdin/stdout, implementing the ACP agent
//! side as specified at <https://agentclientprotocol.com>:
//!
//! | Method | Direction | Notes |
//! |---|---|---|
//! | `initialize` | client → agent | negotiates `protocolVersion` + `agentCapabilities` |
//! | `session/new` | client → agent | requires `cwd`; returns the agent-chosen `sessionId` |
//! | `session/load` | client → agent | replays the conversation, then returns `null` |
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
    self, ApprovalPrompt, ApprovalPromptResult, AskUserPrompt, Session, SessionExtras, TokenSink,
    ToolActivityEvent, ToolActivitySink,
};
use crate::checkpoints::LibsqlCheckpointer;
use crate::config::Config;
use crate::hitl::ApprovalState;
use crate::serve::history_values;
use serde_json::{Value, json};
use std::collections::HashMap;
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

/// The prompt turn currently running, kept so `session/cancel` can end it.
struct InFlightPrompt {
    session_id: String,
    prompt_id: Value,
    abort_handle: tokio::task::AbortHandle,
}

/// Everything the stdio loop needs between messages.
pub struct AcpServerState {
    pub cfg: Config,
    pub approval: Arc<Mutex<ApprovalState>>,
    pub outbound_tx: OutboundTx,
    /// Server-initiated requests waiting for a client response, by request id.
    pub pending_permissions:
        Arc<Mutex<HashMap<String, std::sync::mpsc::Sender<ApprovalPromptResult>>>>,
    pub pending_questions: Arc<Mutex<HashMap<String, std::sync::mpsc::Sender<Option<String>>>>>,
    pub current_session_id: Arc<Mutex<String>>,
    pub frame_slot: FrameSlot,
    pub session: Option<Session>,
    pub workspace_root: Option<PathBuf>,
    in_flight_prompt: Arc<Mutex<Option<InFlightPrompt>>>,
    req_counter: Arc<AtomicU64>,
}

impl AcpServerState {
    pub fn new(
        cfg: Config,
        outbound_tx: OutboundTx,
        workspace_root: Option<PathBuf>,
    ) -> AcpServerState {
        let approval = Arc::new(Mutex::new(
            ApprovalState {
                mode: cfg.permission_mode,
                auto_approve: cfg.auto_approve,
                auto_approve_network: cfg.auto_approve_web(),
                shell_allow_list: cfg.shell_allow_list.clone(),
            }
            .with_env_allow_list(),
        ));

        AcpServerState {
            cfg,
            approval,
            outbound_tx,
            pending_permissions: Arc::new(Mutex::new(HashMap::new())),
            pending_questions: Arc::new(Mutex::new(HashMap::new())),
            current_session_id: Arc::new(Mutex::new(String::new())),
            frame_slot: Arc::new(Mutex::new(None)),
            session: None,
            workspace_root,
            in_flight_prompt: Arc::new(Mutex::new(None)),
            req_counter: Arc::new(AtomicU64::new(1)),
        }
    }

    /// Build the agent session, or switch it to `session_id`'s thread.
    ///
    /// # Errors
    /// Returns `Err` when no session can be built (typically no model
    /// configured) or the requested thread cannot be loaded.
    pub async fn ensure_session(&mut self, session_id: Option<String>) -> Result<(), String> {
        if self.session.is_none() {
            let built = build_acp_session(self, session_id.clone()).await;
            let Some(session) = built else {
                return Err("no agent session (is a model configured?)".into());
            };
            *self.current_session_id.lock().unwrap() = session.thread_id.clone();
            self.session = Some(session);
            return Ok(());
        }
        let Some(target) = session_id else {
            return Ok(());
        };
        let Some(session) = self.session.as_mut() else {
            return Ok(());
        };
        if session.thread_id == target {
            return Ok(());
        }
        match session.load_thread(&target).await {
            Ok(true) => {}
            // Unknown thread: adopt the id and start it empty, which is what a
            // client that generated its own conversation id expects.
            Ok(false) => {
                session.set_thread_id(target.clone());
                *session.agent.state.lock().unwrap() = Default::default();
            }
            Err(e) => return Err(format!("could not load session {target}: {e}")),
        }
        *self.current_session_id.lock().unwrap() = target;
        Ok(())
    }

    /// Messages of the live thread, in ACP replay order.
    fn live_history(&self) -> Vec<Value> {
        let Some(session) = self.session.as_ref() else {
            return Vec::new();
        };
        let messages = session.agent.state.lock().unwrap().messages.clone();
        history_values(&messages)
    }

    /// End the running turn: release every per-turn resource, unblock anything
    /// waiting on the client, then abort the task and answer its prompt.
    ///
    /// The order matters. A turn blocked inside an approval or ask-user wait is
    /// parked on a blocking thread, where `abort()` has no effect until the next
    /// await point — so the waiters are resolved *first*.
    pub fn cancel_in_flight(&self) -> bool {
        let in_flight = self.in_flight_prompt.lock().unwrap().take();

        // Stop the A2UI forwarder by dropping the frame sender.
        *self.frame_slot.lock().unwrap() = None;

        let permissions: Vec<_> = self
            .pending_permissions
            .lock()
            .unwrap()
            .drain()
            .map(|(_, waiter)| waiter)
            .collect();
        for waiter in permissions {
            let _ = waiter.send(ApprovalPromptResult::Reject("turn cancelled".into()));
        }
        let questions: Vec<_> = self
            .pending_questions
            .lock()
            .unwrap()
            .drain()
            .map(|(_, waiter)| waiter)
            .collect();
        for waiter in questions {
            let _ = waiter.send(None);
        }

        let Some(prompt) = in_flight else {
            return false;
        };
        prompt.abort_handle.abort();
        respond(
            &self.outbound_tx,
            prompt.prompt_id,
            json!({ "stopReason": "cancelled" }),
        );
        true
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
async fn build_acp_session(state: &AcpServerState, session_id: Option<String>) -> Option<Session> {
    let messages = Arc::new(AtomicU64::new(1));

    let tx = state.outbound_tx.clone();
    let sid = state.current_session_id.clone();
    let message_ids = messages.clone();
    let token_sink: TokenSink = Arc::new(move |token: &str| {
        let session_id = sid.lock().unwrap().clone();
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
    let sid = state.current_session_id.clone();
    let tool_activity_sink: ToolActivitySink = Arc::new(move |event: ToolActivityEvent| {
        let session_id = sid.lock().unwrap().clone();
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

    let tx = state.outbound_tx.clone();
    let sid = state.current_session_id.clone();
    let pending = state.pending_permissions.clone();
    let counter = state.req_counter.clone();
    let approval_prompt: ApprovalPrompt = Arc::new(
        move |tool: &str, summary: &str, details: &str| {
            let req_id = format!("perm-{}", counter.fetch_add(1, Ordering::Relaxed));
            let session_id = sid.lock().unwrap().clone();
            let (waiter_tx, waiter_rx) = std::sync::mpsc::channel();
            pending.lock().unwrap().insert(req_id.clone(), waiter_tx);

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
                        "toolCallId": req_id,
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
            // Blocking wait: the client's response, or `cancel_in_flight`
            // draining the map, wakes this up.
            tokio::task::block_in_place(|| waiter_rx.recv())
                .unwrap_or_else(|_| ApprovalPromptResult::Reject("turn cancelled".into()))
        },
    );

    let tx = state.outbound_tx.clone();
    let sid = state.current_session_id.clone();
    let pending = state.pending_questions.clone();
    let counter = state.req_counter.clone();
    let ask_user_prompt: AskUserPrompt = Arc::new(move |question: &str| {
        let req_id = format!("ask-{}", counter.fetch_add(1, Ordering::Relaxed));
        let session_id = sid.lock().unwrap().clone();
        let (waiter_tx, waiter_rx) = std::sync::mpsc::channel();
        pending.lock().unwrap().insert(req_id.clone(), waiter_tx);

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
        tokio::task::block_in_place(|| waiter_rx.recv())
            .ok()
            .flatten()
    });

    let extras = SessionExtras {
        token_sink: Some(token_sink),
        tool_activity_sink: Some(tool_activity_sink),
        approval_prompt: Some(approval_prompt),
        ask_user_prompt: Some(ask_user_prompt),
        extra_tools: vec![Arc::new(RenderUiTool::new(state.frame_slot.clone()))],
        workspace_root: state.workspace_root.clone(),
        ..SessionExtras::default()
    };

    agent::build_session_ext(
        &state.cfg,
        state.approval.clone(),
        SYSTEM_PROMPT_SUFFIX,
        // Activity goes to stderr: stdout carries JSON-RPC only.
        true,
        false,
        session_id,
        extras,
    )
    .await
}

// ── Parameter / response parsing ────────────────────────────────────────────

/// Map an ACP `RequestPermissionOutcome` to the agent's approval result.
///
/// Spec shape first (`result.outcome`), then tolerated legacy shapes so a
/// hand-written client or an older Craftsman UI still works.
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

    // Tolerated non-spec shapes.
    if let Some(approved) = result.as_bool() {
        return if approved {
            ApprovalPromptResult::Approve
        } else {
            ApprovalPromptResult::Reject(String::new())
        };
    }
    if let Some(option) = result.as_str() {
        return decision_for_option(option);
    }
    if let Some(option) = result.get("optionId").and_then(Value::as_str) {
        return decision_for_option(option);
    }
    if let Some(decision) = result.get("decision").and_then(Value::as_str) {
        let reason = result
            .get("feedback")
            .or_else(|| result.get("reason"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        return match decision_for_option(decision) {
            ApprovalPromptResult::Reject(_) => ApprovalPromptResult::Reject(reason.to_string()),
            other => other,
        };
    }
    ApprovalPromptResult::Reject(String::new())
}

/// `allow`/`approve` grants, `always` makes it sticky, anything else rejects.
fn decision_for_option(option: &str) -> ApprovalPromptResult {
    let option = option.to_ascii_lowercase();
    let granted = option.starts_with("allow") || option.starts_with("approve");
    if granted && option.contains("always") {
        ApprovalPromptResult::ApproveAlways
    } else if granted {
        ApprovalPromptResult::Approve
    } else {
        ApprovalPromptResult::Reject(String::new())
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
        .map(str::to_owned)
        .ok_or_else(|| "missing 'sessionId'".into())
}

/// `_meta.craftsman.threadId`, the client's own conversation id when it has one.
fn requested_thread_id(params: &Value) -> Option<String> {
    params
        .get("_meta")
        .and_then(|meta| meta.get("craftsman"))
        .and_then(|craftsman| craftsman.get("threadId"))
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
}

/// The version to run at: the client's, capped at what this agent implements.
fn negotiate_protocol_version(params: &Value) -> u64 {
    let requested = params
        .get("protocolVersion")
        .and_then(Value::as_u64)
        .unwrap_or(u64::from(ACP_PROTOCOL_VERSION));
    requested.min(u64::from(ACP_PROTOCOL_VERSION))
}

// ── Message dispatch ────────────────────────────────────────────────────────

/// Handle one inbound JSON-RPC message.
#[allow(clippy::too_many_lines)] // one match arm per protocol method reads better than 12 indirections
pub async fn handle_message(state: &mut AcpServerState, msg: Value) {
    let id_opt = msg.get("id").cloned();
    let method_opt = msg.get("method").and_then(Value::as_str);

    // A frame with an id but no method is a response to something we sent.
    if method_opt.is_none() {
        if let Some(id) = &id_opt {
            resolve_pending_response(state, id, &msg);
        }
        // Anything else (unknown or expired id) is ignored, as JSON-RPC
        // requires — a response is never itself answered.
        return;
    }
    let Some(method) = method_opt else { return };
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    let tx = state.outbound_tx.clone();

    match method {
        "initialize" => {
            let Some(id) = id_opt else { return };
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

        // The client picks the conversation with `_meta.craftsman.threadId`; without
        // it the agent mints one. `cwd`/`mcpServers` are accepted and ignored:
        // the pod's working directory and MCP wiring are platform-owned.
        "session/new" => {
            let Some(id) = id_opt else { return };
            let session_id =
                requested_thread_id(&params).unwrap_or_else(LibsqlCheckpointer::new_thread_id);
            if let Err(e) = state.ensure_session(Some(session_id.clone())).await {
                respond_error(&tx, id, -32603, &e);
                return;
            }
            *state.current_session_id.lock().unwrap() = session_id.clone();
            respond(
                &tx,
                id,
                json!({
                    "sessionId": session_id,
                    "_meta": { "craftsman": { "threadId": session_id } },
                }),
            );
        }

        // Replay the conversation as `session/update` notifications, then
        // return `null` — the spec's resume contract.
        "session/load" => {
            let Some(id) = id_opt else { return };
            let session_id = match extract_session_id(&params) {
                Ok(session_id) => session_id,
                Err(e) => return respond_error(&tx, id, -32602, &e),
            };
            if let Err(e) = state.ensure_session(Some(session_id.clone())).await {
                respond_error(&tx, id, -32603, &e);
                return;
            }
            *state.current_session_id.lock().unwrap() = session_id.clone();

            for (index, message) in state.live_history().iter().enumerate() {
                let role = message.get("role").and_then(Value::as_str).unwrap_or("");
                let content = message.get("content").and_then(Value::as_str).unwrap_or("");
                let kind = match role {
                    "user" => "user_message_chunk",
                    "assistant" => "agent_message_chunk",
                    _ => continue,
                };
                session_update(
                    &tx,
                    &session_id,
                    json!({
                        "sessionUpdate": kind,
                        "messageId": format!("replay-{index}"),
                        "content": text_block(content),
                    }),
                );
            }
            respond(&tx, id, Value::Null);
        }

        "session/prompt" => {
            let Some(id) = id_opt else { return };
            let session_id = match extract_session_id(&params) {
                Ok(session_id) => session_id,
                Err(e) => return respond_error(&tx, id, -32602, &e),
            };
            let prompt_text = match extract_prompt_text(&params) {
                Ok(text) => text,
                Err(e) => return respond_error(&tx, id, -32602, &e),
            };
            if let Err(e) = state.ensure_session(Some(session_id.clone())).await {
                respond_error(&tx, id, -32603, &e);
                return;
            }
            // A second prompt supersedes the first; the client sees its
            // `cancelled` stop reason before this turn starts streaming.
            state.cancel_in_flight();
            *state.current_session_id.lock().unwrap() = session_id.clone();
            start_prompt_turn(state, id, session_id, prompt_text);
        }

        // Notification: no response, and the pending prompt answers `cancelled`.
        "session/cancel" => {
            let target = params
                .get("sessionId")
                .or_else(|| params.get("session_id"))
                .and_then(Value::as_str);
            let matches_target = target.is_none_or(|target| {
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
            let _ = state.ensure_session(None).await;
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
        let _ = waiter.send(parse_approval_result(msg));
        return;
    }
    if let Some(waiter) = state.pending_questions.lock().unwrap().remove(&key) {
        let _ = waiter.send(parse_question_result(msg));
    }
}

/// Spawn the turn task and record it as in-flight.
fn start_prompt_turn(
    state: &mut AcpServerState,
    prompt_id: Value,
    session_id: String,
    prompt_text: String,
) {
    let tx = state.outbound_tx.clone();

    // A2UI frames arrive on a channel while the turn runs; forward each as an
    // ACP resource content block.
    let (a2ui_tx, mut a2ui_rx) = tokio::sync::mpsc::unbounded_channel::<Value>();
    *state.frame_slot.lock().unwrap() = Some(a2ui_tx);
    let a2ui_task = {
        let tx = tx.clone();
        let session_id = session_id.clone();
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
                    &session_id,
                    json!({
                        "sessionUpdate": "agent_message_chunk",
                        "content": {
                            "type": "resource",
                            "resource": {
                                "uri": "craftsman:a2ui",
                                "mimeType": A2UI_MIME_TYPE,
                                "text": payload,
                            },
                        },
                        "_meta": { "craftsman": { "a2ui": true } },
                    }),
                );
            }
        })
    };

    let Some(session) = state.session.as_ref() else {
        respond_error(&tx, prompt_id, -32603, "no agent session");
        return;
    };
    let agent = session.agent.clone();
    let usage = session.usage.clone();
    let usage_before = *usage.lock().unwrap();
    let frame_slot = state.frame_slot.clone();
    let in_flight = state.in_flight_prompt.clone();
    let turn_prompt_id = prompt_id.clone();

    let turn = tokio::spawn(async move {
        let outcome = agent.run_turn(&prompt_text).await;

        // Releasing the slot ends the forwarder, so the last A2UI frame is
        // always flushed before the prompt response.
        *frame_slot.lock().unwrap() = None;
        let _ = a2ui_task.await;
        let _ = in_flight.lock().unwrap().take();

        let usage_after = *usage.lock().unwrap();
        let input = usage_after.0.saturating_sub(usage_before.0);
        let output = usage_after.1.saturating_sub(usage_before.1);
        let craftsman_meta = json!({
            "usage": {
                "inputTokens": input,
                "outputTokens": output,
            }
        });
        // Per-model pricing metadata is no longer fetched (see
        // docs/platform-token-refactor.md); costUsd is simply omitted.

        // ACP carries no usage field on the prompt result, so it rides `_meta`.
        let stop_reason = match outcome {
            Ok(
                craftsman_harness::TurnResult::Done(_)
                | craftsman_harness::TurnResult::Suspended(_),
            ) => "end_turn",
            Err(craftsman_harness::AgentError::MaxRounds(_)) => {
                agent::rollback_dangling_user(&agent);
                "max_turn_requests"
            }
            Err(e) => {
                agent::rollback_dangling_user(&agent);
                respond_error(&tx, turn_prompt_id, -32603, &format!("agent error: {e}"));
                return;
            }
        };
        respond(
            &tx,
            turn_prompt_id,
            json!({ "stopReason": stop_reason, "_meta": { "craftsman": craftsman_meta } }),
        );
    });

    *state.in_flight_prompt.lock().unwrap() = Some(InFlightPrompt {
        session_id,
        prompt_id,
        abort_handle: turn.abort_handle(),
    });
}

/// Switch the configured model, rebuilding the session on the same thread.
/// Rolls back to the previous model when the rebuild fails.
async fn set_model(state: &mut AcpServerState, model: String) -> Result<(), String> {
    let previous = state.cfg.model.clone();
    if previous.as_deref() == Some(model.as_str()) {
        return Ok(());
    }
    let thread = state.session.as_ref().map(|s| s.thread_id.clone());

    state.cfg.model = Some(model);
    let _ = state.cfg.save();
    if let Some(rebuilt) = build_acp_session(state, thread.clone()).await {
        state.session = Some(rebuilt);
        return Ok(());
    }

    state.cfg.model = previous;
    let _ = state.cfg.save();
    state.session = build_acp_session(state, thread).await;
    Err("agent rebuild failed for that model".into())
}

/// Conversations this harness has persisted locally.
async fn list_sessions(state: &AcpServerState) -> Vec<Value> {
    use craftsman_harness::Checkpointer;
    let Some(checkpointer) = state.session.as_ref().and_then(|s| s.checkpointer.as_ref()) else {
        return Vec::new();
    };
    checkpointer
        .list_threads()
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|thread| {
            json!({
                "sessionId": thread.id,
                "title": thread.title,
                "createdAt": thread.created,
                "updatedAt": thread.updated,
            })
        })
        .collect()
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

    state.cancel_in_flight();
    drop(state);
    drop(outbound_tx);
    let _ = writer_task.await;
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
    fn permission_outcomes_follow_the_spec_then_tolerate_legacy_shapes() {
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

        // Tolerated shapes.
        assert_eq!(
            parse_approval_result(&json!({"result": true})),
            ApprovalPromptResult::Approve
        );
        assert_eq!(
            parse_approval_result(&json!({"result": "approve_always"})),
            ApprovalPromptResult::ApproveAlways
        );
        assert_eq!(
            parse_approval_result(&json!({"result": {"decision": "reject", "feedback": "unsafe"}})),
            ApprovalPromptResult::Reject("unsafe".into())
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
        let mut harness = Harness::start(model_cfg(model.uri(), true), None);

        harness
            .send(json!({
                "jsonrpc": "2.0", "id": 1, "method": "session/new",
                "params": {
                    "cwd": "/home/worker",
                    "mcpServers": [],
                    "_meta": { "craftsman": { "threadId": "thread-42" } },
                },
            }))
            .await;
        let created = harness.next().await;
        assert_eq!(
            created["result"]["sessionId"], "thread-42",
            "the client's conversation id must be adopted, not discarded"
        );

        harness
            .send(json!({
                "jsonrpc": "2.0", "id": 2, "method": "session/prompt",
                "params": {
                    "sessionId": "thread-42",
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
                .all(|msg| msg["params"]["sessionId"] == "thread-42")
        );
        harness.finish().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn session_load_replays_the_conversation_then_returns_null() {
        let model = mock_model(TextResponder).await;
        let mut harness = Harness::start(model_cfg(model.uri(), true), None);

        harness
            .send(json!({
                "jsonrpc": "2.0", "id": 1, "method": "session/new",
                "params": { "cwd": "/home/worker", "mcpServers": [],
                            "_meta": { "craftsman": { "threadId": "thread-load" } } },
            }))
            .await;
        harness.next().await;
        harness
            .send(json!({
                "jsonrpc": "2.0", "id": 2, "method": "session/prompt",
                "params": { "sessionId": "thread-load", "prompt": [{"type": "text", "text": "remember me"}] },
            }))
            .await;
        harness.until_response(json!(2)).await;

        harness
            .send(json!({
                "jsonrpc": "2.0", "id": 3, "method": "session/load",
                "params": { "sessionId": "thread-load", "cwd": "/home/worker", "mcpServers": [] },
            }))
            .await;
        let (response, replay) = harness.until_response(json!(3)).await;

        assert!(response["result"].is_null(), "session/load returns null");
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

        harness
            .send(json!({
                "jsonrpc": "2.0", "id": 1, "method": "session/prompt",
                "params": { "sessionId": "tool-session", "prompt": [{"type": "text", "text": "write test.txt"}] },
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

        harness
            .send(json!({
                "jsonrpc": "2.0", "id": 100, "method": "session/prompt",
                "params": { "sessionId": "hitl", "prompt": [{"type": "text", "text": "write test.txt"}] },
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
        let mut harness = Harness::start(model_cfg(model.uri(), true), None);

        harness
            .send(json!({
                "jsonrpc": "2.0", "id": 200, "method": "session/prompt",
                "params": { "sessionId": "a2ui", "prompt": [{"type": "text", "text": "show ui"}] },
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
        let mut harness = Harness::start(model_cfg(server.uri(), true), None);

        harness
            .send(json!({
                "jsonrpc": "2.0", "id": 50, "method": "session/prompt",
                "params": { "sessionId": "cancel-me", "prompt": [{"type": "text", "text": "work slowly"}] },
            }))
            .await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        // Notification: no id, so nothing may come back for the cancel itself.
        harness
            .send(json!({
                "jsonrpc": "2.0", "method": "session/cancel",
                "params": { "sessionId": "cancel-me" },
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

        harness
            .send(json!({
                "jsonrpc": "2.0", "id": 70, "method": "session/prompt",
                "params": { "sessionId": "cancel-hitl", "prompt": [{"type": "text", "text": "write test.txt"}] },
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
                "params": { "sessionId": "cancel-hitl" },
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
