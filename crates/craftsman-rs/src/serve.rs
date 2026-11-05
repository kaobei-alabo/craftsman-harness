//! `craftsman serve` — run the CLI agent as a long-lived WS chat server.
//!
//! Expose the agent over a WebSocket so a front-end (the platform, proxying
//! a browser's chat socket) can drive the SAME agent loop the terminal REPL
//! uses, streaming tokens back as
//! frames. Config comes from `~/.craftsman/config.json` like every other command;
//! how it gets there (a normal login, or a workspace pod's platform-managed
//! seeding) is outside this command's concern.
//!
//! Frame protocol v2 (JSON text frames, one object per frame) — the wire
//! contract shared with the frontend kit's `@trend-mgai/agent-ui`
//! `contract.ts`. Keep the two in lockstep.
//!
//! Client → server:
//!   {"type":"message","content":"…","thread_id":"…"?}   run a turn
//!   {"type":"set_model","model":"…"}                    switch model
//!   {"type":"ui_action","name":…,"surface_id":…,        a2ui event →
//!     "source_component_id":…,"context":{…}}            runs a turn
//!   {"type":"history","thread_id":"…"?}                 replay thread
//!   {"type":"list_threads"}                               list conversations
//!   {"type":"new_thread"}                                 start a conversation
//!   {"type":"switch_thread","thread_id":"…"}             open a conversation
//!   {"type":"rename_thread","thread_id":"…","title":"…"}
//!   {"type":"delete_thread","thread_id":"…"}
//!   {"type":"get_voice_status"}                         local Whisper status
//!   {"type":"set_voice_model","model":"…"}             persist Whisper model
//!   {"type":"voice_transcribe","audio":"<base64 pcm16>",
//!     "sample_rate":16000,"operation_id":"…"}            transcribe browser audio
//!
//! Server → client:
//!   {"type":"hello","protocol":2,"model":…,"thread_id":…,"a2ui":true}
//!   {"type":"token","text":"…"}                         streamed deltas
//!   {"type":"a2ui","messages":[…]}                      A2UI v0.9 msgs
//!   {"type":"done","thread_id":…}                       turn complete
//!   {"type":"model","model":…,"ok":bool,"message":…}    set_model ack
//!   {"type":"history","thread_id":…,"messages":[{"role","content"}…]}
//!   {"type":"threads","current_thread_id":…,"threads":[…]}
//!   {"type":"thread","action":"created"|"renamed"|"deleted",…}
//!   {"type":"voice_status","voice":{…}}
//!   {"type":"voice_event","event":"state"|"download_progress"|
//!     "segment_final"|"error",…}
//!   {"type":"error","message":"…"}
//!
//! HTTP surface (non-WS):
//!   GET /healthz → {"status":"ok"|"starting"} — unauthenticated liveness.
//!   GET /stats   → {"disk":{…},"home":{…},"model","uptime_s"} — auth-gated.
//!
//! Design notes:
//! * One tokio runtime end to end: there is no event-loop bridging.
//! * An unauthorized WS dial gets HTTP 403 before the upgrade instead of
//!   accept-then-close(1008) — same observable failure to a client, honest
//!   HTTP on the wire.
//!
//! HITL: the pod IS the sandbox (network-locked), so gated tools are
//! auto-approved rather than surfaced over the wire (`ApprovalState` with
//! both switches on).

