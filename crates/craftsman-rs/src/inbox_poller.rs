//! Workspace inbox poller — the pod-side "craftsman worker" the workspace
//! addon's inbox expects.
//!
//! The platform's scheduler (and the offline-message path) delivers prompts
//! into a durable inbox (`workspace_message`, status=queued) owned by the
//! workspace addon. The pod claims them via
//! `GET {platform}/apps/workspace/api/workspace/inbox` (claiming is ATOMIC
//! server-side: returned messages are marked delivered and can never be
//! returned again), runs each as a normal agent turn, and reports the
//! outcome via `POST .../inbox/{id}/result`.
//!
//! At-most-once by design: a claimed message is OWNED by this process. If
//! the turn fails we post the error — we never re-run, because a re-poll
//! cannot see the message again anyway.
//!
//! Polling deliberately does not count as user activity (the addon's
//! `poll_inbox` skips `touch_active`, keeping the idle reaper honest);
//! `post_result` bumps activity server-side, so no extra heartbeats here.
//!
//! # Where this runs
//!
//! The workspace pod's entrypoint is the ACP bridge, not `craftsman serve`:
//!
//! ```text
//! craftsman-rs acp-bridge --host 0.0.0.0 --port 8080 \
//!     -- craftsman-rs acp-serve --permission-mode bypass-permissions
//! ```
//!
//! so the poller is spawned from [`crate::acp_bridge::run`] — the only
//! long-lived process in the pod — and drives each turn by spawning its OWN
//! harness child and speaking ACP to it over stdio, exactly as the bridge
//! does for a browser WebSocket.
//!
//! ## Concurrency
//!
//! Because a poller turn spawns its own harness child, a poller turn and a
//! user's WebSocket turn CAN now run at the same time (the old design
//! serialized both behind `AgentRunner`'s mutex). Each child is an
//! independent process with independent in-memory state; the shared state is
//! the on-disk checkpointer / thread store, and distinct thread ids (see
//! [`thread_for`]) keep the two conversations from colliding. The harness and
//! the pod must tolerate that overlap.

use crate::acp_bridge::Harness;
use crate::config::Config;
use serde_json::Value;
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

/// Default seconds between polls; override with `CRAFTSMAN_INBOX_POLL_SECS`.
const DEFAULT_POLL_SECS: u64 = 5;
/// Per-request HTTP timeout.
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
/// A repeated identical warning is re-logged every Nth occurrence.
const WARN_EVERY_N: u32 = 60;
/// Wall-clock cap on one inbox message's ACP turn (spawn → `stopReason`).
/// A harness that wedges must not wedge the poller with it: on timeout the
/// child is killed and the message is settled with an error.
const TURN_TIMEOUT: Duration = Duration::from_secs(600);

/// One claimed inbox message.
#[derive(Debug, Clone, PartialEq)]
pub struct InboxMessage {
    pub id: String,
    pub thread_id: Option<String>,
    pub content: String,
}

/// Live platform credentials, re-read for EVERY HTTP call.
///
/// The personal API token is static (rotation is handled on the platform
/// side), but the poller still holds no `Config` of its own: credentials
/// mean "whatever is on disk right now" — see [`ConfigAuth`]. Re-reading
/// before each claim and each settle is what makes a token swapped by a
/// *different* process visible here.
pub trait AuthSource: Send + Sync {
    /// Current (platform.base_url, platform API token).
    fn current_auth(&self) -> (String, String);
}

/// The real source: `~/.craftsman/config.json`, re-read on every call.
///
/// Load failures (missing file, malformed JSON) yield empty credentials,
/// which the caller treats as "not ready" rather than as a hard error — the
/// init container may not have seeded the file yet.
pub struct ConfigAuth;

impl AuthSource for ConfigAuth {
    fn current_auth(&self) -> (String, String) {
        match Config::load() {
            Ok(Some(cfg)) => {
                let token = cfg.platform_api_token();
                (cfg.platform.base_url.clone(), token)
            }
            Ok(None) => (String::new(), String::new()),
            Err(e) => {
                eprintln!("inbox poller: could not read config.json: {e}");
                (String::new(), String::new())
            }
        }
    }
}

/// Poll interval from the environment: `CRAFTSMAN_INBOX_POLL_SECS`.
pub fn poll_interval() -> Duration {
    interval_from(std::env::var("CRAFTSMAN_INBOX_POLL_SECS").ok().as_deref())
}

/// Interval from a raw env value: default 5s, clamped to >= 1s (a zero or
/// garbage value must not busy-loop the pod).
fn interval_from(raw: Option<&str>) -> Duration {
    let secs = raw
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_POLL_SECS)
        .max(1);
    Duration::from_secs(secs)
}

/// Thread for a message: its own `thread_id` when the scheduler pinned one,
/// else a deterministic per-message thread (`inbox-<id>`) so a retry of the
/// same message id — which cannot happen post-claim, but costs nothing to
/// keep deterministic — would land in the same conversation. The harness
/// treats an unknown `_meta.craftsman.threadId` as a fresh thread, so arbitrary
/// strings are fine.
pub fn thread_for(message: &InboxMessage) -> String {
    match message.thread_id.as_deref().filter(|t| !t.is_empty()) {
        Some(thread) => thread.to_string(),
        None => format!("inbox-{}", message.id),
    }
}