use crate::a2ui::{FrameSlot, FrameTx, RenderUiTool, SYSTEM_PROMPT_SUFFIX};
use crate::agent::{self, Session, SessionExtras};
use crate::config::Config;
use crate::hitl::ApprovalState;
use async_trait::async_trait;
use axum::Router;
use axum::extract::ws::{Message as WsMessage, WebSocket};
use axum::extract::{State, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::get;
use base64::Engine;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Header the platform's WS bridge injects on the upstream dial so this pod
/// can trust the proxied connection. Value is a shared secret; a signed
/// per-user assertion can replace it later without changing the wire.
pub(crate) const PROXY_HEADER: &str = "x-craftsman-workspace-proxy";
pub(crate) const PROXY_SECRET_ENV: &str = "CRAFTSMAN_WORKSPACE_PROXY_SECRET";

/// How long the `/stats` home-directory walk is reused before recomputing.
const HOME_CACHE_TTL_SECS: u64 = 60;

/// The agent side of the server, injectable so the protocol layer is testable
/// without building a real agent or contacting a model.
#[async_trait]
pub trait TurnDriver: Send + Sync {
    /// False only while the agent is still being built / failed to build.
    async fn ready(&self) -> bool;
    /// Drive one turn, streaming `token`/`a2ui` frames to `tx`. `Err` is the
    /// message for an `error` frame. Implementations serialize turns.
    async fn run_turn(&self, prompt: String, thread_id: String, tx: FrameTx) -> Result<(), String>;
    /// Switch model (persisted). Returns (ok, error_message).
    async fn set_model(&self, model: String) -> (bool, Option<String>);
    /// `[{"role":"user"|"assistant","content":…}, …]` for a thread. Best
    /// effort: unknown thread = empty.
    async fn history(&self, thread_id: String) -> Vec<Value>;
    /// Persisted conversations in this pod's regular-chat workspace.
    async fn list_threads(&self) -> Result<Vec<Value>, String>;
    /// Start a fresh regular-chat conversation and make it current.
    async fn new_thread(&self) -> Result<String, String>;
    /// Open an existing conversation and return its history.
    async fn switch_thread(&self, thread_id: String) -> Result<Vec<Value>, String>;
    /// Rename a persisted conversation.
    async fn rename_thread(&self, thread_id: String, title: String) -> Result<(), String>;
    /// Delete a persisted conversation, returning the safe current thread.
    async fn delete_thread(&self, thread_id: String) -> Result<String, String>;
    async fn current_thread(&self) -> String;
    async fn current_model(&self) -> String;
    async fn voice_status(&self) -> Value {
        json!({
            "available": false,
            "enabled": false,
            "state": "idle",
            "engine": "unavailable",
            "model": "",
            "language": Value::Null,
            "auto_submit": false,
            "min_words_to_submit": 3,
            "partial_transcripts": false,
            "segment_silence_ms": 800,
            "idle_timeout_ms": 30000,
            "max_session_minutes": 10,
            "models": [],
        })
    }
    async fn set_voice_model(&self, _model: String) -> Result<Value, String> {
        Err("voice model selection is unavailable".into())
    }
    async fn transcribe_voice(
        &self,
        _samples: Vec<i16>,
        _sample_rate: u32,
        _events: tokio::sync::mpsc::UnboundedSender<crate::voice::VoiceEvent>,
        _abort: Arc<AtomicBool>,
    ) -> Result<String, String> {
        Err("voice transcription is unavailable".into())
    }
}

/// The real driver: one [`Session`] serialized behind a tokio mutex (the
/// agent tolerates one in-flight turn; rebuilds must not swap it mid-turn).
pub struct AgentRunner {
    inner: tokio::sync::Mutex<RunnerInner>,
    /// Frame sink slot shared with the session's token sink and `render_ui`
    /// tool; a sender is installed for the duration of each turn.
    slot: FrameSlot,
    approval: Arc<Mutex<ApprovalState>>,
    voice: crate::voice::RemoteVoiceRuntime,
}

struct RunnerInner {
    cfg: Config,
    session: Option<Session>,
}

impl AgentRunner {
    /// Build the runner, then the session with the serve extras. `None`
    /// session = failed build; the server still runs and reports `starting`
    /// / errors per connection.
    pub async fn new(cfg: Config) -> AgentRunner {
        let approval = Arc::new(Mutex::new(ApprovalState {
            mode: crate::config::PermissionMode::BypassPermissions,
            // The container is the sandbox (network-locked): gated tools must
            // not block on a human that isn't there. Covers both switches,
            // like --auto-approve.
            auto_approve: true,
            auto_approve_network: true,
            shell_allow_list: cfg.shell_allow_list.clone(),
        }));
        let slot: FrameSlot = Arc::new(Mutex::new(None));
        let session = build_serve_session(&cfg, &approval, &slot, None).await;
        let voice = crate::voice::RemoteVoiceRuntime::new(&cfg);
        AgentRunner {
            inner: tokio::sync::Mutex::new(RunnerInner { cfg, session }),
            slot,
            approval,
            voice,
        }
    }
}

/// Session for serve mode: quiet, auto-approved, tokens to the frame slot,
/// a2ui tool + prompt block wired in.
async fn build_serve_session(
    cfg: &Config,
    approval: &Arc<Mutex<ApprovalState>>,
    slot: &FrameSlot,
    thread_id: Option<String>,
) -> Option<Session> {
    let sink_slot = slot.clone();
    let extras = SessionExtras {
        token_sink: Some(Arc::new(move |t: &str| {
            if let Some(tx) = &*sink_slot.lock().unwrap() {
                let _ = tx.send(json!({"type": "token", "text": t}));
            }
        })),
        extra_tools: vec![Arc::new(RenderUiTool::new(slot.clone()))],
        ..SessionExtras::default()
    };
    agent::build_session_ext(
        cfg,
        approval.clone(),
        SYSTEM_PROMPT_SUFFIX,
        true, // activity → stderr (pod log), never the protocol stream
        false,
        thread_id,
        extras,
    )
    .await
}

#[async_trait]
impl TurnDriver for AgentRunner {
    async fn ready(&self) -> bool {
        self.inner.lock().await.session.is_some()
    }

    async fn run_turn(&self, prompt: String, thread_id: String, tx: FrameTx) -> Result<(), String> {
        let mut inner = self.inner.lock().await;
        let Some(session) = inner.session.as_mut() else {
            return Err("agent failed to start".into());
        };
        // Thread switch: resume the requested thread from the checkpointer,
        // or start it fresh when it doesn't exist yet (like the REPL's
        // /session + /clear semantics).
        if thread_id != session.thread_id {
            match session.load_thread(&thread_id).await {
                Ok(true) => {}
                Ok(false) => {
                    session.set_thread_id(thread_id.clone());
                    *session.agent.state.lock().unwrap() = Default::default();
                }
                Err(e) => return Err(format!("could not load thread {thread_id}: {e}")),
            }
        }

        // Attach the frame sink for the turn; ALWAYS detached after (a stale
        // sender would leak the next turn's frames to a gone socket).
        *self.slot.lock().unwrap() = Some(tx);
        let result = session.agent.run_turn(&prompt).await;
        *self.slot.lock().unwrap() = None;

        match result {
            Ok(craftsman_harness::TurnResult::Done(_)) => Ok(()),
            // auto_approve is on, so the gate never suspends; a suspension
            // here is a bug surfaced honestly rather than a hang.
            Ok(craftsman_harness::TurnResult::Suspended(s)) => {
                agent::rollback_dangling_user(&session.agent);
                Err(format!("unexpected suspension: {}", s.request.summary))
            }
            Err(e) => {
                agent::rollback_dangling_user(&session.agent);
                Err(e.to_string())
            }
        }
    }

    async fn set_model(&self, model: String) -> (bool, Option<String>) {
        let model = model.trim().to_string();
        if model.is_empty() {
            return (false, Some("model must be a non-empty string".into()));
        }
        let mut inner = self.inner.lock().await;
        let previous = inner.cfg.model.clone();
        if previous.as_deref() == Some(model.as_str()) {
            return (true, None);
        }
        let thread = inner.session.as_ref().map(|s| s.thread_id.clone());
        inner.cfg.model = Some(model);
        let _ = inner.cfg.save();
        if let Some(rebuilt) =
            build_serve_session(&inner.cfg, &self.approval, &self.slot, thread.clone()).await
        {
            inner.session = Some(rebuilt);
            return (true, None);
        }
        // Roll back so the session stays usable on the old model.
        inner.cfg.model = previous;
        let _ = inner.cfg.save();
        if let Some(rebuilt) =
            build_serve_session(&inner.cfg, &self.approval, &self.slot, thread).await
        {
            inner.session = Some(rebuilt);
        }
        (false, Some("agent rebuild failed for that model".into()))
    }

    async fn voice_status(&self) -> Value {
        let inner = self.inner.lock().await;
        let voice = crate::voice::VoiceConfig::from_config(&inner.cfg);
        json!({
            "available": crate::voice::available(),
            "enabled": voice.enabled,
            "state": "idle",
            "engine": voice.engine,
            "model": voice.model,
            "language": voice.language,
            "auto_submit": voice.auto_submit,
            "min_words_to_submit": voice.min_words_to_submit,
            "partial_transcripts": voice.partial_transcripts,
            "segment_silence_ms": voice.vad_silence_ms,
            "idle_timeout_ms": voice.idle_timeout_ms,
            "max_session_minutes": voice.max_session_minutes,
            "models": crate::voice::model_specs().map(|model| json!({
                "name": model.name,
                "bytes": model.bytes,
                "downloaded": crate::voice::model_downloaded(model.name),
            })).collect::<Vec<_>>(),
        })
    }

    async fn set_voice_model(&self, model: String) -> Result<Value, String> {
        let canonical = crate::voice::canonical_model_name(&model)
            .ok_or_else(|| format!("unknown voice model: {model}"))?;
        let mut inner = self.inner.lock().await;
        inner.cfg.set_voice_setting("model", json!(canonical));
        inner.cfg.save().map_err(|error| error.to_string())?;
        let config = crate::voice::VoiceConfig::from_config(&inner.cfg);
        drop(inner);
        self.voice.reconfigure(config).await;
        Ok(self.voice_status().await)
    }

    async fn transcribe_voice(
        &self,
        samples: Vec<i16>,
        sample_rate: u32,
        events: tokio::sync::mpsc::UnboundedSender<crate::voice::VoiceEvent>,
        abort: Arc<AtomicBool>,
    ) -> Result<String, String> {
        self.voice
            .transcribe_pcm16(samples, sample_rate, events, abort)
            .await
            .map_err(|error| {
                let model = self
                    .inner
                    .try_lock()
                    .ok()
                    .map(|inner| crate::voice::VoiceConfig::from_config(&inner.cfg).model)
                    .unwrap_or_else(|| "large-v3-turbo-q5_0".into());
                error.user_message(&model)
            })
    }

    async fn history(&self, thread_id: String) -> Vec<Value> {
        let inner = self.inner.lock().await;
        let Some(session) = inner.session.as_ref() else {
            return Vec::new();
        };
        // The live thread reads straight from agent state; any other thread
        // peeks at its latest checkpoint without disturbing the live agent.
        let messages = if thread_id == session.thread_id {
            session.agent.state.lock().unwrap().messages.clone()
        } else {
            let Some(cp) = session.checkpointer.as_ref() else {
                return Vec::new();
            };
            use craftsman_harness::Checkpointer;
            match cp.load_latest(&thread_id).await {
                Ok(Some((_, state, _))) => state.messages,
                _ => Vec::new(),
            }
        };
        messages
            .iter()
            .filter_map(|m| match m {
                craftsman_harness::Msg::User { content } if !content.is_empty() => {
                    Some(json!({"role": "user", "content": content}))
                }
                craftsman_harness::Msg::Assistant { content, .. } if !content.is_empty() => {
                    Some(json!({"role": "assistant", "content": content}))
                }
                _ => None,
            })
            .collect()
    }

    async fn list_threads(&self) -> Result<Vec<Value>, String> {
        let inner = self.inner.lock().await;
        let Some(session) = inner.session.as_ref() else {
            return Err("agent failed to start".into());
        };
        let Some(checkpointer) = session.checkpointer.as_ref() else {
            return Ok(vec![new_thread_summary(&session.thread_id)]);
        };
        use craftsman_harness::Checkpointer;
        let mut threads = checkpointer
            .list_threads()
            .await
            .map_err(|error| error.to_string())?
            .into_iter()
            .map(|thread| {
                json!({
                    "id": thread.id,
                    "created": thread.created,
                    "updated": thread.updated,
                    "title": thread.title,
                    "checkpoints": thread.checkpoints,
                })
            })
            .collect::<Vec<_>>();
        if !threads
            .iter()
            .any(|thread| thread["id"].as_str() == Some(session.thread_id.as_str()))
        {
            threads.insert(0, new_thread_summary(&session.thread_id));
        }
        Ok(threads)
    }

    async fn new_thread(&self) -> Result<String, String> {
        let mut inner = self.inner.lock().await;
        let Some(session) = inner.session.as_mut() else {
            return Err("agent failed to start".into());
        };
        let thread_id = crate::checkpoints::LibsqlCheckpointer::new_thread_id();
        agent::clear_session(session);
        session.set_thread_id(thread_id.clone());
        Ok(thread_id)
    }

    async fn switch_thread(&self, thread_id: String) -> Result<Vec<Value>, String> {
        let mut inner = self.inner.lock().await;
        let Some(session) = inner.session.as_mut() else {
            return Err("agent failed to start".into());
        };
        if thread_id != session.thread_id
            && !session
                .load_thread(&thread_id)
                .await
                .map_err(|error| error.to_string())?
        {
            return Err(format!("conversation not found: {thread_id}"));
        }
        Ok(history_values(
            &session.agent.state.lock().unwrap().messages,
        ))
    }

    async fn rename_thread(&self, thread_id: String, title: String) -> Result<(), String> {
        let inner = self.inner.lock().await;
        let Some(session) = inner.session.as_ref() else {
            return Err("agent failed to start".into());
        };
        let Some(checkpointer) = session.checkpointer.as_ref() else {
            return Err("conversation storage is unavailable".into());
        };
        checkpointer
            .rename_thread(&thread_id, &title)
            .await
            .map_err(|error| error.to_string())
    }

    async fn delete_thread(&self, thread_id: String) -> Result<String, String> {
        let mut inner = self.inner.lock().await;
        let Some(session) = inner.session.as_mut() else {
            return Err("agent failed to start".into());
        };
        let Some(checkpointer) = session.checkpointer.clone() else {
            return Err("conversation storage is unavailable".into());
        };
        use craftsman_harness::Checkpointer;
        checkpointer
            .delete_thread(&thread_id)
            .await
            .map_err(|error| error.to_string())?;
        if session.thread_id == thread_id {
            let next = crate::checkpoints::LibsqlCheckpointer::new_thread_id();
            agent::clear_session(session);
            session.set_thread_id(next);
        }
        Ok(session.thread_id.clone())
    }

    async fn current_thread(&self) -> String {
        self.inner
            .lock()
            .await
            .session
            .as_ref()
            .map(|s| s.thread_id.clone())
            .unwrap_or_default()
    }

    async fn current_model(&self) -> String {
        self.inner
            .lock()
            .await
            .cfg
            .model
            .clone()
            .unwrap_or_default()
    }
}

fn new_thread_summary(thread_id: &str) -> Value {
    json!({
        "id": thread_id,
        "created": 0,
        "updated": 0,
        "title": "New chat",
        "checkpoints": 0,
    })
}

pub(crate) fn history_values(messages: &[craftsman_harness::Msg]) -> Vec<Value> {
    messages
        .iter()
        .filter_map(|message| match message {
            craftsman_harness::Msg::User { content } if !content.is_empty() => {
                Some(json!({"role": "user", "content": content}))
            }
            craftsman_harness::Msg::Assistant { content, .. } if !content.is_empty() => {
                Some(json!({"role": "assistant", "content": content}))
            }
            _ => None,
        })
        .collect()
}

/// True when the inbound connection carries the platform proxy secret. With
/// `CRAFTSMAN_WORKSPACE_PROXY_SECRET` unset (local dev) auth is disabled — the
/// NetworkPolicy is then the only gate. In the pod the env is always set, so
/// a direct dial without the header is rejected.
pub(crate) fn authorized(headers: &HeaderMap) -> bool {
    authorized_from_secret(headers, std::env::var_os(PROXY_SECRET_ENV))
}

fn authorized_from_secret(headers: &HeaderMap, secret: Option<std::ffi::OsString>) -> bool {
    let Some(secret) = secret else {
        return true;
    };
    let secret = secret.to_string_lossy().into_owned();
    if secret.is_empty() {
        return true;
    }
    let Some(presented) = headers.get(PROXY_HEADER).and_then(|v| v.to_str().ok()) else {
        return false;
    };
    constant_time_eq(presented.as_bytes(), secret.as_bytes())
}

/// Constant-time byte compare (length still leaks).
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Shared state behind the router.
#[derive(Clone)]
struct AppState {
    driver: Arc<dyn TurnDriver>,
    started: Instant,
    /// 60s cache for the `/stats` home-directory walk (a big home must not
    /// turn a poll endpoint into an IO hammer).
    home_cache: Arc<Mutex<Option<(Instant, u64)>>>,
    #[cfg(test)]
    proxy_fixture: Option<Arc<Mutex<Option<&'static str>>>>,
}

impl AppState {
    fn authorized(&self, headers: &HeaderMap) -> bool {
        #[cfg(test)]
        if let Some(fixture) = &self.proxy_fixture {
            let secret = match fixture.lock() {
                Ok(secret) => *secret,
                Err(_) => return false,
            };
            return authorized_from_secret(headers, secret.map(std::ffi::OsString::from));
        }
        authorized(headers)
    }
}

/// Build the router. `driver` is injectable for tests (stub instead of a
/// real agent).
pub fn build_router(driver: Arc<dyn TurnDriver>) -> Router {
    let state = AppState {
        driver,
        started: Instant::now(),
        home_cache: Arc::new(Mutex::new(None)),
        #[cfg(test)]
        proxy_fixture: None,
    };
    router_from_state(state)
}

fn router_from_state(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/stats", get(stats))
        .route("/chat", get(chat_upgrade))
        .with_state(state)
}

async fn healthz(State(state): State<AppState>) -> Json<Value> {
    let status = if state.driver.ready().await {
        "ok"
    } else {
        "starting"
    };
    Json(json!({"status": status}))
}

async fn stats(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !state.authorized(&headers) {
        return (StatusCode::FORBIDDEN, Json(json!({"detail": "forbidden"}))).into_response();
    }
    let home = home_dir();
    let (disk_used, disk_total) = disk_usage(&home);
    // The walk can take a while on a big home dir — run it off the async
    // workers, and never while holding the cache lock.
    let cached = {
        let cache = state.home_cache.lock().unwrap();
        cache.and_then(|(at, v)| (at.elapsed().as_secs() < HOME_CACHE_TTL_SECS).then_some(v))
    };
    let home_used = match cached {
        Some(v) => v,
        None => {
            let walk_root = home.clone();
            let v = tokio::task::spawn_blocking(move || dir_used_bytes(&walk_root))
                .await
                .unwrap_or(0);
            *state.home_cache.lock().unwrap() = Some((Instant::now(), v));
            v
        }
    };
    let model = state.driver.current_model().await;
    Json(json!({
        "disk": {"used_bytes": disk_used, "total_bytes": disk_total},
        "home": {"used_bytes": home_used},
        "model": model,
        "uptime_s": state.started.elapsed().as_secs_f64(),
    }))
    .into_response()
}

async fn chat_upgrade(
    State(state): State<AppState>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if !state.authorized(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    ws.on_upgrade(move |socket| chat_session(socket, state))
}

async fn chat_session(mut ws: WebSocket, state: AppState) {
    let driver = state.driver;
    if !driver.ready().await {
        let _ = send_json(
            &mut ws,
            &json!({"type":"error","message":"agent failed to start"}),
        )
        .await;
        return;
    }
    let model = driver.current_model().await;
    let voice = driver.voice_status().await;
    let capabilities = if voice["available"].as_bool() == Some(true) {
        vec!["voice"]
    } else {
        Vec::new()
    };
    let hello = json!({
        "type": "hello",
        "protocol": 2,
        "model": model,
        "thread_id": driver.current_thread().await,
        "a2ui": true,
        "capabilities": capabilities,
    });
    if send_json(&mut ws, &hello).await.is_err() {
        return;
    }

    while let Some(Ok(msg)) = ws.recv().await {
        let WsMessage::Text(raw) = msg else { continue };
        let Ok(req) = serde_json::from_str::<Value>(&raw) else {
            let _ = send_json(&mut ws, &json!({"type":"error","message":"invalid json"})).await;
            continue;
        };
        match req["type"].as_str().unwrap_or("") {
            "message" => {
                let prompt = req["content"].as_str().unwrap_or("").to_string();
                if prompt.is_empty() {
                    continue;
                }
                let thread_id = requested_thread(&req, &driver).await;
                if run_turn_over_ws(&mut ws, &driver, prompt, thread_id)
                    .await
                    .is_err()
                {
                    return;
                }
            }
            "ui_action" => {
                // A canvas interaction (A2UI action event), encoded as a
                // regular turn — the system prompt teaches the agent the
                // `[ui_action] {json}` convention. Keys are emitted in fixed
                // order (sorted) so the turn text is deterministic.
                let name = req["name"].as_str().unwrap_or("").to_string();
                if name.is_empty() {
                    let _ = send_json(
                        &mut ws,
                        &json!({"type":"error","message":"ui_action needs a name"}),
                    )
                    .await;
                    continue;
                }
                let payload = json!({
                    "context": req.get("context").cloned().unwrap_or_else(|| json!({})),
                    "name": name,
                    "source_component_id": req["source_component_id"].as_str().unwrap_or(""),
                    "surface_id": req["surface_id"].as_str().unwrap_or(""),
                });
                let thread_id = requested_thread(&req, &driver).await;
                let prompt = format!("[ui_action] {payload}");
                if run_turn_over_ws(&mut ws, &driver, prompt, thread_id)
                    .await
                    .is_err()
                {
                    return;
                }
            }
            "set_model" => {
                let model = req["model"].as_str().unwrap_or("").to_string();
                let (ok, message) = driver.set_model(model).await;
                let ack = json!({
                    "type": "model",
                    "model": driver.current_model().await,
                    "ok": ok,
                    "message": message,
                });
                if send_json(&mut ws, &ack).await.is_err() {
                    return;
                }
            }
            "history" => {
                let thread_id = requested_thread(&req, &driver).await;
                let messages = driver.history(thread_id.clone()).await;
                let frame = json!({
                    "type": "history",
                    "thread_id": thread_id,
                    "messages": messages,
                });
                if send_json(&mut ws, &frame).await.is_err() {
                    return;
                }
            }
            "list_threads" => {
                if send_threads(&mut ws, &driver).await.is_err() {
                    return;
                }
            }
            "new_thread" => match driver.new_thread().await {
                Ok(thread_id) => {
                    let frame = json!({
                        "type": "thread",
                        "action": "created",
                        "thread_id": thread_id,
                        "current_thread_id": driver.current_thread().await,
                    });
                    if send_json(&mut ws, &frame).await.is_err()
                        || send_threads(&mut ws, &driver).await.is_err()
                    {
                        return;
                    }
                }
                Err(message) => {
                    if send_error(&mut ws, message).await.is_err() {
                        return;
                    }
                }
            },
            "switch_thread" => {
                let Some(thread_id) = required_string(&req, "thread_id") else {
                    if send_error(&mut ws, "switch_thread needs a thread_id")
                        .await
                        .is_err()
                    {
                        return;
                    }
                    continue;
                };
                match driver.switch_thread(thread_id.clone()).await {
                    Ok(messages) => {
                        let frame = json!({
                            "type": "history",
                            "thread_id": thread_id,
                            "messages": messages,
                            "switched": true,
                        });
                        if send_json(&mut ws, &frame).await.is_err()
                            || send_threads(&mut ws, &driver).await.is_err()
                        {
                            return;
                        }
                    }
                    Err(message) => {
                        if send_error(&mut ws, message).await.is_err() {
                            return;
                        }
                    }
                }
            }
            "rename_thread" => {
                let Some(thread_id) = required_string(&req, "thread_id") else {
                    if send_error(&mut ws, "rename_thread needs a thread_id")
                        .await
                        .is_err()
                    {
                        return;
                    }
                    continue;
                };
                let Some(title) = required_string(&req, "title") else {
                    if send_error(&mut ws, "rename_thread needs a title")
                        .await
                        .is_err()
                    {
                        return;
                    }
                    continue;
                };
                match driver.rename_thread(thread_id.clone(), title.clone()).await {
                    Ok(()) => {
                        let frame = json!({
                            "type": "thread",
                            "action": "renamed",
                            "thread_id": thread_id,
                            "title": title.trim(),
                            "current_thread_id": driver.current_thread().await,
                        });
                        if send_json(&mut ws, &frame).await.is_err()
                            || send_threads(&mut ws, &driver).await.is_err()
                        {
                            return;
                        }
                    }
                    Err(message) => {
                        if send_error(&mut ws, message).await.is_err() {
                            return;
                        }
                    }
                }
            }
            "delete_thread" => {
                let Some(thread_id) = required_string(&req, "thread_id") else {
                    if send_error(&mut ws, "delete_thread needs a thread_id")
                        .await
                        .is_err()
                    {
                        return;
                    }
                    continue;
                };
                match driver.delete_thread(thread_id.clone()).await {
                    Ok(current_thread_id) => {
                        let frame = json!({
                            "type": "thread",
                            "action": "deleted",
                            "thread_id": thread_id,
                            "current_thread_id": current_thread_id,
                        });
                        if send_json(&mut ws, &frame).await.is_err()
                            || send_threads(&mut ws, &driver).await.is_err()
                        {
                            return;
                        }
                    }
                    Err(message) => {
                        if send_error(&mut ws, message).await.is_err() {
                            return;
                        }
                    }
                }
            }
            "get_voice_status" => {
                let frame = json!({
                    "type": "voice_status",
                    "operation_id": req["operation_id"].as_str(),
                    "voice": driver.voice_status().await,
                });
                if send_json(&mut ws, &frame).await.is_err() {
                    return;
                }
            }
            "set_voice_model" => {
                let Some(model) = required_string(&req, "model") else {
                    if send_voice_error(
                        &mut ws,
                        req["operation_id"].as_str(),
                        "voice_model_invalid",
                        "voice model is required",
                    )
                    .await
                    .is_err()
                    {
                        return;
                    }
                    continue;
                };
                match driver.set_voice_model(model).await {
                    Ok(voice) => {
                        let frame = json!({
                            "type": "voice_status",
                            "operation_id": req["operation_id"].as_str(),
                            "voice": voice,
                        });
                        if send_json(&mut ws, &frame).await.is_err() {
                            return;
                        }
                    }
                    Err(message) => {
                        if send_voice_error(
                            &mut ws,
                            req["operation_id"].as_str(),
                            "voice_model_failed",
                            &message,
                        )
                        .await
                        .is_err()
                        {
                            return;
                        }
                    }
                }
            }
            "voice_transcribe" => {
                let operation_id = req["operation_id"].as_str().map(str::to_string);
                let sample_rate = req["sample_rate"]
                    .as_u64()
                    .and_then(|value| u32::try_from(value).ok());
                let decoded = req["audio"]
                    .as_str()
                    .ok_or_else(|| "voice_transcribe needs base64 PCM16 audio".to_string())
                    .and_then(decode_pcm16);
                let (sample_rate, samples) = match (sample_rate, decoded) {
                    (Some(sample_rate), Ok(samples)) => (sample_rate, samples),
                    (_, Err(message)) => {
                        if send_voice_error(
                            &mut ws,
                            operation_id.as_deref(),
                            "voice_audio_invalid",
                            &message,
                        )
                        .await
                        .is_err()
                        {
                            return;
                        }
                        continue;
                    }
                    (None, Ok(_)) => {
                        if send_voice_error(
                            &mut ws,
                            operation_id.as_deref(),
                            "voice_audio_invalid",
                            "voice_transcribe needs a valid sample_rate",
                        )
                        .await
                        .is_err()
                        {
                            return;
                        }
                        continue;
                    }
                };
                if !(8_000..=192_000).contains(&sample_rate) {
                    if send_voice_error(
                        &mut ws,
                        operation_id.as_deref(),
                        "voice_audio_invalid",
                        "sample_rate must be between 8000 and 192000",
                    )
                    .await
                    .is_err()
                    {
                        return;
                    }
                    continue;
                }
                if run_voice_transcription_over_ws(
                    &mut ws,
                    &driver,
                    samples,
                    sample_rate,
                    operation_id,
                )
                .await
                .is_err()
                {
                    return;
                }
            }
            // Unknown types are ignored (forward-compat).
            _ => {}
        }
    }
}

const MAX_VOICE_PCM_BYTES: usize = 4 * 1024 * 1024;

fn decode_pcm16(encoded: &str) -> Result<Vec<i16>, String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| "voice audio is not valid base64".to_string())?;
    if bytes.is_empty() || bytes.len() > MAX_VOICE_PCM_BYTES || bytes.len() % 2 != 0 {
        return Err("voice audio must be non-empty little-endian PCM16 under 4 MiB".into());
    }
    Ok(bytes
        .chunks_exact(2)
        .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
        .collect())
}

async fn send_voice_error(
    ws: &mut WebSocket,
    operation_id: Option<&str>,
    code: &str,
    message: &str,
) -> Result<(), ()> {
    send_json(
        ws,
        &json!({
            "type": "voice_event",
            "event": "error",
            "operation_id": operation_id,
            "code": code,
            "message": message,
        }),
    )
    .await
}

fn voice_state_name(state: crate::voice::VoiceState) -> &'static str {
    use crate::voice::VoiceState;
    match state {
        VoiceState::Idle => "idle",
        VoiceState::Initializing => "initializing",
        VoiceState::Downloading => "downloading",
        VoiceState::Recording => "recording",
        VoiceState::Finalizing => "finalizing",
        VoiceState::Paused => "paused",
    }
}

fn voice_event_frame(event: crate::voice::VoiceEvent, operation_id: Option<&str>) -> Option<Value> {
    use crate::voice::VoiceEvent;
    match event {
        VoiceEvent::State(state) => Some(json!({
            "type": "voice_event", "event": "state", "operation_id": operation_id, "state": voice_state_name(state),
        })),
        VoiceEvent::DownloadProgress { downloaded, total } => Some(json!({
            "type": "voice_event", "event": "download_progress", "operation_id": operation_id, "downloaded": downloaded, "total": total,
        })),
        VoiceEvent::Level(level) => Some(json!({
            "type": "voice_event", "event": "level", "operation_id": operation_id, "level": level,
        })),
        _ => None,
    }
}