/// Parse a poll response body (the 200 case) into messages.
/// `{"messages":[{"id":"...","thread_id":...|null,"content":"..."}]}` —
/// the workspace addon's `claim_queued` row shape.
///
/// An entry without an id is a contract break: it was claimed server-side
/// but can never be settled (no id to post a result for). It is dropped
/// from the batch but logged loudly so the row stuck in `delivered` is
/// traceable to this pod. Likewise, an unparseable 200 body means the whole
/// batch was claimed and lost — the `Err` surfaces that in the poll log.
pub fn parse_messages(body: &[u8]) -> Result<Vec<InboxMessage>, String> {
    let value: Value =
        serde_json::from_slice(body).map_err(|e| format!("inbox response is not JSON: {e}"))?;
    let Some(items) = value["messages"].as_array() else {
        return Err("inbox response missing \"messages\" array".into());
    };
    Ok(items
        .iter()
        .filter_map(|item| {
            let Some(id) = item["id"].as_str().filter(|id| !id.is_empty()) else {
                eprintln!(
                    "inbox poller: claimed message has no id and cannot be settled \
                     (contract break; entry: {item})"
                );
                return None;
            };
            Some(InboxMessage {
                id: id.to_string(),
                thread_id: item["thread_id"]
                    .as_str()
                    .filter(|t| !t.is_empty())
                    .map(String::from),
                content: item["content"].as_str().unwrap_or("").to_string(),
            })
        })
        .collect())
}

fn inbox_url(base_url: &str) -> String {
    format!(
        "{}/apps/workspace/api/workspace/inbox",
        base_url.trim_end_matches('/')
    )
}

fn result_url(base_url: &str, message_id: &str) -> String {
    format!("{}/{message_id}/result", inbox_url(base_url))
}

/// Claim queued messages. `Ok(vec![])` covers 204/empty; any transport or
/// non-2xx failure is an `Err` string for the throttled warn log. (A claim
/// 401 is harmless — nothing was claimed; the next tick reads a fresh
/// token.)
async fn claim(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
) -> Result<Vec<InboxMessage>, String> {
    let response = client
        .get(inbox_url(base_url))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| format!("inbox poll failed: {e}"))?;
    let status = response.status();
    if status == reqwest::StatusCode::NO_CONTENT {
        return Ok(Vec::new());
    }
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        // Nothing was claimed, so no message is at risk — but this pod cannot
        // fix itself either. The poller only READS config.json; token refresh
        // happens inside harness children, and no child runs until a claim
        // succeeds. So a pod whose access AND refresh tokens both expired
        // while idle will sit here forever. Say that plainly instead of
        // hiding it behind a generic "HTTP 401" that reads as transient.
        return Err(format!(
            "inbox poll: HTTP {status} — the access token was rejected and this pod \
             cannot self-recover (the poller never refreshes; only a running turn does). \
             If this repeats, the workspace needs re-authentication."
        ));
    }
    if !status.is_success() {
        return Err(format!("inbox poll: HTTP {status}"));
    }
    let body = response
        .bytes()
        .await
        .map_err(|e| format!("inbox poll body read failed: {e}"))?;
    if body.is_empty() {
        // Contract: emptiness is signalled via 204, handled above. A 200 with
        // an empty body means the server claimed messages we never received —
        // surface it as an error (the caller throttles the warning) instead of
        // silently treating it as an empty inbox.
        return Err(format!(
            "inbox poll: HTTP {status} with empty body (contract expects 204 for empty inbox)"
        ));
    }
    parse_messages(&body)
}

/// Report a turn outcome for a claimed message, reading FRESH credentials
/// first (see [`AuthSource`] — a harness child may have rotated the token on
/// disk during the turn). Best effort: the message is already owned, so a
/// failure here can only be logged.
async fn settle(
    client: &reqwest::Client,
    auth: &dyn AuthSource,
    message_id: &str,
    outcome: &Result<String, String>,
) -> Result<(), String> {
    let (base_url, token) = auth.current_auth();
    if base_url.is_empty() || token.is_empty() {
        return Err("post_result: platform credentials missing".into());
    }
    let body = match outcome {
        Ok(text) => serde_json::json!({ "result": text, "error": Value::Null }),
        Err(error) => serde_json::json!({ "result": Value::Null, "error": error }),
    };
    let response = client
        .post(result_url(&base_url, message_id))
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("post_result failed: {e}"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("post_result: HTTP {status}"));
    }
    Ok(())
}

/// JSON-RPC ids used for the three calls of one poller turn. Each turn gets a
/// fresh child, so fixed ids are unambiguous.
const ID_INITIALIZE: i64 = 1;
const ID_SESSION_NEW: i64 = 2;
const ID_PROMPT: i64 = 3;

/// Identity env for a poller-spawned harness child: the same as what
/// [`crate::acp_bridge::bridge_session`] forwards for a browser session.
///
/// The bridge reads these from the platform proxy's per-dial headers
/// (`x-craftsman-workspace-user` / `-tenant`); the poller has no dial, so it uses
/// the pod's own identity from `config.json` — which is the same user/tenant,
/// since a workspace pod belongs to exactly one of each. Attribution matters
/// Environment identity for a harness child: the platform's own env is
/// passed through by the deployment; nothing is derived from login-era
/// identity fields anymore (the token carries the identity server-side).
fn identity_env() -> HashMap<String, String> {
    HashMap::new()
}

/// Run one message as an ACP turn against a freshly spawned harness child.
///
/// The sequence is what a browser client does through the bridge:
/// `initialize` → `session/new` (pinning the thread via
/// `_meta.craftsman.threadId`) → `session/prompt`, collecting the
/// `agent_message_chunk` text along the way. The concatenated chunk text IS
/// the assistant's reply — ACP has no separate "final text" field. Non-text
/// chunk content (a2ui resources carry `content.type == "resource"`, no
/// `text`) is skipped by construction.
///
/// The whole thing is wrapped in [`TURN_TIMEOUT`]; the child is killed on
/// every exit path (`kill_on_drop` plus an explicit kill).
async fn run_message_turn(harness: &Harness, message: &InboxMessage) -> Result<String, String> {
    run_message_turn_within(harness, message, TURN_TIMEOUT).await
}

async fn run_message_turn_within(
    harness: &Harness,
    message: &InboxMessage,
    limit: Duration,
) -> Result<String, String> {
    match tokio::time::timeout(limit, acp_turn(harness, message)).await {
        Ok(result) => result,
        Err(_) => Err(format!(
            "inbox turn exceeded {}s and was aborted",
            limit.as_secs()
        )),
    }
}