async fn run_voice_transcription_over_ws(
    ws: &mut WebSocket,
    driver: &Arc<dyn TurnDriver>,
    samples: Vec<i16>,
    sample_rate: u32,
    operation_id: Option<String>,
) -> Result<(), ()> {
    let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();
    let abort = Arc::new(AtomicBool::new(false));
    let task_abort = abort.clone();
    let drv = driver.clone();
    let mut task = tokio::spawn(async move {
        drv.transcribe_voice(samples, sample_rate, event_tx, task_abort)
            .await
    });
    let started = Instant::now();
    let mut current_state = "initializing";
    let mut events_open = true;
    let mut heartbeat = tokio::time::interval(std::time::Duration::from_secs(5));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    heartbeat.tick().await;
    let result = loop {
        tokio::select! {
            event = event_rx.recv(), if events_open => match event {
                Some(event) => {
                    if let crate::voice::VoiceEvent::State(state) = &event {
                        current_state = voice_state_name(*state);
                    }
                    if let Some(frame) = voice_event_frame(event, operation_id.as_deref()) {
                        if send_json(ws, &frame).await.is_err() {
                            abort.store(true, Ordering::Relaxed);
                            task.abort();
                            return Err(());
                        }
                    }
                }
                None => events_open = false,
            },
            task_result = &mut task => {
                while let Ok(event) = event_rx.try_recv() {
                    if let Some(frame) = voice_event_frame(event, operation_id.as_deref()) {
                        if send_json(ws, &frame).await.is_err() {
                            abort.store(true, Ordering::Relaxed);
                            return Err(());
                        }
                    }
                }
                break task_result;
            },
            _ = heartbeat.tick() => {
                let frame = json!({
                    "type": "voice_event",
                    "event": "state",
                    "operation_id": operation_id,
                    "state": current_state,
                    "elapsed_ms": started.elapsed().as_millis() as u64,
                });
                if send_json(ws, &frame).await.is_err() {
                    abort.store(true, Ordering::Relaxed);
                    task.abort();
                    return Err(());
                }
            }
        }
    };
    match result {
        Ok(Ok(text)) => {
            send_json(
                ws,
                &json!({
                    "type": "voice_event",
                    "event": "segment_final",
                    "operation_id": operation_id,
                    "text": text,
                    "committed": "",
                    "partials_enabled": false,
                }),
            )
            .await?;
            send_json(
                ws,
                &json!({
                    "type": "voice_status",
                    "voice": driver.voice_status().await,
                }),
            )
            .await
        }
        Ok(Err(message)) => {
            send_voice_error(
                ws,
                operation_id.as_deref(),
                "voice_transcription_failed",
                &message,
            )
            .await
        }
        Err(error) => {
            send_voice_error(
                ws,
                operation_id.as_deref(),
                "voice_transcription_failed",
                &format!("voice worker failed: {error}"),
            )
            .await
        }
    }
}