async fn acp_turn(harness: &Harness, message: &InboxMessage) -> Result<String, String> {
    let mut cmd = Command::new(&harness.command);
    cmd.args(&harness.args)
        .envs(identity_env())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        // The harness logs to stderr; it belongs in the pod log.
        .stderr(Stdio::inherit())
        .kill_on_drop(true);
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("failed to spawn harness `{}`: {e}", harness.command))?;
    let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
        let _ = child.kill().await;
        return Err("harness stdio was not captured".into());
    };

    let mut conn = AcpConn {
        stdin,
        lines: BufReader::new(stdout).lines(),
    };
    let outcome = conn.drive(message).await;
    // Belt and braces: `kill_on_drop` covers a panic/cancel, this covers the
    // ordinary path so the child is reaped before the next message starts.
    let _ = child.kill().await;
    let _ = child.wait().await;
    outcome
}

/// One newline-delimited JSON-RPC conversation with a harness child.
struct AcpConn {
    stdin: tokio::process::ChildStdin,
    lines: tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
}

impl AcpConn {
    async fn drive(&mut self, message: &InboxMessage) -> Result<String, String> {
        self.send(serde_json::json!({
            "jsonrpc": "2.0",
            "id": ID_INITIALIZE,
            "method": "initialize",
            "params": {
                "protocolVersion": 1,
                "clientCapabilities": {
                    "fs": { "readTextFile": false, "writeTextFile": false }
                },
                "clientInfo": { "name": "craftsman-inbox-poller", "version": "1" },
            },
        }))
        .await?;
        let mut sink = String::new();
        self.await_response(ID_INITIALIZE, &mut sink).await?;

        self.send(serde_json::json!({
            "jsonrpc": "2.0",
            "id": ID_SESSION_NEW,
            "method": "session/new",
            "params": {
                "cwd": crate::serve::home_dir().to_string_lossy(),
                "mcpServers": [],
                "_meta": { "craftsman": { "threadId": thread_for(message) } },
            },
        }))
        .await?;
        let created = self.await_response(ID_SESSION_NEW, &mut sink).await?;
        let session_id = created["sessionId"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "session/new returned no sessionId".to_string())?
            .to_string();

        self.send(serde_json::json!({
            "jsonrpc": "2.0",
            "id": ID_PROMPT,
            "method": "session/prompt",
            "params": {
                "sessionId": session_id,
                "prompt": [{ "type": "text", "text": message.content }],
            },
        }))
        .await?;
        // Chunks stream as notifications while this waits for the response.
        let mut text = String::new();
        let result = self.await_response(ID_PROMPT, &mut text).await?;

        match result["stopReason"].as_str() {
            Some("end_turn") => Ok(text),
            Some(other) => Err(format!("turn ended with stopReason `{other}`: {text}")),
            None => Err("session/prompt returned no stopReason".into()),
        }
    }

    async fn send(&mut self, frame: Value) -> Result<(), String> {
        let mut line = serde_json::to_vec(&frame).map_err(|e| format!("encode failed: {e}"))?;
        line.push(b'\n');
        // A broken pipe here means the harness died; it is reported as the
        // turn's error rather than retried (the message is already claimed).
        self.stdin
            .write_all(&line)
            .await
            .map_err(|e| format!("harness stdin write failed: {e}"))?;
        self.stdin
            .flush()
            .await
            .map_err(|e| format!("harness stdin flush failed: {e}"))
    }

    /// Read lines until the response for `id` arrives, appending every
    /// `agent_message_chunk` text to `text`. A JSON-RPC `error` for `id`
    /// becomes an `Err`; EOF before the response means the harness died.
    async fn await_response(&mut self, id: i64, text: &mut String) -> Result<Value, String> {
        loop {
            let line = self
                .lines
                .next_line()
                .await
                .map_err(|e| format!("harness stdout read failed: {e}"))?;
            let Some(line) = line else {
                return Err(format!(
                    "harness exited before answering request {id} (ACP stream ended)"
                ));
            };
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Ok(frame) = serde_json::from_str::<Value>(line) else {
                // Not our protocol — a stray log line on stdout. Ignore it
                // rather than failing the turn.
                continue;
            };
            if frame["method"] == "session/update" {
                let update = &frame["params"]["update"];
                if update["sessionUpdate"] == "agent_message_chunk"
                    && let Some(chunk) = update["content"]["text"].as_str()
                {
                    text.push_str(chunk);
                }
                continue;
            }
            // A request FROM the agent. These MUST be answered: the harness
            // blocks a thread waiting for the reply (`block_in_place` on the
            // pending-request channel in acp_serve), so ignoring one wedges
            // the turn until TURN_TIMEOUT.
            //
            // INVARIANT (load-bearing): agent-initiated request ids are
            // STRINGS (`perm-{n}`, `ask-{n}` — acp_serve's `req_counter`),
            // while every id WE originate is an int (ID_INITIALIZE/…). That
            // is why the `as_i64()` comparison below cannot confuse the two,
            // and why answering here can never swallow our own response.
            if let Some(request_id) = agent_request_id(&frame) {
                self.answer_agent_request(request_id, &frame).await?;
                continue;
            }
            if frame["id"].as_i64() != Some(id) {
                // An unrelated / stale response. Not ours to act on.
                continue;
            }
            if let Some(error) = frame.get("error").filter(|e| !e.is_null()) {
                let detail = error["message"].as_str().unwrap_or("unknown error");
                return Err(format!(
                    "harness returned an error for request {id}: {detail}"
                ));
            }
            return Ok(frame.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    /// Reply to an agent-initiated request so its blocked thread resumes.
    ///
    /// The poller is a headless client: there is no human to ask and no human
    /// to approve. Both answers are therefore "no", expressed in whichever
    /// shape the corresponding parser in `acp_serve` understands:
    ///
    /// * `_craftsman/session/request_input` (ask_user) — a JSON-RPC **error**.
    ///   `parse_question_result` maps both a missing answer and an error to
    ///   `None`, and an error is the documented reply for a client that does
    ///   not implement this Craftsman extension (acp_serve.rs's own comment says
    ///   so), which is exactly what the poller is. `AskUserTool` then returns
    ///   "The user did not provide an answer. Proceed with your best
    ///   assumption." and the turn continues instead of hanging.
    ///
    /// * `session/request_permission` — a `reject-once` outcome. The pod runs
    ///   `--permission-mode bypass-permissions` so this should never fire,
    ///   but a harness configured otherwise must not wedge the poller either;
    ///   rejecting is the safe default for an unattended turn (it fails the
    ///   tool, not the process).
    async fn answer_agent_request(&mut self, id: &str, frame: &Value) -> Result<(), String> {
        let method = frame["method"].as_str().unwrap_or_default();
        let reply = match method {
            ASK_USER_METHOD => serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {
                    "code": -32601,
                    "message": "craftsman-inbox-poller runs unattended; no user is available to ask",
                },
            }),
            _ => serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "outcome": { "outcome": "selected", "optionId": OPTION_REJECT_ONCE },
                },
            }),
        };
        self.send(reply).await
    }
}