/// The request's thread id, defaulting to the driver's current thread.
async fn requested_thread(req: &Value, driver: &Arc<dyn TurnDriver>) -> String {
    match req["thread_id"].as_str().filter(|s| !s.is_empty()) {
        Some(t) => t.to_string(),
        None => driver.current_thread().await,
    }
}

fn required_string(req: &Value, key: &str) -> Option<String> {
    req[key]
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

async fn send_threads(ws: &mut WebSocket, driver: &Arc<dyn TurnDriver>) -> Result<(), ()> {
    match driver.list_threads().await {
        Ok(threads) => {
            let frame = json!({
                "type": "threads",
                "current_thread_id": driver.current_thread().await,
                "threads": threads,
            });
            send_json(ws, &frame).await
        }
        Err(message) => send_error(ws, message).await,
    }
}

async fn send_error(ws: &mut WebSocket, message: impl Into<String>) -> Result<(), ()> {
    send_json(ws, &json!({"type": "error", "message": message.into()})).await
}

/// Drive one turn: spawn the driver, forward its frames as they stream, then
/// close with `done` (or `error`). `Err(())` = the socket is gone.
async fn run_turn_over_ws(
    ws: &mut WebSocket,
    driver: &Arc<dyn TurnDriver>,
    prompt: String,
    thread_id: String,
) -> Result<(), ()> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let drv = driver.clone();
    let tid = thread_id.clone();
    let task = tokio::spawn(async move { drv.run_turn(prompt, tid, tx).await });
    // The channel closes when the turn drops its sender — drain fully first.
    while let Some(frame) = rx.recv().await {
        send_json(ws, &frame).await?;
    }
    let closing = match task.await {
        Ok(Ok(())) => json!({"type": "done", "thread_id": thread_id}),
        Ok(Err(message)) => json!({"type": "error", "message": message}),
        Err(join_err) => json!({"type": "error", "message": format!("turn panicked: {join_err}")}),
    };
    send_json(ws, &closing).await
}