/// The Craftsman ask-user extension method (`acp_serve`'s `ask_user_prompt`).
const ASK_USER_METHOD: &str = "_craftsman/session/request_input";
/// `acp_serve`'s reject option id, as offered in `session/request_permission`.
const OPTION_REJECT_ONCE: &str = "reject-once";

/// The id of an agent-initiated REQUEST, i.e. a frame carrying both a
/// `method` and an `id`. Responses have an `id` but no `method`;
/// notifications have a `method` but no `id`.
fn agent_request_id(frame: &Value) -> Option<&str> {
    if !frame.get("method").is_some_and(Value::is_string) {
        return None;
    }
    frame.get("id").and_then(Value::as_str)
}

/// One full poll cycle: claim, then run + report each message sequentially
/// (one harness child at a time keeps the pod's memory bounded). Returns the
/// number of messages processed; `Err` is a claim failure only — per-message
/// failures are posted back as results and never abort the batch.
pub async fn poll_once(
    client: &reqwest::Client,
    auth: &dyn AuthSource,
    harness: &Harness,
) -> Result<usize, String> {
    let (base_url, token) = auth.current_auth();
    if base_url.is_empty() || token.is_empty() {
        return Err("platform credentials missing; skipping poll".into());
    }
    let messages = claim(client, &base_url, &token).await?;
    let count = messages.len();
    for message in &messages {
        let outcome = if message.content.is_empty() {
            // Nothing to run; still settle the claim so the row terminates.
            Err("inbox message has no content".to_string())
        } else {
            run_message_turn(harness, message).await
        };
        if let Err(error) = &outcome {
            eprintln!(
                "inbox poller: turn failed for message {}: {error}",
                message.id
            );
        }
        // The message is claimed either way — post the outcome (with fresh
        // credentials), never rerun. A settle failure can only be logged;
        // the rest of the batch still gets its results.
        if let Err(e) = settle(client, auth, &message.id, &outcome).await {
            eprintln!("inbox poller: {} (message {})", e, message.id);
        }
    }
    Ok(count)
}

/// De-duplicates consecutive identical warnings so a dead platform does not
/// write the same line every poll tick.
struct ThrottledWarn {
    last: Option<String>,
    repeats: u32,
}

impl ThrottledWarn {
    fn new() -> Self {
        ThrottledWarn {
            last: None,
            repeats: 0,
        }
    }

    fn warn(&mut self, message: &str) {
        if self.last.as_deref() == Some(message) {
            self.repeats += 1;
            if self.repeats % WARN_EVERY_N == 0 {
                eprintln!("inbox poller: {message} (repeated {} times)", self.repeats);
            }
            return;
        }
        self.last = Some(message.to_string());
        self.repeats = 1;
        eprintln!("inbox poller: {message}");
    }

    fn clear(&mut self) {
        self.last = None;
        self.repeats = 0;
    }
}

/// Spawn the poller alongside the ACP bridge. The task never panics its way
/// out: every failure mode is a logged warn + the next tick.
///
/// The ready gate is now purely credential-based. There is no `TurnDriver`
/// to ask "is the agent up?" — the harness is a child process that does not
/// exist until a message needs it, and a spawn/handshake failure is reported
/// per message. What we CAN check cheaply, and must, is that the pod has a
/// platform to talk to at all: with no `platform_base_url`/`auth_token` on
/// disk we skip the claim entirely, leaving messages queued server-side
/// (exactly right — they run once credentials land).
///
/// The returned handle keeps the task's ownership explicit; the bridge runs
/// until the process dies, so nothing joins it. If it ever grows graceful
/// shutdown, abort this handle there.
pub fn spawn(harness: Arc<Harness>) -> tokio::task::JoinHandle<()> {
    spawn_with(harness, Arc::new(ConfigAuth))
}

fn spawn_with(harness: Arc<Harness>, auth: Arc<dyn AuthSource>) -> tokio::task::JoinHandle<()> {
    let interval = poll_interval();
    eprintln!("inbox poller: polling every {}s", interval.as_secs());
    tokio::spawn(async move {
        let client = match reqwest::Client::builder().timeout(HTTP_TIMEOUT).build() {
            Ok(client) => client,
            Err(error) => {
                // Fall back to the default client rather than killing the
                // poller, but say so: a silently dropped timeout is exactly
                // the kind of thing that makes hangs undiagnosable.
                eprintln!(
                    "inbox poller: reqwest client build failed ({error}); \
                     falling back to default client WITHOUT the {}s timeout",
                    HTTP_TIMEOUT.as_secs()
                );
                reqwest::Client::default()
            }
        };
        let mut throttle = ThrottledWarn::new();
        loop {
            // Sleep FIRST: the initial delay is deliberate — it gives the
            // init container's config write a beat to land before the first
            // claim, and keeps the loop shape uniform.
            tokio::time::sleep(interval).await;
            match poll_once(&client, auth.as_ref(), &harness).await {
                Ok(_) => throttle.clear(),
                Err(message) => throttle.warn(&message),
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::sync::Mutex;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn interval_defaults_and_clamps() {
        assert_eq!(interval_from(None), Duration::from_secs(DEFAULT_POLL_SECS));
        assert_eq!(
            interval_from(Some("garbage")),
            Duration::from_secs(DEFAULT_POLL_SECS)
        );
        assert_eq!(interval_from(Some("0")), Duration::from_secs(1));
        assert_eq!(interval_from(Some(" 7 ")), Duration::from_secs(7));
        assert_eq!(
            interval_from(Some("-3")),
            Duration::from_secs(DEFAULT_POLL_SECS)
        );
    }

    #[test]
    fn thread_id_prefers_message_thread_then_derives() {
        let with_thread = InboxMessage {
            id: "m1".into(),
            thread_id: Some("t-42".into()),
            content: "hi".into(),
        };
        assert_eq!(thread_for(&with_thread), "t-42");
        let without = InboxMessage {
            id: "m2".into(),
            thread_id: None,
            content: "hi".into(),
        };
        assert_eq!(thread_for(&without), "inbox-m2");
        let empty = InboxMessage {
            id: "m3".into(),
            thread_id: Some(String::new()),
            content: "hi".into(),
        };
        assert_eq!(thread_for(&empty), "inbox-m3");
    }

    #[test]
    fn parses_messages_and_rejects_malformed() {
        let body = json!({"messages": [
            {"id": "m1", "thread_id": "t-1", "content": "do the thing"},
            {"id": "m2", "thread_id": null, "content": "another"},
            {"id": "", "content": "dropped: no id"},
        ]})
        .to_string();
        let messages = parse_messages(body.as_bytes()).unwrap();
        assert_eq!(
            messages,
            vec![
                InboxMessage {
                    id: "m1".into(),
                    thread_id: Some("t-1".into()),
                    content: "do the thing".into(),
                },
                InboxMessage {
                    id: "m2".into(),
                    thread_id: None,
                    content: "another".into(),
                },
            ]
        );
        assert!(parse_messages(b"not json").is_err());
        assert!(parse_messages(b"{\"nope\":[]}").is_err());
        assert_eq!(parse_messages(b"{\"messages\":[]}").unwrap(), vec![]);
    }

    /// Fixed-credentials auth source, plus a rotating variant for the
    /// token-rotated-mid-turn test.
    struct StubAuth {
        creds: Mutex<(String, String)>,
    }

    impl StubAuth {
        fn new(base_url: &str, token: &str) -> Self {
            StubAuth {
                creds: Mutex::new((base_url.to_string(), token.to_string())),
            }
        }
        fn rotate(&self, token: &str) {
            self.creds.lock().unwrap().1 = token.to_string();
        }
    }

    impl AuthSource for StubAuth {
        fn current_auth(&self) -> (String, String) {
            self.creds.lock().unwrap().clone()
        }
    }

    /// A fake ACP harness: a `sh` script that reads request lines and replies
    /// with the frames `script` prints.
    ///
    /// The happy-path script answers `initialize`, `session/new`, then streams
    /// two `agent_message_chunk` notifications (plus one non-text `resource`
    /// chunk that must NOT leak into the result) and a `stopReason` response.
    fn harness_from_script(script: &str) -> Harness {
        Harness {
            command: "sh".into(),
            args: vec!["-c".into(), script.into()],
        }
    }

    /// `harness_from_script` with `arg` passed to the script as `$1`.
    ///
    /// Use it for anything path-like. `sh` drops unquoted backslashes, so a
    /// Windows path such as `C:\Users\...` spliced into the script text names
    /// a different file (#64).
    fn harness_from_script_with_arg(script: &str, arg: &str) -> Harness {
        Harness {
            command: "sh".into(),
            // `sh -c SCRIPT NAME ARG`: NAME becomes `$0` and ARG becomes `$1`.
            args: vec!["-c".into(), script.into(), "sh".into(), arg.into()],
        }
    }

    /// An empty file the fake harness appends received lines to, for the test
    /// to read back. Its parent directory name contains a backslash on every
    /// OS (a separator on Windows, an ordinary character elsewhere), so a
    /// script that splices the path instead of quoting `"$1"` fails on Linux
    /// and macOS too, not only on Windows (#64).
    fn dump_file() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().join("dump\\dir");
        std::fs::create_dir_all(&parent).unwrap();
        let path = parent.join("lines.jsonl");
        std::fs::write(&path, "").unwrap();
        (dir, path)
    }

    /// Reply frames emitted as each request line arrives. `sh` reads one line
    /// per request, so the responses stay in lockstep with the poller's three
    /// calls. `printf '%b'` (not `%s`) expands the `\n` separators, so one
    /// "reply" may be several newline-delimited frames — that is how the
    /// prompt step streams notifications before its response.
    fn scripted_harness(reply1: &str, reply2: &str, reply3: &str) -> Harness {
        harness_from_script(&format!(
            "read -r _line; printf '%b\\n' '{reply1}'; \
             read -r _line; printf '%b\\n' '{reply2}'; \
             read -r _line; printf '%b\\n' '{reply3}'; \
             cat > /dev/null"
        ))
    }

    fn ok_init() -> String {
        json!({"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1}}).to_string()
    }

    fn ok_session(id: &str) -> String {
        json!({"jsonrpc":"2.0","id":2,"result":{"sessionId": id}}).to_string()
    }

    /// Two text chunks + one a2ui resource chunk + the end_turn response, all
    /// printed after the `session/prompt` line is read.
    fn streaming_prompt_reply() -> String {
        let chunk = |t: &str| {
            json!({"jsonrpc":"2.0","method":"session/update","params":{
                "sessionId":"s1",
                "update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":t}}
            }})
            .to_string()
        };
        let resource = json!({"jsonrpc":"2.0","method":"session/update","params":{
            "sessionId":"s1",
            "update":{"sessionUpdate":"agent_message_chunk","content":{
                "type":"resource","resource":{"uri":"craftsman:a2ui","text":"NOISE"}}}
        }})
        .to_string();
        let done = json!({"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn"}}).to_string();
        format!(
            "{}\\n{}\\n{}\\n{}",
            chunk("Hel"),
            resource,
            chunk("lo"),
            done
        )
    }

    fn happy_harness() -> Harness {
        scripted_harness(&ok_init(), &ok_session("s1"), &streaming_prompt_reply())
    }

    /// The regression fixture for the `ask_user` wedge.
    ///
    /// Unlike every other fixture here, this one does NOT reply in lockstep:
    /// after the prompt it issues two agent-initiated requests (an ask_user
    /// and a permission request) and **blocks reading their answers** before
    /// emitting the rest of the turn. If the poller ignores either request the
    /// script never proceeds, the turn hits its cap, and the assertions fail —
    /// which is exactly the production deadlock (a harness thread parked in
    /// `block_in_place` waiting for a reply that never comes).
    ///
    /// Answers are appended to `dump_path`, which the script receives as `$1`,
    /// so the test can assert their shape.
    fn interleaved_request_harness(dump_path: &str) -> Harness {
        let chunk = |t: &str| {
            json!({"jsonrpc":"2.0","method":"session/update","params":{
                "sessionId":"s1",
                "update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":t}}
            }})
            .to_string()
        };
        let ask = json!({
            "jsonrpc":"2.0","id":"ask-1","method":"_craftsman/session/request_input",
            "params":{"sessionId":"s1","question":"which directory?"}
        })
        .to_string();
        let perm = json!({
            "jsonrpc":"2.0","id":"perm-1","method":"session/request_permission",
            "params":{"sessionId":"s1","toolCall":{"toolCallId":"perm-1"},"options":[
                {"optionId":"allow-once"},{"optionId":"reject-once"}]}
        })
        .to_string();
        let done = json!({"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn"}}).to_string();
        harness_from_script_with_arg(
            &format!(
                "read -r _l; printf '%s\\n' '{init}'; \
                 read -r _l; printf '%s\\n' '{session}'; \
                 read -r _l; \
                 printf '%s\\n' '{first}'; \
                 printf '%s\\n' '{ask}'; \
                 read -r answer; printf '%s\\n' \"$answer\" >> \"$1\"; \
                 printf '%s\\n' '{perm}'; \
                 read -r answer; printf '%s\\n' \"$answer\" >> \"$1\"; \
                 printf '%s\\n' '{second}'; \
                 printf '%s\\n' '{done}'; \
                 cat > /dev/null",
                init = ok_init(),
                session = ok_session("s1"),
                first = chunk("before "),
                second = chunk("after"),
            ),
            dump_path,
        )
    }

    /// BLOCKING-BUG REGRESSION: an agent that asks a question mid-turn must be
    /// answered, not ignored. Before the fix this test would hang until
    /// `TURN_TIMEOUT` and settle as a timeout error.
    #[tokio::test]
    async fn agent_initiated_requests_are_answered_so_the_turn_completes() {
        let (_dir, dump) = dump_file();
        let harness = interleaved_request_harness(&dump.to_string_lossy());

        // A cap far below TURN_TIMEOUT: if the poller fails to answer, this
        // returns a timeout error instead of hanging the suite for 10 minutes.
        let text = run_message_turn_within(&harness, &msg("m1", "tidy up"), SHORT_CAP)
            .await
            .expect("the turn must complete, not time out");
        assert_eq!(text, "before after", "streamed text survives the detour");

        let answers: Vec<Value> = std::fs::read_to_string(&dump)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(answers.len(), 2, "both requests answered: {answers:?}");

        // ask_user: a JSON-RPC error, which `parse_question_result` maps to
        // None → "proceed with your best assumption".
        assert_eq!(answers[0]["id"], "ask-1");
        assert!(
            answers[0]["error"]["message"].is_string(),
            "ask_user answer must be an error reply: {}",
            answers[0]
        );
        assert!(crate::acp_serve::parse_question_result(&answers[0]).is_none());

        // permission: an explicit reject-once outcome.
        assert_eq!(answers[1]["id"], "perm-1");
        assert_eq!(
            answers[1]["result"]["outcome"],
            json!({"outcome": "selected", "optionId": "reject-once"})
        );
    }

    /// The invariant that makes the id matching safe: ids WE originate are
    /// ints, ids the agent originates are strings, so neither is mistaken for
    /// the other. (acp_serve mints `perm-{n}` / `ask-{n}`.)
    #[test]
    fn agent_request_ids_are_strings_and_our_responses_are_not() {
        let agent_ask = json!({"id":"ask-1","method":"_craftsman/session/request_input"});
        let agent_perm = json!({"id":"perm-2","method":"session/request_permission"});
        assert_eq!(agent_request_id(&agent_ask), Some("ask-1"));
        assert_eq!(agent_request_id(&agent_perm), Some("perm-2"));

        // Our own responses carry an int id and no method — never answerable.
        let our_response = json!({"id": ID_PROMPT, "result": {"stopReason": "end_turn"}});
        assert_eq!(agent_request_id(&our_response), None);
        assert_eq!(our_response["id"].as_i64(), Some(ID_PROMPT));
        // A notification has a method but no id.
        let notification = json!({"method": "session/update", "params": {}});
        assert_eq!(agent_request_id(&notification), None);
    }

    /// Stand-in for [`TURN_TIMEOUT`] in tests: long enough that a working
    /// harness always finishes, short enough that a wedged one fails the test
    /// in seconds instead of ten minutes.
    const SHORT_CAP: Duration = Duration::from_secs(10);

    fn msg(id: &str, content: &str) -> InboxMessage {
        InboxMessage {
            id: id.into(),
            thread_id: None,
            content: content.into(),
        }
    }

    #[tokio::test]
    async fn acp_turn_concatenates_text_chunks_and_accepts_end_turn() {
        let text = run_message_turn(&happy_harness(), &msg("m1", "say hello"))
            .await
            .unwrap();
        assert_eq!(text, "Hello", "resource chunks must not leak into the text");
    }

    /// The harness must actually receive our three requests, in order, with
    /// the thread id pinned in `session/new`.
    #[tokio::test]
    async fn acp_turn_speaks_initialize_session_new_prompt_in_order() {
        let (_dir, dump) = dump_file();
        let harness = harness_from_script_with_arg(
            &format!(
                "read -r l; printf '%s\\n' \"$l\" >> \"$1\"; printf '%s\\n' '{}'; \
                 read -r l; printf '%s\\n' \"$l\" >> \"$1\"; printf '%s\\n' '{}'; \
                 read -r l; printf '%s\\n' \"$l\" >> \"$1\"; printf '%s\\n' '{}'; \
                 cat > /dev/null",
                ok_init(),
                ok_session("s1"),
                json!({"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn"}})
            ),
            &dump.to_string_lossy(),
        );
        let message = InboxMessage {
            id: "m1".into(),
            thread_id: Some("t-9".into()),
            content: "do it".into(),
        };
        assert_eq!(run_message_turn(&harness, &message).await.unwrap(), "");

        let sent: Vec<Value> = std::fs::read_to_string(&dump)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(sent.len(), 3, "got {sent:?}");
        assert_eq!(sent[0]["method"], "initialize");
        assert_eq!(sent[0]["params"]["protocolVersion"], 1);
        assert_eq!(sent[1]["method"], "session/new");
        assert_eq!(sent[1]["params"]["_meta"]["craftsman"]["threadId"], "t-9");
        assert_eq!(sent[2]["method"], "session/prompt");
        assert_eq!(sent[2]["params"]["sessionId"], "s1");
        assert_eq!(sent[2]["params"]["prompt"][0]["text"], "do it");
    }

    #[tokio::test]
    async fn acp_turn_surfaces_a_jsonrpc_error() {
        let harness = scripted_harness(
            &ok_init(),
            &ok_session("s1"),
            &json!({"jsonrpc":"2.0","id":3,"error":{"code":-32603,"message":"agent error: boom"}})
                .to_string(),
        );
        let err = run_message_turn(&harness, &msg("m1", "hi"))
            .await
            .unwrap_err();
        assert!(err.contains("agent error: boom"), "got: {err}");
    }

    /// A non-`end_turn` stop reason is a failure, and the reason is reported.
    #[tokio::test]
    async fn acp_turn_reports_a_non_end_turn_stop_reason() {
        let harness = scripted_harness(
            &ok_init(),
            &ok_session("s1"),
            &json!({"jsonrpc":"2.0","id":3,"result":{"stopReason":"max_turn_requests"}})
                .to_string(),
        );
        let err = run_message_turn(&harness, &msg("m1", "hi"))
            .await
            .unwrap_err();
        assert!(err.contains("max_turn_requests"), "got: {err}");
    }

    #[tokio::test]
    async fn acp_turn_detects_a_harness_that_exits_early() {
        // Answers `initialize`, then dies before `session/new`.
        let harness = harness_from_script(&format!(
            "read -r _l; printf '%s\\n' '{}'; exit 0",
            ok_init()
        ));
        let err = run_message_turn(&harness, &msg("m1", "hi"))
            .await
            .unwrap_err();
        // Whether we notice on the next write (broken pipe) or the next read
        // (stream ended) is a race with the child's exit; both must be a
        // clean, reported failure rather than a hang. Assert the harness's
        // stable operation prefix, not a Unix/Windows/localized OS message.
        assert!(
            err.contains("exited before answering")
                || err.starts_with("harness stdin write failed:")
                || err.starts_with("harness stdin flush failed:"),
            "got: {err}"
        );
    }

    #[tokio::test]
    async fn acp_turn_reports_a_harness_that_cannot_be_spawned() {
        let harness = Harness {
            command: "/nonexistent/craftsman-harness".into(),
            args: vec![],
        };
        let err = run_message_turn(&harness, &msg("m1", "hi"))
            .await
            .unwrap_err();
        assert!(err.contains("failed to spawn harness"), "got: {err}");
    }

    /// A harness that accepts the prompt and never answers must not wedge the
    /// poller: the turn is abandoned once the cap elapses, and the child is
    /// killed on the way out. Tested with a short cap; production uses
    /// [`TURN_TIMEOUT`].
    #[tokio::test]
    async fn acp_turn_times_out_on_a_hung_harness() {
        // Answers initialize + session/new, then reads forever without ever
        // responding to session/prompt.
        let harness = scripted_harness(&ok_init(), &ok_session("s1"), "");
        let err = run_message_turn_within(&harness, &msg("m1", "hi"), Duration::from_millis(300))
            .await
            .unwrap_err();
        assert!(err.contains("exceeded"), "got: {err}");
    }

    /// Full poll → ACP turn → post_result cycle against a mock platform: one
    /// message succeeds (chunks concatenated into `result`), one fails (the
    /// harness errors), both settled, bearer attached.
    #[tokio::test]
    async fn poll_cycle_runs_turns_and_posts_results() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/apps/workspace/api/workspace/inbox"))
            .and(header("authorization", "Bearer tok-1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "messages": [
                    {"id": "m1", "thread_id": "t-7", "content": "say hello"},
                ]
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/apps/workspace/api/workspace/inbox/m1/result"))
            .and(header("authorization", "Bearer tok-1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"message_id": "m1"})))
            .expect(1)
            .mount(&server)
            .await;

        let auth = StubAuth::new(&server.uri(), "tok-1");
        let client = reqwest::Client::new();
        let processed = poll_once(&client, &auth, &happy_harness()).await.unwrap();
        assert_eq!(processed, 1);

        let requests = server.received_requests().await.unwrap();
        let posted: Value = requests
            .iter()
            .find(|r| r.method == wiremock::http::Method::POST)
            .map(|r| serde_json::from_slice(&r.body).unwrap())
            .unwrap();
        assert_eq!(posted, json!({"result": "Hello", "error": null}));
    }

    /// A harness failure is posted as `error`, and the batch continues.
    #[tokio::test]
    async fn turn_failures_are_posted_as_errors() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/apps/workspace/api/workspace/inbox"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "messages": [{"id": "m1", "thread_id": null, "content": "boom"}]
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/apps/workspace/api/workspace/inbox/m1/result"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"message_id": "m1"})))
            .expect(1)
            .mount(&server)
            .await;

        let harness = scripted_harness(
            &ok_init(),
            &ok_session("s1"),
            &json!({"jsonrpc":"2.0","id":3,"error":{"code":-32603,"message":"nope"}}).to_string(),
        );
        let auth = StubAuth::new(&server.uri(), "tok");
        let client = reqwest::Client::new();
        assert_eq!(poll_once(&client, &auth, &harness).await.unwrap(), 1);

        let requests = server.received_requests().await.unwrap();
        let posted: Value = requests
            .iter()
            .find(|r| r.method == wiremock::http::Method::POST)
            .map(|r| serde_json::from_slice(&r.body).unwrap())
            .unwrap();
        assert!(posted["result"].is_null());
        assert!(
            posted["error"].as_str().unwrap().contains("nope"),
            "got {posted}"
        );
    }

    /// A token rotated on disk DURING the turn (by the harness child's own
    /// `ensure_fresh` → `Config::save`) must be the one that settles the
    /// message — the claim-time token may already be dead by then.
    #[tokio::test]
    async fn settle_uses_fresh_token_after_mid_turn_rotation() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/apps/workspace/api/workspace/inbox"))
            .and(header("authorization", "Bearer tok-1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "messages": [{"id": "m1", "thread_id": null, "content": "please rotate"}]
            })))
            .mount(&server)
            .await;
        // The settle endpoint only accepts the ROTATED token.
        Mock::given(method("POST"))
            .and(path("/apps/workspace/api/workspace/inbox/m1/result"))
            .and(header("authorization", "Bearer tok-2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"message_id": "m1"})))
            .expect(1)
            .mount(&server)
            .await;

        /// Rotates on the SECOND read (claim reads tok-1, settle reads tok-2),
        /// standing in for a harness child rewriting config.json mid-turn.
        struct RotatingAuth {
            inner: StubAuth,
            reads: Mutex<u32>,
        }
        impl AuthSource for RotatingAuth {
            fn current_auth(&self) -> (String, String) {
                let creds = self.inner.current_auth();
                let mut reads = self.reads.lock().unwrap();
                *reads += 1;
                if *reads == 1 {
                    self.inner.rotate("tok-2");
                }
                creds
            }
        }

        let auth = RotatingAuth {
            inner: StubAuth::new(&server.uri(), "tok-1"),
            reads: Mutex::new(0),
        };
        let client = reqwest::Client::new();
        assert_eq!(
            poll_once(&client, &auth, &happy_harness()).await.unwrap(),
            1
        );
        // Mock .expect(1) verifies the settle carried Bearer tok-2 on drop.
    }

    /// A settle failure (HTTP 500) is logged and the batch continues: the
    /// next message still runs and posts. Also covers the empty-content
    /// path: no harness is spawned, but the claim is settled with an error.
    #[tokio::test]
    async fn settle_failure_continues_batch_and_empty_content_settles() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/apps/workspace/api/workspace/inbox"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "messages": [
                    {"id": "m1", "thread_id": null, "content": "say hello"},
                    {"id": "m2", "thread_id": null, "content": ""},
                ]
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/apps/workspace/api/workspace/inbox/m1/result"))
            .respond_with(ResponseTemplate::new(500))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/apps/workspace/api/workspace/inbox/m2/result"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"message_id": "m2"})))
            .expect(1)
            .mount(&server)
            .await;

        let auth = StubAuth::new(&server.uri(), "tok");
        let client = reqwest::Client::new();
        let processed = poll_once(&client, &auth, &happy_harness()).await.unwrap();
        assert_eq!(processed, 2, "m1's settle 500 must not abort the batch");
        let requests = server.received_requests().await.unwrap();
        let m2_body: Value = requests
            .iter()
            .find(|r| r.url.path().ends_with("/m2/result"))
            .map(|r| serde_json::from_slice(&r.body).unwrap())
            .expect("empty-content claim must still be settled");
        assert_eq!(
            m2_body,
            json!({"result": null, "error": "inbox message has no content"})
        );
    }

    #[tokio::test]
    async fn poll_treats_204_as_empty_and_non_2xx_as_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/apps/workspace/api/workspace/inbox"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
        let harness = happy_harness();
        let auth = StubAuth::new(&server.uri(), "tok");
        let client = reqwest::Client::new();
        assert_eq!(poll_once(&client, &auth, &harness).await.unwrap(), 0);
        server.reset().await;
        Mock::given(method("GET"))
            .and(path("/apps/workspace/api/workspace/inbox"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        assert!(poll_once(&client, &auth, &harness).await.is_err());
        // 200 with an empty body violates the contract (empty = 204) and
        // must surface as an error, not masquerade as an empty inbox.
        server.reset().await;
        Mock::given(method("GET"))
            .and(path("/apps/workspace/api/workspace/inbox"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        let err = poll_once(&client, &auth, &harness).await.unwrap_err();
        assert!(err.contains("empty body"), "got: {err}");
    }

    /// The ready gate: with no credentials on disk the poller never claims.
    #[tokio::test]
    async fn missing_credentials_skip_the_claim() {
        let auth = StubAuth::new("", "");
        let client = reqwest::Client::new();
        let err = poll_once(&client, &auth, &happy_harness())
            .await
            .unwrap_err();
        assert!(err.contains("credentials missing"), "got: {err}");
        // And a base_url with no token is equally unusable.
        let half = StubAuth::new("http://platform", "");
        assert!(poll_once(&client, &half, &happy_harness()).await.is_err());
    }
}