async fn send_json(ws: &mut WebSocket, frame: &Value) -> Result<(), ()> {
    ws.send(WsMessage::Text(frame.to_string().into()))
        .await
        .map_err(|_| ())
}

pub(crate) fn home_dir() -> std::path::PathBuf {
    dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("."))
}

/// (used, total) bytes of the filesystem containing `path` — generic
/// capacity/headroom facts (on hostPath-style k8s storage this is the NODE
/// disk, which is why `/stats` also reports the `home` walk).
pub(crate) fn disk_usage(path: &std::path::Path) -> (u64, u64) {
    crate::platform_os::disk_usage(path)
}

/// Bytes actually stored under `root`: iterative walk summing regular-file
/// sizes, never following symlinks (they contribute nothing and can't cause
/// cycles or escape `root`). Per-entry errors are skipped; a missing or
/// unreadable root yields 0.
pub(crate) fn dir_used_bytes(root: &std::path::Path) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            // `DirEntry::metadata` does not traverse symlinks, so a link is
            // neither a dir nor a regular file here beyond itself.
            if meta.is_dir() && !meta.is_symlink() {
                stack.push(entry.path());
            } else if meta.is_file() {
                total += meta.len();
            }
        }
    }
    total
}

/// Synchronous entrypoint for `craftsman serve`.
pub async fn run(cfg: Config, host: &str, port: u16) -> anyhow::Result<()> {
    let capabilities = crate::execution_profile::ExecutionProfile::Workspace.capabilities();
    debug_assert!(capabilities.real_filesystem);
    debug_assert!(!capabilities.virtual_filesystem);
    debug_assert!(capabilities.shell);
    debug_assert!(capabilities.local_sessions);
    debug_assert!(capabilities.persistent_home);
    if cfg.model.is_none() {
        anyhow::bail!(
            "no model configured; `craftsman serve` needs cfg.model set \
             (the workspace pod seeds it in config.json)"
        );
    }
    let port = std::env::var("CRAFTSMAN_SERVE_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(port);
    let runner = Arc::new(AgentRunner::new(cfg).await);
    // The workspace inbox worker does NOT live here any more: the pod's
    // entrypoint is `craftsman acp-bridge`, so the poller is spawned there (see
    // `inbox_poller::spawn`). Running one from serve too would let two
    // processes claim from the same inbox — and a claim is irreversible.
    let app = build_router(runner);
    let listener = tokio::net::TcpListener::bind((host, port)).await?;
    eprintln!("craftsman serve listening on {host}:{port}");
    axum::serve(listener, app).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::{SinkExt, StreamExt};

    #[test]
    fn constant_time_eq_matches_semantics() {
        assert!(constant_time_eq(b"secret", b"secret"));
        assert!(!constant_time_eq(b"secret", b"secreT"));
        assert!(!constant_time_eq(b"secret", b"secre"));
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn dir_walk_counts_files_and_skips_symlinks() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.txt"), b"12345").unwrap();
        std::fs::create_dir(tmp.path().join("sub")).unwrap();
        std::fs::write(tmp.path().join("sub/b.txt"), b"123").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(tmp.path().join("a.txt"), tmp.path().join("link")).unwrap();
        assert_eq!(dir_used_bytes(tmp.path()), 8);
        assert_eq!(dir_used_bytes(&tmp.path().join("missing")), 0);
    }

    /// Scripted stand-in for the agent: echoes two token frames + one a2ui
    /// frame per turn, records prompts, accepts one specific model.
    struct StubDriver {
        prompts: Mutex<Vec<(String, String)>>,
        current_thread: Mutex<String>,
        threads: Mutex<Vec<Value>>,
    }

    #[async_trait]
    impl TurnDriver for StubDriver {
        async fn ready(&self) -> bool {
            true
        }
        async fn run_turn(
            &self,
            prompt: String,
            thread_id: String,
            tx: FrameTx,
        ) -> Result<(), String> {
            if prompt.contains("explode") {
                return Err("stub turn failure".into());
            }
            self.prompts.lock().unwrap().push((prompt, thread_id));
            let _ = tx.send(json!({"type":"token","text":"Hel"}));
            let _ = tx.send(json!({"type":"token","text":"lo"}));
            let _ = tx.send(json!({"type":"a2ui","messages":[{"v":"stub"}]}));
            Ok(())
        }
        async fn set_model(&self, model: String) -> (bool, Option<String>) {
            if model == "good-model" {
                (true, None)
            } else {
                (false, Some("unknown model".into()))
            }
        }
        async fn voice_status(&self) -> Value {
            json!({
                "available": true, "enabled": true, "state": "idle", "engine": "whisper",
                "model": "large-v3-turbo-q5_0", "language": Value::Null, "auto_submit": false,
                "min_words_to_submit": 3, "partial_transcripts": true, "segment_silence_ms": 800,
                "idle_timeout_ms": 30000, "max_session_minutes": 10,
                "models": [{"name":"large-v3-turbo-q5_0","bytes":574041195,"downloaded":true}],
            })
        }
        async fn set_voice_model(&self, model: String) -> Result<Value, String> {
            if model != "large-v3-turbo-q5_0" {
                return Err("unknown voice model".into());
            }
            Ok(self.voice_status().await)
        }
        async fn transcribe_voice(
            &self,
            samples: Vec<i16>,
            sample_rate: u32,
            events: tokio::sync::mpsc::UnboundedSender<crate::voice::VoiceEvent>,
            _abort: Arc<AtomicBool>,
        ) -> Result<String, String> {
            assert_eq!(sample_rate, 16_000);
            assert_eq!(samples, vec![1, -2]);
            let _ = events.send(crate::voice::VoiceEvent::State(
                crate::voice::VoiceState::Finalizing,
            ));
            Ok("local transcript".into())
        }
        async fn history(&self, thread_id: String) -> Vec<Value> {
            vec![json!({"role":"user","content":format!("past of {thread_id}")})]
        }
        async fn list_threads(&self) -> Result<Vec<Value>, String> {
            Ok(self.threads.lock().unwrap().clone())
        }
        async fn new_thread(&self) -> Result<String, String> {
            let thread_id = "t-new".to_string();
            *self.current_thread.lock().unwrap() = thread_id.clone();
            self.threads.lock().unwrap().insert(
                0,
                json!({"id":thread_id,"created":0,"updated":0,"title":"New chat","checkpoints":0}),
            );
            Ok(thread_id)
        }
        async fn switch_thread(&self, thread_id: String) -> Result<Vec<Value>, String> {
            if !self
                .threads
                .lock()
                .unwrap()
                .iter()
                .any(|thread| thread["id"].as_str() == Some(thread_id.as_str()))
            {
                return Err(format!("conversation not found: {thread_id}"));
            }
            *self.current_thread.lock().unwrap() = thread_id.clone();
            Ok(vec![
                json!({"role":"user","content":format!("opened {thread_id}")}),
            ])
        }
        async fn rename_thread(&self, thread_id: String, title: String) -> Result<(), String> {
            let mut threads = self.threads.lock().unwrap();
            let Some(thread) = threads
                .iter_mut()
                .find(|thread| thread["id"].as_str() == Some(thread_id.as_str()))
            else {
                return Err(format!("conversation not found: {thread_id}"));
            };
            thread["title"] = Value::String(title.trim().to_string());
            Ok(())
        }
        async fn delete_thread(&self, thread_id: String) -> Result<String, String> {
            self.threads
                .lock()
                .unwrap()
                .retain(|thread| thread["id"].as_str() != Some(thread_id.as_str()));
            if self.current_thread.lock().unwrap().as_str() == thread_id {
                *self.current_thread.lock().unwrap() = "t-main".into();
            }
            Ok(self.current_thread.lock().unwrap().clone())
        }
        async fn current_thread(&self) -> String {
            self.current_thread.lock().unwrap().clone()
        }
        async fn current_model(&self) -> String {
            "stub-model".into()
        }
    }

    async fn spawn_server() -> (String, Arc<StubDriver>, Arc<Mutex<Option<&'static str>>>) {
        let driver = Arc::new(StubDriver {
            prompts: Mutex::new(Vec::new()),
            current_thread: Mutex::new("t-main".into()),
            threads: Mutex::new(vec![
                json!({"id":"t-main","created":20,"updated":30,"title":"Main chat","checkpoints":2}),
                json!({"id":"t-old","created":10,"updated":11,"title":"Older chat","checkpoints":1}),
            ]),
        });
        // Per-router test source starts absent; never inspect or mutate a
        // real user environment value to run this protocol fixture.
        let proxy_fixture = Arc::new(Mutex::new(None));
        let app = router_from_state(AppState {
            driver: driver.clone(),
            started: Instant::now(),
            home_cache: Arc::new(Mutex::new(None)),
            proxy_fixture: Some(proxy_fixture.clone()),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("127.0.0.1:{}", addr.port()), driver, proxy_fixture)
    }

    async fn recv_json(
        ws: &mut tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    ) -> Value {
        loop {
            match ws.next().await.expect("socket open").expect("frame") {
                tokio_tungstenite::tungstenite::Message::Text(t) => {
                    return serde_json::from_str(&t).expect("json frame");
                }
                _ => continue,
            }
        }
    }

    #[test]
    fn pcm16_decoder_validates_shape_and_limit() {
        let encoded = base64::engine::general_purpose::STANDARD
            .encode([1_i16.to_le_bytes(), (-2_i16).to_le_bytes()].concat());
        assert_eq!(decode_pcm16(&encoded).unwrap(), vec![1, -2]);
        assert!(decode_pcm16("not-base64").is_err());
        assert!(decode_pcm16(&base64::engine::general_purpose::STANDARD.encode([1_u8])).is_err());
    }

    /// The whole protocol exchange AND the auth gate retain their original
    /// assertions, using a thread-safe per-router synthetic secret source.
    /// No process-global environment mutation can affect other test servers.
    #[tokio::test]
    async fn ws_protocol_v2_round_trip() {
        let (addr, driver, proxy_fixture) = spawn_server().await;

        // Liveness, unauthenticated.
        let health: Value = reqwest::get(format!("http://{addr}/healthz"))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(health["status"], "ok");

        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/chat"))
            .await
            .expect("ws connect");

        // hello arrives first, unprompted.
        let hello = recv_json(&mut ws).await;
        assert_eq!(hello["type"], "hello");
        assert_eq!(hello["protocol"], 2);
        assert_eq!(hello["model"], "stub-model");
        assert_eq!(hello["thread_id"], "t-main");
        assert_eq!(hello["a2ui"], true);
        assert_eq!(hello["capabilities"][0], "voice");

        // message → token, token, a2ui, done (in order).
        ws.send(tokio_tungstenite::tungstenite::Message::Text(
            json!({"type":"message","content":"hi there"})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
        assert_eq!(recv_json(&mut ws).await["text"], "Hel");
        assert_eq!(recv_json(&mut ws).await["text"], "lo");
        assert_eq!(recv_json(&mut ws).await["type"], "a2ui");
        let done = recv_json(&mut ws).await;
        assert_eq!(done["type"], "done");
        assert_eq!(
            done["thread_id"], "t-main",
            "defaults to the current thread"
        );
        assert_eq!(
            driver.prompts.lock().unwrap()[0],
            ("hi there".to_string(), "t-main".to_string())
        );

        // Turn failure → error frame, socket stays usable.
        ws.send(tokio_tungstenite::tungstenite::Message::Text(
            json!({"type":"message","content":"explode now"})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
        let err = recv_json(&mut ws).await;
        assert_eq!(err["type"], "error");
        assert_eq!(err["message"], "stub turn failure");

        // ui_action encodes as a [ui_action] turn with sorted keys.
        ws.send(tokio_tungstenite::tungstenite::Message::Text(
            json!({"type":"ui_action","name":"submit","surface_id":"s1",
                   "context":{"a":1},"thread_id":"t-42"})
            .to_string()
            .into(),
        ))
        .await
        .unwrap();
        // Drain that turn: 3 frames + done.
        for _ in 0..3 {
            recv_json(&mut ws).await;
        }
        assert_eq!(recv_json(&mut ws).await["thread_id"], "t-42");
        {
            let prompts = driver.prompts.lock().unwrap();
            let (prompt, tid) = &prompts[1];
            assert!(prompt.starts_with("[ui_action] {"), "got: {prompt}");
            assert!(prompt.contains("\"name\":\"submit\""));
            assert_eq!(tid, "t-42");
        }

        // set_model ack carries ok + message.
        ws.send(tokio_tungstenite::tungstenite::Message::Text(
            json!({"type":"set_model","model":"bad"}).to_string().into(),
        ))
        .await
        .unwrap();
        let ack = recv_json(&mut ws).await;
        assert_eq!(ack["type"], "model");
        assert_eq!(ack["ok"], false);
        assert_eq!(ack["message"], "unknown model");

        // history replays the requested thread.
        ws.send(tokio_tungstenite::tungstenite::Message::Text(
            json!({"type":"history","thread_id":"t-7"})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
        let hist = recv_json(&mut ws).await;
        assert_eq!(hist["type"], "history");
        assert_eq!(hist["messages"][0]["content"], "past of t-7");

        // Regular-chat conversation catalog.
        ws.send(tokio_tungstenite::tungstenite::Message::Text(
            json!({"type":"list_threads"}).to_string().into(),
        ))
        .await
        .unwrap();
        let threads = recv_json(&mut ws).await;
        assert_eq!(threads["type"], "threads");
        assert_eq!(threads["current_thread_id"], "t-main");
        assert_eq!(threads["threads"][0]["title"], "Main chat");

        // New chat returns the selected thread then a refreshed catalog.
        ws.send(tokio_tungstenite::tungstenite::Message::Text(
            json!({"type":"new_thread"}).to_string().into(),
        ))
        .await
        .unwrap();
        let created = recv_json(&mut ws).await;
        assert_eq!(created["type"], "thread");
        assert_eq!(created["action"], "created");
        assert_eq!(created["thread_id"], "t-new");
        assert_eq!(recv_json(&mut ws).await["type"], "threads");

        // Open, rename, and delete a persisted chat without any project/code
        // semantics in the workspace protocol.
        ws.send(tokio_tungstenite::tungstenite::Message::Text(
            json!({"type":"switch_thread","thread_id":"t-old"})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
        let opened = recv_json(&mut ws).await;
        assert_eq!(opened["type"], "history");
        assert_eq!(opened["thread_id"], "t-old");
        assert_eq!(opened["switched"], true);
        assert_eq!(recv_json(&mut ws).await["type"], "threads");

        ws.send(tokio_tungstenite::tungstenite::Message::Text(
            json!({"type":"rename_thread","thread_id":"t-old","title":"Renamed chat"})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
        let renamed = recv_json(&mut ws).await;
        assert_eq!(renamed["type"], "thread");
        assert_eq!(renamed["action"], "renamed");
        assert_eq!(renamed["title"], "Renamed chat");
        let renamed_threads = recv_json(&mut ws).await;
        assert!(
            renamed_threads["threads"]
                .as_array()
                .unwrap()
                .iter()
                .any(|thread| thread["title"] == "Renamed chat")
        );

        ws.send(tokio_tungstenite::tungstenite::Message::Text(
            json!({"type":"delete_thread","thread_id":"t-old"})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
        let deleted = recv_json(&mut ws).await;
        assert_eq!(deleted["type"], "thread");
        assert_eq!(deleted["action"], "deleted");
        assert_eq!(deleted["current_thread_id"], "t-main");
        let deleted_threads = recv_json(&mut ws).await;
        assert!(
            !deleted_threads["threads"]
                .as_array()
                .unwrap()
                .iter()
                .any(|thread| thread["id"] == "t-old")
        );

        // Workspace voice is served by the pod's local Whisper runtime.
        ws.send(tokio_tungstenite::tungstenite::Message::Text(
            json!({"type":"get_voice_status","operation_id":"voice-status"})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
        let voice_status = recv_json(&mut ws).await;
        assert_eq!(voice_status["type"], "voice_status");
        assert_eq!(voice_status["voice"]["model"], "large-v3-turbo-q5_0");

        let pcm = base64::engine::general_purpose::STANDARD
            .encode([1_i16.to_le_bytes(), (-2_i16).to_le_bytes()].concat());
        ws.send(tokio_tungstenite::tungstenite::Message::Text(
            json!({"type":"voice_transcribe","operation_id":"voice-1","sample_rate":16000,"audio":pcm}).to_string().into(),
        ))
        .await
        .unwrap();
        let voice_state = recv_json(&mut ws).await;
        assert_eq!(voice_state["event"], "state");
        assert_eq!(voice_state["state"], "finalizing");
        let transcript = recv_json(&mut ws).await;
        assert_eq!(transcript["event"], "segment_final");
        assert_eq!(transcript["operation_id"], "voice-1");
        assert_eq!(transcript["text"], "local transcript");
        assert_eq!(recv_json(&mut ws).await["type"], "voice_status");

        // Garbage JSON → error frame, not a dropped socket.
        ws.send(tokio_tungstenite::tungstenite::Message::Text(
            "{nope".into(),
        ))
        .await
        .unwrap();
        assert_eq!(recv_json(&mut ws).await["type"], "error");
        drop(ws);

        // Same actual auth gate/HTTP/WS assertions; only this router's
        // synthetic source changes. No global environment write.
        *proxy_fixture.lock().unwrap() = Some("s3cret");
        let client = reqwest::Client::new();
        // /stats: no header and a wrong header are both rejected BEFORE any
        // work (notably before the home-dir walk — the 200 path is exercised
        // per-piece by the disk/walk unit tests; walking a real dev home here
        // would cost a minute).
        let denied = client
            .get(format!("http://{addr}/stats"))
            .send()
            .await
            .unwrap();
        assert_eq!(denied.status(), 403);
        let wrong = client
            .get(format!("http://{addr}/stats"))
            .header(PROXY_HEADER, "nope")
            .send()
            .await
            .unwrap();
        assert_eq!(wrong.status(), 403);

        // WS without the header: rejected before the upgrade (HTTP 403).
        let ws_denied = tokio_tungstenite::connect_async(format!("ws://{addr}/chat")).await;
        // WS with the right header: upgrades and greets.
        let req = tokio_tungstenite::tungstenite::client::IntoClientRequest::into_client_request(
            format!("ws://{addr}/chat"),
        )
        .map(|mut r| {
            r.headers_mut()
                .insert(PROXY_HEADER, "s3cret".parse().unwrap());
            r
        })
        .unwrap();
        let ws_ok = tokio_tungstenite::connect_async(req).await;
        *proxy_fixture.lock().unwrap() = None;

        match ws_denied {
            Err(tokio_tungstenite::tungstenite::Error::Http(resp)) => {
                assert_eq!(resp.status(), 403);
            }
            other => panic!("expected HTTP 403 rejection, got {other:?}"),
        }
        let (mut ws2, _) = ws_ok.expect("authorized ws connects");
        assert_eq!(recv_json(&mut ws2).await["type"], "hello");
    }

    #[test]
    fn supplied_secret_auth_preserves_empty_header_lossy_and_constant_time_behavior() {
        let mut headers = HeaderMap::new();
        assert!(authorized_from_secret(&headers, None));
        assert!(authorized_from_secret(&headers, Some("".into())));
        assert!(!authorized_from_secret(&headers, Some("s3cret".into())));
        headers.insert(PROXY_HEADER, "wrong".parse().unwrap());
        assert!(!authorized_from_secret(&headers, Some("s3cret".into())));
        headers.insert(PROXY_HEADER, "s3cret".parse().unwrap());
        assert!(authorized_from_secret(&headers, Some("s3cret".into())));
        headers.insert(
            PROXY_HEADER,
            axum::http::HeaderValue::from_bytes(&[255]).unwrap(),
        );
        assert!(!authorized_from_secret(&headers, Some("s3cret".into())));
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            assert!(!authorized_from_secret(
                &headers,
                Some(std::ffi::OsString::from_vec(vec![255]))
            ));
        }
    }

    #[tokio::test]
    async fn actual_router_auth_sources_are_independent_and_never_mutate_process_environment() {
        let (one, _driver_one, secret_one) = spawn_server().await;
        let (two, _driver_two, secret_two) = spawn_server().await;
        *secret_one.lock().unwrap() = Some("fixture-one");
        *secret_two.lock().unwrap() = Some("fixture-two");
        let request = |address: &str, value: &str| {
            use tokio_tungstenite::tungstenite::client::IntoClientRequest;
            let mut request = format!("ws://{address}/chat")
                .into_client_request()
                .unwrap();
            request
                .headers_mut()
                .insert(PROXY_HEADER, value.parse().unwrap());
            request
        };
        let (mut accepted, _) = tokio_tungstenite::connect_async(request(&one, "fixture-one"))
            .await
            .unwrap();
        assert_eq!(recv_json(&mut accepted).await["type"], "hello");
        drop(accepted);
        let refused = tokio_tungstenite::connect_async(request(&two, "fixture-one")).await;
        assert!(
            matches!(refused, Err(tokio_tungstenite::tungstenite::Error::Http(response)) if response.status() == 403)
        );
        let (mut accepted, _) = tokio_tungstenite::connect_async(request(&two, "fixture-two"))
            .await
            .unwrap();
        assert_eq!(recv_json(&mut accepted).await["type"], "hello");
        drop(accepted);
        *secret_one.lock().unwrap() = None;
        let (mut accepted, _) = tokio_tungstenite::connect_async(format!("ws://{one}/chat"))
            .await
            .unwrap();
        assert_eq!(recv_json(&mut accepted).await["type"], "hello");
        drop(accepted);
        let still_refused = tokio_tungstenite::connect_async(format!("ws://{two}/chat")).await;
        assert!(
            matches!(still_refused, Err(tokio_tungstenite::tungstenite::Error::Http(response)) if response.status() == 403)
        );
    }
}

#[cfg(test)]
pub(crate) fn proxy_environment_fixture_for_tests(secret_present: bool) {
    let mut headers = HeaderMap::new();
    assert_eq!(authorized(&headers), !secret_present);
    headers.insert(PROXY_HEADER, "nope".parse().unwrap());
    assert_eq!(authorized(&headers), !secret_present);
    headers.insert(PROXY_HEADER, "s3cret".parse().unwrap());
    assert!(authorized(&headers));
}
