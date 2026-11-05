//! `craftsman desktop-serve` — local desktop bridge.
//!
//! The existing `serve` command intentionally auto-approves tools because it
//! runs inside a network-isolated workspace pod. The desktop process runs with
//! the user's host permissions, so it needs the same HITL and `ask_user`
//! behavior as the Ratatui frontend. This module is a separate protocol
//! adapter: the agent, tools, filesystem, persistence, MCP, and A2UI runtime
//! remain the shared implementations in `agent.rs`.

use crate::a2ui::{FrameTx, RenderUiTool, SYSTEM_PROMPT_SUFFIX};
use crate::agent::{self, ApprovalPromptResult, Session, SessionExtras, ToolActivityEvent};
use crate::config::Config;
use crate::hitl::ApprovalState;
use async_trait::async_trait;
use axum::Router;
use axum::extract::ws::{Message as WsMessage, WebSocket};
use axum::extract::{Query, State, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::get;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc as std_mpsc};

const PROTOCOL_VERSION: u64 = 4;
const PROXY_HEADER: &str = "x-craftsman-workspace-proxy";
const PROXY_SECRET_ENV: &str = "CRAFTSMAN_WORKSPACE_PROXY_SECRET";
const DESKTOP_TOKEN_ENV: &str = "CRAFTSMAN_DESKTOP_TOKEN";
const DEFAULT_THREAD_PAGE_SIZE: usize = 10;
const MAX_THREAD_PAGE_SIZE: usize = 100;
const CONNECTION_HANDOFF: std::time::Duration = std::time::Duration::from_millis(250);
const WRITER_SHUTDOWN: std::time::Duration = std::time::Duration::from_secs(1);

struct ThreadPage {
    threads: Vec<Value>,
    next_offset: Option<usize>,
}

#[derive(Clone)]
struct TurnContext {
    turn_id: String,
    thread_id: String,
    connection_generation: u64,
}

struct BridgeConnection {
    generation: u64,
    tx: FrameTx,
}

struct PendingApproval {
    waiter: std_mpsc::Sender<ApprovalPromptResult>,
    tool: String,
    summary: String,
    details: String,
    turn: Option<TurnContext>,
}

struct PendingQuestion {
    waiter: std_mpsc::Sender<Option<String>>,
    question: String,
    turn: Option<TurnContext>,
}

struct WorkspaceSwitchGuard {
    bridge: Arc<DesktopBridge>,
}

impl Drop for WorkspaceSwitchGuard {
    fn drop(&mut self) {
        self.bridge.finish_workspace_switch();
    }
}

/// Connection-scoped request/response bridge captured by synchronous agent UI
/// callbacks. The WebSocket reader resolves waiters while the agent task is
/// blocked, the same channel design as the Ratatui `UiBridge`.
struct DesktopBridge {
    slot: Mutex<Option<BridgeConnection>>,
    connection_owner: Arc<tokio::sync::Mutex<()>>,
    next_connection_generation: AtomicU64,
    approvals: Mutex<HashMap<String, PendingApproval>>,
    questions: Mutex<HashMap<String, PendingQuestion>>,
    active_turn: Mutex<Option<TurnContext>>,
    next_request: AtomicU64,
    next_turn: AtomicU64,
    workspace_switching: std::sync::atomic::AtomicBool,
}

impl DesktopBridge {
    fn new() -> Self {
        Self {
            slot: Mutex::new(None),
            connection_owner: Arc::new(tokio::sync::Mutex::new(())),
            next_connection_generation: AtomicU64::new(1),
            approvals: Mutex::new(HashMap::new()),
            questions: Mutex::new(HashMap::new()),
            active_turn: Mutex::new(None),
            next_request: AtomicU64::new(1),
            next_turn: AtomicU64::new(1),
            workspace_switching: std::sync::atomic::AtomicBool::new(false),
        }
    }

    fn request_id(&self, prefix: &str) -> String {
        let sequence = self.next_request.fetch_add(1, Ordering::Relaxed);
        format!("{prefix}-{sequence}")
    }

    fn current_turn(&self) -> Option<TurnContext> {
        self.active_turn.lock().unwrap().clone()
    }

    fn begin_turn(&self, thread_id: String, connection_generation: u64) -> Option<TurnContext> {
        if self.workspace_switching.load(Ordering::SeqCst) {
            return None;
        }
        let mut active = self.active_turn.lock().unwrap();
        if active.is_some() || self.workspace_switching.load(Ordering::SeqCst) {
            return None;
        }
        let turn = TurnContext {
            turn_id: format!("turn-{}", self.next_turn.fetch_add(1, Ordering::Relaxed)),
            thread_id,
            connection_generation,
        };
        *active = Some(turn.clone());
        Some(turn)
    }

    fn finish_turn(&self, turn_id: &str) {
        let mut active = self.active_turn.lock().unwrap();
        if active.as_ref().is_some_and(|turn| turn.turn_id == turn_id) {
            *active = None;
        }
    }

    fn send(&self, frame: Value) -> bool {
        if matches!(
            frame["type"].as_str(),
            Some(
                "token"
                    | "activity"
                    | "permission_request"
                    | "ask_user"
                    | "tool_started"
                    | "tool_finished"
                    | "a2ui"
                    | "done"
                    | "turn_error"
            )
        ) {
            let Some(turn) = self.current_turn() else {
                return false;
            };
            return self.send_for_turn(&turn, frame);
        }
        self.slot
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|connection| connection.tx.send(frame).is_ok())
    }

    fn send_for_turn(&self, turn: &TurnContext, mut frame: Value) -> bool {
        if let Some(object) = frame.as_object_mut() {
            object
                .entry("turn_id")
                .or_insert_with(|| Value::String(turn.turn_id.clone()));
            object
                .entry("thread_id")
                .or_insert_with(|| Value::String(turn.thread_id.clone()));
        }
        self.send_for_generation(turn.connection_generation, frame)
    }

    fn send_for_generation(&self, generation: u64, frame: Value) -> bool {
        self.slot
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|connection| {
                connection.generation == generation && connection.tx.send(frame).is_ok()
            })
    }

    fn attach(&self, tx: FrameTx) -> Option<u64> {
        let mut slot = self.slot.lock().unwrap();
        if slot.is_some() {
            return None;
        }
        let generation = self
            .next_connection_generation
            .fetch_add(1, Ordering::SeqCst);
        *slot = Some(BridgeConnection { generation, tx });
        Some(generation)
    }

    fn detach(&self, generation: u64) -> bool {
        let detached = {
            let mut slot = self.slot.lock().unwrap();
            if slot
                .as_ref()
                .is_none_or(|connection| connection.generation != generation)
            {
                false
            } else {
                *slot = None;
                true
            }
        };
        if detached {
            self.cancel_pending();
        }
        detached
    }

    fn request_approval(&self, tool: &str, summary: &str, details: &str) -> ApprovalPromptResult {
        let request_id = self.request_id("permission");
        let (tx, rx) = std_mpsc::channel();
        self.approvals.lock().unwrap().insert(
            request_id.clone(),
            PendingApproval {
                waiter: tx,
                tool: tool.to_string(),
                summary: summary.to_string(),
                details: details.to_string(),
                turn: self.current_turn(),
            },
        );
        if !self.send(json!({
            "type": "permission_request",
            "request_id": request_id,
            "tool": tool,
            "summary": summary,
            "details": details,
            "approve_always_scope": "desktop_session",
        })) {
            self.approvals.lock().unwrap().remove(&request_id);
            return ApprovalPromptResult::Reject(String::new());
        }
        tokio::task::block_in_place(|| rx.recv())
            .unwrap_or_else(|_| ApprovalPromptResult::Reject(String::new()))
    }

    fn request_answer(&self, question: &str) -> Option<String> {
        let request_id = self.request_id("question");
        let (tx, rx) = std_mpsc::channel();
        self.questions.lock().unwrap().insert(
            request_id.clone(),
            PendingQuestion {
                waiter: tx,
                question: question.to_string(),
                turn: self.current_turn(),
            },
        );
        if !self.send(json!({
            "type": "ask_user",
            "request_id": request_id,
            "question": question,
        })) {
            self.questions.lock().unwrap().remove(&request_id);
            return None;
        }
        tokio::task::block_in_place(|| rx.recv()).ok().flatten()
    }

    fn resolve_approval(&self, request_id: &str, decision: &str, feedback: Option<&str>) -> bool {
        let result = match decision {
            "approve_once" => ApprovalPromptResult::Approve,
            "approve_always" => ApprovalPromptResult::ApproveAlways,
            "reject" => ApprovalPromptResult::Reject(feedback.unwrap_or_default().to_string()),
            _ => return false,
        };
        self.approvals
            .lock()
            .unwrap()
            .remove(request_id)
            .is_some_and(|pending| pending.waiter.send(result).is_ok())
    }

    fn resolve_question(&self, request_id: &str, answer: Option<String>) -> bool {
        self.questions
            .lock()
            .unwrap()
            .remove(request_id)
            .is_some_and(|pending| pending.waiter.send(answer).is_ok())
    }

    fn pending_snapshot(&self) -> Vec<Value> {
        let mut pending = Vec::new();
        for (request_id, item) in self.approvals.lock().unwrap().iter() {
            pending.push(json!({
                "kind": "permission",
                "request_id": request_id,
                "tool": item.tool,
                "summary": item.summary,
                "details": item.details,
                "turn_id": item.turn.as_ref().map(|turn| turn.turn_id.as_str()),
                "thread_id": item.turn.as_ref().map(|turn| turn.thread_id.as_str()),
                "approve_always_scope": "desktop_session",
            }));
        }
        for (request_id, item) in self.questions.lock().unwrap().iter() {
            pending.push(json!({
                "kind": "question",
                "request_id": request_id,
                "question": item.question,
                "turn_id": item.turn.as_ref().map(|turn| turn.turn_id.as_str()),
                "thread_id": item.turn.as_ref().map(|turn| turn.thread_id.as_str()),
            }));
        }
        pending
    }

    fn has_pending_interactions(&self) -> bool {
        !self.approvals.lock().unwrap().is_empty() || !self.questions.lock().unwrap().is_empty()
    }

    fn begin_workspace_switch(&self) -> bool {
        if self
            .workspace_switching
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return false;
        }
        if self.current_turn().is_some() || self.has_pending_interactions() {
            self.workspace_switching.store(false, Ordering::SeqCst);
            return false;
        }
        true
    }

    fn finish_workspace_switch(&self) {
        self.workspace_switching.store(false, Ordering::SeqCst);
    }

    /// A disconnected desktop must never leave a host-mutating tool blocked.
    /// Resolve every permission to deny and every question to no-answer.
    fn cancel_pending(&self) {
        for (_, pending) in self.approvals.lock().unwrap().drain() {
            let _ = pending
                .waiter
                .send(ApprovalPromptResult::Reject(String::new()));
        }
        for (_, pending) in self.questions.lock().unwrap().drain() {
            let _ = pending.waiter.send(None);
        }
    }
}

struct DesktopRunner {
    inner: tokio::sync::Mutex<RunnerInner>,
    approval: Arc<Mutex<ApprovalState>>,
    bridge: Arc<DesktopBridge>,
    agent_handle: Mutex<Option<craftsman_harness::Agent>>,
    registry_handle: Mutex<Option<Arc<craftsman_harness::AgentRegistry>>>,
    voice: Arc<DesktopVoiceRuntime>,
    initialized: AtomicBool,
    initialized_notify: tokio::sync::Notify,
}

struct DesktopVoiceRuntime {
    bridge: Arc<DesktopBridge>,
    config: Mutex<crate::voice::VoiceConfig>,
    state: Mutex<crate::voice::VoiceState>,
    controller: Mutex<Option<crate::voice::VoiceController>>,
    connection_generation: AtomicU64,
}

impl DesktopVoiceRuntime {
    fn new(cfg: &Config, bridge: Arc<DesktopBridge>) -> Arc<Self> {
        Arc::new(Self {
            bridge,
            config: Mutex::new(crate::voice::VoiceConfig::from_config(cfg)),
            state: Mutex::new(crate::voice::VoiceState::Idle),
            controller: Mutex::new(None),
            connection_generation: AtomicU64::new(0),
        })
    }

    fn active(&self) -> bool {
        matches!(
            *self.state.lock().unwrap(),
            crate::voice::VoiceState::Initializing
                | crate::voice::VoiceState::Downloading
                | crate::voice::VoiceState::Recording
                | crate::voice::VoiceState::Finalizing
        )
    }

    fn status(&self) -> Value {
        let config = self.config.lock().unwrap().clone();
        let state = *self.state.lock().unwrap();
        json!({
            "available": crate::voice::available(),
            "enabled": config.enabled,
            "state": voice_state_name(state),
            "engine": config.engine,
            "model": config.model,
            "language": config.language,
            "auto_submit": config.auto_submit,
            "min_words_to_submit": config.min_words_to_submit,
            "partial_transcripts": config.partial_transcripts,
            "vad_threshold": config.vad_threshold,
            "segment_silence_ms": config.vad_silence_ms,
            "idle_timeout_ms": config.idle_timeout_ms,
            "max_session_minutes": config.max_session_minutes,
            "models": crate::voice::model_specs().map(|model| json!({
                "name": model.name,
                "bytes": model.bytes,
                "downloaded": crate::voice::model_downloaded(model.name),
            })).collect::<Vec<_>>(),
        })
    }

    fn start(self: &Arc<Self>, connection_generation: u64) -> Result<(), String> {
        if !crate::voice::available() {
            return Err("voice input is unavailable on this platform or build".into());
        }
        if !self.config.lock().unwrap().enabled {
            return Err("voice input is disabled in Craftsman configuration".into());
        }
        if self.active() {
            return Err("voice input is already active".into());
        }
        self.connection_generation
            .store(connection_generation, Ordering::SeqCst);
        *self.state.lock().unwrap() = crate::voice::VoiceState::Initializing;
        let mut controller = self.controller.lock().unwrap();
        if controller.is_none() {
            let (events, mut incoming) = tokio::sync::mpsc::unbounded_channel();
            *controller = Some(crate::voice::VoiceController::spawn(
                self.config.lock().unwrap().clone(),
                events,
            ));
            let runtime = self.clone();
            tokio::spawn(async move {
                while let Some(event) = incoming.recv().await {
                    runtime.forward_event(event);
                }
            });
        }
        controller
            .as_ref()
            .expect("voice controller created")
            .start();
        Ok(())
    }

    fn stop_and_finalize(&self) -> bool {
        self.controller
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|controller| {
                controller.stop_and_finalize();
                true
            })
    }

    fn cancel_segment(&self) -> bool {
        self.controller
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|controller| {
                controller.cancel_segment();
                true
            })
    }

    fn cancel(&self) -> bool {
        self.connection_generation.store(0, Ordering::SeqCst);
        self.controller
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|controller| {
                controller.cancel();
                *self.state.lock().unwrap() = crate::voice::VoiceState::Idle;
                true
            })
    }

    async fn reconfigure(&self, config: crate::voice::VoiceConfig) {
        self.connection_generation.store(0, Ordering::SeqCst);
        let controller = self.controller.lock().unwrap().take();
        if let Some(controller) = controller {
            controller.shutdown().await;
        }
        *self.config.lock().unwrap() = config;
        *self.state.lock().unwrap() = crate::voice::VoiceState::Idle;
    }

    fn forward_event(&self, event: crate::voice::VoiceEvent) {
        if let crate::voice::VoiceEvent::State(state) = &event {
            *self.state.lock().unwrap() = *state;
        }
        let generation = self.connection_generation.load(Ordering::SeqCst);
        if generation == 0 {
            return;
        }
        let model = self.config.lock().unwrap().model.clone();
        let frame = voice_event_frame(event, &model);
        let idle = frame["event"] == "state" && frame["state"] == "idle";
        let _ = self.bridge.send_for_generation(generation, frame);
        if idle {
            self.connection_generation.store(0, Ordering::SeqCst);
        }
    }
}

fn voice_state_name(state: crate::voice::VoiceState) -> &'static str {
    match state {
        crate::voice::VoiceState::Idle => "idle",
        crate::voice::VoiceState::Initializing => "initializing",
        crate::voice::VoiceState::Downloading => "downloading",
        crate::voice::VoiceState::Recording => "recording",
        crate::voice::VoiceState::Finalizing => "finalizing",
        crate::voice::VoiceState::Paused => "paused",
    }
}

fn voice_event_frame(event: crate::voice::VoiceEvent, model: &str) -> Value {
    match event {
        crate::voice::VoiceEvent::State(state) => {
            json!({"type":"voice_event", "event":"state", "state":voice_state_name(state)})
        }
        crate::voice::VoiceEvent::DownloadProgress { downloaded, total } => {
            json!({"type":"voice_event", "event":"download_progress", "downloaded":downloaded, "total":total})
        }
        crate::voice::VoiceEvent::Level(level) => {
            json!({"type":"voice_event", "event":"level", "level":level})
        }
        crate::voice::VoiceEvent::SpeechStarted => {
            json!({"type":"voice_event", "event":"speech_started"})
        }
        crate::voice::VoiceEvent::Partial {
            committed,
            tentative,
        } => {
            json!({"type":"voice_event", "event":"partial", "committed":committed, "tentative":tentative})
        }
        crate::voice::VoiceEvent::SegmentFinal {
            text,
            committed,
            partials_enabled,
        } => {
            json!({"type":"voice_event", "event":"segment_final", "text":text, "committed":committed, "partials_enabled":partials_enabled})
        }
        crate::voice::VoiceEvent::SegmentCancelled => {
            json!({"type":"voice_event", "event":"segment_cancelled"})
        }
        crate::voice::VoiceEvent::SessionFinished { dictated_text } => {
            json!({"type":"voice_event", "event":"session_finished", "dictated_text":dictated_text})
        }
        crate::voice::VoiceEvent::Error(error) => {
            let code = match &error {
                crate::voice::VoiceError::MicPermissionDenied => "microphone_permission_denied",
                crate::voice::VoiceError::NoAudioCaptured => "no_audio_captured",
                crate::voice::VoiceError::NoSpeechDetected => "no_speech_detected",
                crate::voice::VoiceError::EngineFailed(_) => "voice_engine_failed",
            };
            let message = match &error {
                crate::voice::VoiceError::MicPermissionDenied => "Microphone access was denied. Allow Craftsman in System Settings → Privacy & Security → Microphone, then try again.".to_string(),
                _ => error.user_message(model),
            };
            json!({"type":"voice_event", "event":"error", "code":code, "message":message})
        }
    }
}

#[async_trait]
trait DesktopDriver: Send + Sync {
    async fn ready(&self) -> bool;
    async fn current_thread(&self) -> String;
    async fn current_model(&self) -> String;
    async fn current_workspace(&self) -> String {
        std::env::current_dir()
            .map(|path| path.display().to_string())
            .unwrap_or_default()
    }
    async fn switch_workspace(
        &self,
        _workspace: String,
        _thread_id: Option<String>,
    ) -> Result<Value, String> {
        Err("workspace switching is unavailable".into())
    }
    async fn run_turn(&self, prompt: String, thread_id: String) -> Result<(), String>;
    async fn set_model(&self, model: String) -> (bool, Option<String>);
    async fn history(&self, thread_id: String) -> Vec<Value>;
    async fn list_threads(&self, offset: usize, limit: usize) -> Result<ThreadPage, String>;
    async fn new_thread(&self) -> Result<String, String>;
    async fn delete_thread(&self, thread_id: String) -> Result<String, String>;
    async fn list_models(&self) -> Result<Vec<String>, String>;
    fn steer(&self, message: String) -> bool;
    fn cancel(&self) -> bool;
    fn voice_status(&self) -> Value {
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
    fn start_voice(&self, _connection_generation: u64) -> Result<(), String> {
        Err("voice input is unavailable".into())
    }
    fn stop_voice(&self) -> bool {
        false
    }
    fn cancel_voice_segment(&self) -> bool {
        false
    }
    fn cancel_voice(&self) -> bool {
        false
    }
    async fn set_voice_model(&self, _model: String) -> Result<Value, String> {
        Err("voice model selection is unavailable".into())
    }
    async fn set_voice_enabled(&self, _enabled: bool) -> Result<Value, String> {
        Err("voice configuration is unavailable".into())
    }

    async fn state_snapshot(&self) -> Value {
        json!({
            "configured": self.ready().await,
            "agent_ready": self.ready().await,
            "workspace": self.current_workspace().await,
            "project_contexts": [],
            "thread_id": self.current_thread().await,
            "model": self.current_model().await,
            "permission_mode": "manual",
            "harness_profile": "auto",
            "platform_configured": false,
            "provider": "unknown",
        })
    }
    async fn configure(&self, _request: Value) -> Result<Value, String> {
        Err("configuration is unavailable".into())
    }
    async fn logout(&self) -> Result<Value, String> {
        Err("logout is unavailable".into())
    }
    async fn switch_thread(&self, thread_id: String) -> Result<Vec<Value>, String> {
        Ok(self.history(thread_id).await)
    }
    async fn set_thread_contexts(
        &self,
        _thread_id: String,
        _project_contexts: Vec<String>,
    ) -> Result<Vec<String>, String> {
        Err("conversation project context is unavailable".into())
    }
    async fn rename_thread(&self, _thread_id: String, _title: String) -> Result<(), String> {
        Err("conversation rename is unavailable".into())
    }
    async fn export_thread(&self, _thread_id: String) -> Result<String, String> {
        Err("conversation export is unavailable".into())
    }
    async fn duplicate_thread(&self, _thread_id: String, _branch: bool) -> Result<String, String> {
        Err("conversation duplication is unavailable".into())
    }
    async fn workspace_diff(&self) -> Result<String, String> {
        tokio::task::spawn_blocking(crate::workspace_diff::workspace_diff)
            .await
            .map_err(|error| error.to_string())
    }
    async fn list_checkpoints(&self) -> Result<Vec<Value>, String> {
        Err("workspace checkpointing is unavailable".into())
    }
    async fn rewind_checkpoint(
        &self,
        _checkpoint_id: Option<String>,
        _force: bool,
    ) -> Result<Value, String> {
        Err("workspace checkpointing is unavailable".into())
    }
    async fn task_list(&self) -> Result<Vec<Value>, String> {
        Ok(Vec::new())
    }
    fn cancel_task(&self, _name: &str) -> bool {
        false
    }
    fn message_task(&self, _name: &str, _message: String) -> bool {
        false
    }
    async fn token_usage(&self) -> Result<Value, String> {
        Ok(json!({"input": 0, "output": 0, "cost_usd": Value::Null}))
    }
    async fn diagnostics(&self) -> Value {
        json!({"agent_ready": self.ready().await})
    }
}

struct RunnerInner {
    cfg: Config,
    session: Option<Session>,
    pending_thread_id: Option<String>,
    general_workspace: PathBuf,
    workspace: PathBuf,
    project_contexts: Vec<PathBuf>,
}

fn canonical_workspace(path: &str) -> Result<PathBuf, String> {
    let path = Path::new(path.trim());
    if path.as_os_str().is_empty() {
        return Err("workspace must be a non-empty directory path".into());
    }
    if !path.is_dir() {
        return Err(format!("workspace is not a directory: {}", path.display()));
    }
    path.canonicalize()
        .map_err(|error| format!("could not resolve workspace {}: {error}", path.display()))
}

fn canonical_project_contexts(paths: Vec<String>) -> Result<Vec<PathBuf>, String> {
    let mut contexts = Vec::new();
    for path in paths {
        let context = canonical_workspace(&path)?;
        if !contexts.contains(&context) {
            contexts.push(context);
        }
    }
    Ok(contexts)
}

fn existing_project_contexts(paths: &[String]) -> Result<Vec<PathBuf>, String> {
    canonical_project_contexts(paths.to_vec())
}

fn legacy_workspace_context(workspace: &Path) -> Vec<PathBuf> {
    let normalized = workspace.to_string_lossy().replace('\\', "/");
    if normalized
        .trim_end_matches('/')
        .ends_with("/.craftsman/general-chat")
    {
        Vec::new()
    } else {
        vec![workspace.to_path_buf()]
    }
}

fn desktop_configuration_ready(cfg: &Config) -> bool {
    cfg.model.is_some() && (!cfg.base_url.is_empty() || cfg.platform_configured())
}

fn desktop_provider_configured(cfg: &Config) -> bool {
    (!cfg.base_url.is_empty() && !cfg.api_key.is_empty()) || cfg.platform_configured()
}

/// The platform's OpenAI-compatible LLM surface — the documented base URL
/// platform deployments set in `base_url` (see
/// `docs/platform-token-refactor.md`).
fn platform_llm_base_url(platform_base: &str) -> String {
    format!("{}/apps/llm/api/v1", platform_base.trim_end_matches('/'))
}

fn apply_voice_model(cfg: &mut Config, model: &str) -> Result<crate::voice::VoiceConfig, String> {
    let model = crate::voice::canonical_model_name(model)
        .ok_or_else(|| format!("unknown voice model: {model}"))?;
    cfg.set_voice_setting("model", serde_json::json!(model));
    Ok(crate::voice::VoiceConfig::from_config(cfg))
}

fn apply_voice_enabled(cfg: &mut Config, enabled: bool) -> crate::voice::VoiceConfig {
    cfg.set_voice_setting("enabled", serde_json::json!(enabled));
    crate::voice::VoiceConfig::from_config(cfg)
}

fn apply_configuration_model(
    cfg: &mut Config,
    route: &str,
    requested: Option<&str>,
    models: &[String],
) -> Result<(), String> {
    let requested = requested.map(str::trim).filter(|value| !value.is_empty());
    if route != "platform" {
        let model = requested.ok_or_else(|| "model is required".to_string())?;
        cfg.model = Some(model.to_string());
        return Ok(());
    }

    let selected = [
        requested.map(str::to_string),
        cfg.model.clone(),
        cfg.default_model.clone(),
    ]
    .into_iter()
    .flatten()
    .find(|candidate| !candidate.trim().is_empty() && models.iter().any(|model| model == candidate))
    .or_else(|| {
        models
            .iter()
            .find(|model| !model.trim().is_empty())
            .cloned()
    })
    .ok_or_else(|| "the Craftsman platform returned no available chat models".to_string())?;
    cfg.model = Some(selected);
    Ok(())
}

fn apply_startup_model(cfg: &mut Config, models: &[String]) -> Result<(), String> {
    if cfg
        .model
        .as_deref()
        .is_some_and(|model| !model.trim().is_empty())
    {
        return Ok(());
    }
    let selected = cfg
        .default_model
        .as_ref()
        .filter(|model| models.iter().any(|candidate| candidate == *model))
        .cloned()
        .or_else(|| {
            models
                .iter()
                .find(|model| !model.trim().is_empty())
                .cloned()
        })
        .ok_or_else(|| "the configured provider returned no available chat models".to_string())?;
    cfg.model = Some(selected);
    Ok(())
}

fn apply_provider_route(
    cfg: &mut Config,
    route: &str,
    platform_url: Option<&str>,
    base_url: Option<&str>,
    api_key: Option<&str>,
    api_token: Option<&str>,
) -> Result<(), String> {
    match route {
        "platform" => {
            let url = platform_url
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| "platform URL is required".to_string())?;
            let normalized = crate::platform_rest::normalize_platform_url(url)
                .map_err(|error| format!("invalid platform URL: {error}"))?;
            // The personal API token arrives as `api_token` (the desktop
            // app's token field); `api_key` is accepted as a legacy alias.
            let token = api_token
                .or(api_key)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| "platform API token is required".to_string())?;
            if cfg.platform.base_url != normalized {
                cfg.platform.api_token.clear();
            }
            cfg.platform.base_url = normalized.clone();
            cfg.platform.api_token = token.to_string();
            // The platform's LLM app is a stock OpenAI-compatible provider:
            // derive the documented endpoint so chat works without any
            // further platform-specific code.
            cfg.base_url = platform_llm_base_url(&normalized);
            cfg.api_key = token.to_string();
        }
        "direct" => {
            let url = base_url
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| "direct provider base URL is required".to_string())?;
            cfg.base_url = url.trim_end_matches('/').to_string();
            if let Some(key) = api_key.filter(|value| !value.is_empty()) {
                cfg.api_key = key.to_string();
            }
            cfg.platform = crate::config::PlatformConfig::default();
        }
        _ => return Err("route must be platform or direct".into()),
    }
    Ok(())
}

impl DesktopRunner {
    fn new(cfg: Config, bridge: Arc<DesktopBridge>, workspace: PathBuf) -> Self {
        let approval = Arc::new(Mutex::new(
            ApprovalState {
                mode: if cfg.auto_approve {
                    crate::config::PermissionMode::BypassPermissions
                } else {
                    cfg.permission_mode
                },
                auto_approve: cfg.auto_approve,
                auto_approve_network: cfg.auto_approve || cfg.auto_approve_web(),
                shell_allow_list: cfg.shell_allow_list.clone(),
            }
            .with_env_allow_list(),
        ));
        let voice = DesktopVoiceRuntime::new(&cfg, bridge.clone());
        Self {
            inner: tokio::sync::Mutex::new(RunnerInner {
                cfg,
                session: None,
                pending_thread_id: Some(crate::checkpoints::LibsqlCheckpointer::new_thread_id()),
                general_workspace: workspace.clone(),
                workspace,
                project_contexts: Vec::new(),
            }),
            approval,
            bridge,
            agent_handle: Mutex::new(None),
            registry_handle: Mutex::new(None),
            voice,
            initialized: AtomicBool::new(false),
            initialized_notify: tokio::sync::Notify::new(),
        }
    }

    async fn initialize(&self) {
        // The desktop bridge is headless and immediately usable. Provider
        // discovery and the heavyweight workspace runtime are intentionally
        // deferred until a real chat turn needs them.
        self.initialized.store(true, Ordering::SeqCst);
        self.initialized_notify.notify_waiters();
        let _ = self.bridge.send(json!({
            "type": "state_snapshot",
            "operation_id": Value::Null,
            "state": self.state_snapshot().await,
        }));
    }

    async fn wait_until_initialized(&self) {
        while !self.initialized.load(Ordering::SeqCst) {
            let notified = self.initialized_notify.notified();
            if self.initialized.load(Ordering::SeqCst) {
                break;
            }
            notified.await;
        }
    }

    async fn ready(&self) -> bool {
        self.inner.lock().await.session.is_some()
    }

    async fn current_thread(&self) -> String {
        let inner = self.inner.lock().await;
        inner
            .session
            .as_ref()
            .map(|session| session.thread_id.clone())
            .or_else(|| inner.pending_thread_id.clone())
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

    async fn current_workspace(&self) -> String {
        self.inner.lock().await.workspace.display().to_string()
    }

    fn has_live_background_tasks(&self) -> bool {
        self.registry_handle
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|registry| {
                registry.details().iter().any(|task| {
                    matches!(
                        task.status,
                        craftsman_harness::AgentStatus::Running
                            | craftsman_harness::AgentStatus::Suspended
                    )
                })
            })
    }

    async fn switch_workspace(
        &self,
        workspace: String,
        thread_id: Option<String>,
    ) -> Result<Value, String> {
        self.wait_until_initialized().await;
        if self.bridge.current_turn().is_some() {
            return Err("cannot switch workspaces during an active turn".into());
        }
        if self.bridge.has_pending_interactions() {
            return Err("cannot switch workspaces while input or approval is pending".into());
        }
        if self.voice.active() {
            return Err("stop voice input before switching workspaces".into());
        }
        if self.has_live_background_tasks() {
            return Err("finish or cancel background tasks before switching workspaces".into());
        }

        let workspace = canonical_workspace(&workspace)?;
        let project_contexts = legacy_workspace_context(&workspace)
            .into_iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>();
        let target_thread = match thread_id.filter(|id| !id.trim().is_empty()) {
            Some(thread_id) => thread_id,
            None => {
                let current = self.current_thread().await;
                if current.is_empty() {
                    crate::checkpoints::LibsqlCheckpointer::new_thread_id()
                } else {
                    current
                }
            }
        };
        self.set_thread_contexts(target_thread.clone(), project_contexts)
            .await?;
        let _ = self.switch_thread(target_thread).await?;
        Ok(self.state_snapshot().await)
    }

    async fn workspace_diff(&self) -> Result<String, String> {
        let workspace = self.inner.lock().await.workspace.clone();
        tokio::task::spawn_blocking(move || crate::workspace_diff::workspace_diff_at(&workspace))
            .await
            .map_err(|error| error.to_string())
    }

    async fn ensure_session_for_turn(
        &self,
        inner: &mut RunnerInner,
        thread_id: &str,
    ) -> Result<(), String> {
        if inner.session.is_some() {
            return Ok(());
        }
        if !desktop_provider_configured(&inner.cfg) {
            return Err("configure a Craftsman provider before sending a message".into());
        }
        if inner.cfg.model.is_none() {
            let route = crate::route::resolve_chat_endpoint(&inner.cfg)
                .await
                .map_err(|error| error.to_string())?;
            let models = crate::platform::list_models(&inner.cfg, &route)
                .await
                .map_err(|error| error.to_string())?;
            apply_startup_model(&mut inner.cfg, &models)?;
        }
        let rebuilt = build_desktop_session(
            &inner.cfg,
            &self.approval,
            &self.bridge,
            &inner.workspace,
            &inner.project_contexts,
            Some(thread_id.to_string()),
        )
        .await
        .ok_or_else(|| "the configured agent could not be initialized".to_string())?;
        *self.agent_handle.lock().unwrap() = Some(rebuilt.agent.clone());
        *self.registry_handle.lock().unwrap() = Some(rebuilt.registry.clone());
        inner.session = Some(rebuilt);
        inner.pending_thread_id = None;
        Ok(())
    }

    async fn activate_conversation(
        &self,
        inner: &mut RunnerInner,
        thread_id: &str,
        require_existing: bool,
    ) -> Result<Vec<Value>, String> {
        let checkpointer =
            crate::checkpoints::LibsqlCheckpointer::open_for_workspace(&inner.general_workspace)
                .await?;
        let info = checkpointer
            .thread_info(thread_id)
            .await
            .map_err(|error| error.to_string())?;
        if require_existing && info.is_none() {
            return Err(format!("conversation not found: {thread_id}"));
        }
        let project_contexts = info
            .as_ref()
            .map(|thread| existing_project_contexts(&thread.project_contexts))
            .transpose()?
            .unwrap_or_default();
        let workspace = project_contexts
            .first()
            .cloned()
            .unwrap_or_else(|| inner.general_workspace.clone());
        let messages = checkpointer
            .latest_state_global(thread_id)
            .await
            .map_err(|error| error.to_string())?
            .map(|state| history_values(&state.messages))
            .unwrap_or_default();

        let previous = inner.session.take();
        inner.workspace = workspace;
        inner.project_contexts = project_contexts;
        inner.pending_thread_id = Some(thread_id.to_string());
        *self.agent_handle.lock().unwrap() = None;
        *self.registry_handle.lock().unwrap() = None;
        if let Some(previous) = previous {
            tokio::spawn(async move { previous.lsp.shutdown().await });
        }
        Ok(messages)
    }

    async fn run_turn(&self, prompt: String, thread_id: String) -> Result<(), String> {
        self.wait_until_initialized().await;
        let mut inner = self.inner.lock().await;
        let current_thread = inner
            .session
            .as_ref()
            .map(|session| session.thread_id.as_str())
            .or(inner.pending_thread_id.as_deref());
        if current_thread != Some(thread_id.as_str()) {
            let _ = self
                .activate_conversation(&mut inner, &thread_id, false)
                .await?;
        }
        self.ensure_session_for_turn(&mut inner, &thread_id).await?;
        let Some(session) = inner.session.as_mut() else {
            return Err("agent failed to start".into());
        };
        match session.agent.run_turn(&prompt).await {
            Ok(craftsman_harness::TurnResult::Done(_)) => Ok(()),
            Ok(craftsman_harness::TurnResult::Suspended(suspension)) => {
                agent::rollback_dangling_user(&session.agent);
                Err(format!(
                    "unexpected suspension: {}",
                    suspension.request.summary
                ))
            }
            Err(error) => {
                agent::rollback_dangling_user(&session.agent);
                Err(error.to_string())
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
        let thread = inner
            .session
            .as_ref()
            .map(|session| session.thread_id.clone())
            .or_else(|| inner.pending_thread_id.clone());
        inner.cfg.model = Some(model);
        let _ = inner.cfg.save();
        if let Some(rebuilt) = build_desktop_session(
            &inner.cfg,
            &self.approval,
            &self.bridge,
            &inner.workspace,
            &inner.project_contexts,
            thread.clone(),
        )
        .await
        {
            *self.agent_handle.lock().unwrap() = Some(rebuilt.agent.clone());
            *self.registry_handle.lock().unwrap() = Some(rebuilt.registry.clone());
            inner.session = Some(rebuilt);
            inner.pending_thread_id = None;
            return (true, None);
        }
        inner.cfg.model = previous;
        let _ = inner.cfg.save();
        if let Some(rebuilt) = build_desktop_session(
            &inner.cfg,
            &self.approval,
            &self.bridge,
            &inner.workspace,
            &inner.project_contexts,
            thread,
        )
        .await
        {
            *self.agent_handle.lock().unwrap() = Some(rebuilt.agent.clone());
            *self.registry_handle.lock().unwrap() = Some(rebuilt.registry.clone());
            inner.session = Some(rebuilt);
            inner.pending_thread_id = None;
        }
        (false, Some("agent rebuild failed for that model".into()))
    }

    async fn history(&self, thread_id: String) -> Vec<Value> {
        let inner = self.inner.lock().await;
        let messages = match inner.session.as_ref() {
            Some(session) if thread_id == session.thread_id => {
                session.agent.state.lock().unwrap().messages.clone()
            }
            Some(session) => {
                let Some(checkpointer) = session.checkpointer.as_ref() else {
                    return Vec::new();
                };
                use craftsman_harness::Checkpointer;
                match checkpointer.load_latest(&thread_id).await {
                    Ok(Some((_, state, _))) => state.messages,
                    _ => Vec::new(),
                }
            }
            None => {
                let Ok(checkpointer) =
                    crate::checkpoints::LibsqlCheckpointer::open_for_workspace(&inner.workspace)
                        .await
                else {
                    return Vec::new();
                };
                match checkpointer.latest_state_global(&thread_id).await {
                    Ok(Some(state)) => state.messages,
                    _ => Vec::new(),
                }
            }
        };
        history_values(&messages)
    }

    async fn list_threads(&self, offset: usize, limit: usize) -> Result<ThreadPage, String> {
        let inner = self.inner.lock().await;
        let page_size = limit.clamp(1, MAX_THREAD_PAGE_SIZE);
        let Some(session) = inner.session.as_ref() else {
            let checkpointer =
                crate::checkpoints::LibsqlCheckpointer::open_for_workspace(&inner.workspace)
                    .await?;
            let (rows, has_more) = checkpointer
                .list_threads_page(offset, page_size, None)
                .await
                .map_err(|error| error.to_string())?;
            let consumed = rows.len();
            return Ok(ThreadPage {
                threads: rows.into_iter().map(thread_summary_value).collect(),
                next_offset: has_more.then_some(offset.saturating_add(consumed)),
            });
        };
        let Some(checkpointer) = session.checkpointer.as_ref() else {
            return Ok(ThreadPage {
                threads: if offset == 0 {
                    vec![json!({
                        "id": session.thread_id,
                        "created": 0,
                        "title": "New conversation",
                        "checkpoints": 0,
                        "updated": 0,
                        "project_contexts": inner.project_contexts.iter().map(|path| path.display().to_string()).collect::<Vec<_>>(),
                    })]
                } else {
                    Vec::new()
                },
                next_offset: None,
            });
        };
        let current = checkpointer
            .thread_info(&session.thread_id)
            .await
            .map_err(|error| error.to_string())?;
        let reserved = usize::from(offset == 0);
        let database_limit = page_size.saturating_sub(reserved);
        let query_limit = database_limit.max(1);
        let (rows, has_more) = checkpointer
            .list_threads_page(offset, query_limit, Some(&session.thread_id))
            .await
            .map_err(|error| error.to_string())?;
        let rows_nonempty = !rows.is_empty();
        let consumed = if database_limit == 0 { 0 } else { rows.len() };
        let mut threads = rows
            .into_iter()
            .take(database_limit)
            .map(thread_summary_value)
            .collect::<Vec<_>>();
        if offset == 0 {
            let current = current.map_or_else(
                || {
                    json!({
                        "id": session.thread_id,
                        "created": 0,
                        "title": "New conversation",
                        "checkpoints": 0,
                        "updated": 0,
                        "project_contexts": inner.project_contexts.iter().map(|path| path.display().to_string()).collect::<Vec<_>>(),
                    })
                },
                thread_summary_value,
            );
            threads.insert(0, current);
        }
        let next_offset = (has_more || (database_limit == 0 && rows_nonempty))
            .then_some(offset.saturating_add(consumed));
        Ok(ThreadPage {
            threads,
            next_offset,
        })
    }

    async fn new_thread(&self) -> Result<String, String> {
        let mut inner = self.inner.lock().await;
        let thread_id = crate::checkpoints::LibsqlCheckpointer::new_thread_id();
        let previous = inner.session.take();
        inner.workspace = inner.general_workspace.clone();
        inner.project_contexts.clear();
        inner.pending_thread_id = Some(thread_id.clone());
        *self.agent_handle.lock().unwrap() = None;
        *self.registry_handle.lock().unwrap() = None;
        drop(inner);
        if let Some(previous) = previous {
            tokio::spawn(async move { previous.lsp.shutdown().await });
        }
        Ok(thread_id)
    }

    async fn delete_thread(&self, thread_id: &str) -> Result<String, String> {
        let mut inner = self.inner.lock().await;
        if inner.session.is_none() {
            let checkpointer =
                crate::checkpoints::LibsqlCheckpointer::open_for_workspace(&inner.workspace)
                    .await?;
            checkpointer
                .delete_thread_global(thread_id)
                .await
                .map_err(|error| error.to_string())?;
            if inner.pending_thread_id.as_deref() == Some(thread_id) {
                inner.pending_thread_id =
                    Some(crate::checkpoints::LibsqlCheckpointer::new_thread_id());
            }
            return Ok(inner.pending_thread_id.clone().unwrap_or_default());
        };
        let session = inner.session.as_mut().expect("checked above");
        let Some(checkpointer) = session.checkpointer.clone() else {
            return Err("conversation storage is unavailable".into());
        };
        checkpointer
            .delete_thread_global(thread_id)
            .await
            .map_err(|error| error.to_string())?;
        if session.thread_id == thread_id {
            let next = crate::checkpoints::LibsqlCheckpointer::new_thread_id();
            agent::clear_session(session);
            session.set_thread_id(next);
        }
        Ok(session.thread_id.clone())
    }

    async fn list_models(&self) -> Result<Vec<String>, String> {
        self.wait_until_initialized().await;
        let inner = self.inner.lock().await;
        if inner.cfg.model.is_none()
            && inner.cfg.base_url.is_empty()
            && !inner.cfg.platform_configured()
        {
            return Ok(Vec::new());
        }
        let route = crate::route::resolve_chat_endpoint(&inner.cfg)
            .await
            .map_err(|error| error.to_string())?;
        crate::platform::list_models(&inner.cfg, &route)
            .await
            .map_err(|error| error.to_string())
    }

    fn sync_approval(&self, cfg: &Config) {
        let mut approval = self.approval.lock().unwrap();
        approval.mode = if cfg.auto_approve {
            crate::config::PermissionMode::BypassPermissions
        } else {
            cfg.permission_mode
        };
        approval.auto_approve = cfg.auto_approve;
        approval.auto_approve_network = cfg.auto_approve || cfg.auto_approve_web();
        approval.shell_allow_list = cfg.shell_allow_list.clone();
    }

    async fn rebuild_session(&self, inner: &mut RunnerInner) -> bool {
        self.sync_approval(&inner.cfg);
        let thread = inner
            .session
            .as_ref()
            .map(|session| session.thread_id.clone())
            .or_else(|| inner.pending_thread_id.clone());
        let rebuilt = if desktop_configuration_ready(&inner.cfg) {
            build_desktop_session(
                &inner.cfg,
                &self.approval,
                &self.bridge,
                &inner.workspace,
                &inner.project_contexts,
                thread,
            )
            .await
        } else {
            None
        };
        *self.agent_handle.lock().unwrap() = rebuilt.as_ref().map(|session| session.agent.clone());
        *self.registry_handle.lock().unwrap() =
            rebuilt.as_ref().map(|session| session.registry.clone());
        let ready = rebuilt.is_some();
        inner.session = rebuilt;
        if inner.session.is_some() {
            inner.pending_thread_id = None;
        }
        ready
    }

    async fn state_snapshot(&self) -> Value {
        let inner = self.inner.lock().await;
        let harness = crate::harness_profiles::configured_harness_preset(&inner.cfg).id;
        json!({
            "configured": desktop_configuration_ready(&inner.cfg),
            "provider_configured": desktop_provider_configured(&inner.cfg),
            "agent_ready": inner.session.is_some(),
            "workspace": inner.workspace.display().to_string(),
            "project_contexts": inner.project_contexts.iter().map(|path| path.display().to_string()).collect::<Vec<_>>(),
            "thread_id": inner.session.as_ref().map(|session| session.thread_id.clone()).or_else(|| inner.pending_thread_id.clone()).unwrap_or_default(),
            "model": inner.cfg.model.clone().unwrap_or_default(),
            "permission_mode": inner.cfg.permission_mode.as_str(),
            "harness_profile": harness,
            "platform_configured": inner.cfg.platform_configured(),
            "provider": if inner.cfg.platform_configured() { "platform" } else if !inner.cfg.base_url.is_empty() { "direct" } else { "unconfigured" },
            "platform_url": inner.cfg.platform.base_url,
            "base_url": inner.cfg.base_url,
            "initializing": !self.initialized.load(Ordering::SeqCst),
            "capabilities": ["a2ui", "activity", "structured_tools", "permissions", "ask_user", "threads", "conversation_catalog", "conversation_management", "project_contexts", "models", "steering", "cancellation", "setup", "workspace_switch", "workspace_diff", "checkpoints", "tasks", "tokens", "diagnostics", "voice"],
        })
    }

    async fn configure(&self, request: Value) -> Result<Value, String> {
        self.wait_until_initialized().await;
        let mut inner = self.inner.lock().await;
        let previous = inner.cfg.clone();
        let route = request["route"].as_str().unwrap_or_else(|| {
            if inner.cfg.platform_configured() {
                "platform"
            } else {
                "direct"
            }
        });
        let result: Result<(), String> = async {
            apply_provider_route(
                &mut inner.cfg,
                route,
                request["platform_url"].as_str(),
                request["base_url"].as_str(),
                request["api_key"].as_str(),
                request["api_token"].as_str(),
            )?;
            if let Some(mode) = request["permission_mode"].as_str() {
                inner.cfg.permission_mode = crate::config::PermissionMode::parse(mode)
                    .ok_or_else(|| format!("unknown permission mode: {mode}"))?;
                inner.cfg.auto_approve = false;
            }
            if let Some(profile) = request["harness_profile"].as_str() {
                crate::harness_profiles::select_harness_preset(&mut inner.cfg, profile)?;
            }
            if route == "platform" && inner.cfg.platform.api_token.trim().is_empty() {
                return Err("a platform API token is required; password login was removed".into());
            }
            // A real provider request catches bad URLs, dead credentials, and
            // endpoints that cannot serve Craftsman before the wizard reports success.
            let resolved = crate::route::resolve_chat_endpoint(&inner.cfg)
                .await
                .map_err(|error| format!("provider configuration is invalid: {error}"))?;
            let models = crate::platform::list_models(&inner.cfg, &resolved)
                .await
                .map_err(|error| format!("provider connection test failed: {error}"))?;
            apply_configuration_model(&mut inner.cfg, route, request["model"].as_str(), &models)?;
            inner.cfg.save().map_err(|error| error.to_string())?;
            if !self.rebuild_session(&mut inner).await {
                return Err("the agent could not be built with this configuration".into());
            }
            Ok(())
        }
        .await;
        if let Err(error) = result {
            inner.cfg = previous;
            let _ = inner.cfg.save();
            let _ = self.rebuild_session(&mut inner).await;
            return Err(error);
        }
        drop(inner);
        Ok(self.state_snapshot().await)
    }

    async fn logout(&self) -> Result<Value, String> {
        let mut inner = self.inner.lock().await;
        // Clear the platform block plus any LLM credentials derived from it.
        if inner.cfg.base_url.trim_end_matches('/')
            == platform_llm_base_url(&inner.cfg.platform.base_url)
        {
            inner.cfg.base_url.clear();
            inner.cfg.api_key.clear();
        }
        inner.cfg.platform = crate::config::PlatformConfig::default();
        inner.cfg.save().map_err(|error| error.to_string())?;
        let _ = self.rebuild_session(&mut inner).await;
        drop(inner);
        Ok(self.state_snapshot().await)
    }

    async fn switch_thread(&self, thread_id: String) -> Result<Vec<Value>, String> {
        let mut inner = self.inner.lock().await;
        if inner
            .session
            .as_ref()
            .is_some_and(|session| session.thread_id == thread_id)
        {
            let messages = inner
                .session
                .as_ref()
                .expect("checked current session")
                .agent
                .state
                .lock()
                .unwrap()
                .messages
                .clone();
            return Ok(history_values(&messages));
        }

        self.activate_conversation(&mut inner, &thread_id, true)
            .await
    }

    async fn set_thread_contexts(
        &self,
        thread_id: String,
        project_contexts: Vec<String>,
    ) -> Result<Vec<String>, String> {
        let contexts = canonical_project_contexts(project_contexts)?;
        let mut inner = self.inner.lock().await;
        let checkpointer =
            crate::checkpoints::LibsqlCheckpointer::open_for_workspace(&inner.general_workspace)
                .await?;
        let serialized = contexts
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>();
        checkpointer
            .set_thread_project_contexts(
                &thread_id,
                &serialized,
                &inner.general_workspace.display().to_string(),
            )
            .await
            .map_err(|error| error.to_string())?;

        let is_current = inner
            .session
            .as_ref()
            .map(|session| session.thread_id.as_str())
            .or(inner.pending_thread_id.as_deref())
            == Some(thread_id.as_str());
        if is_current {
            let previous = inner.session.take();
            inner.workspace = contexts
                .first()
                .cloned()
                .unwrap_or_else(|| inner.general_workspace.clone());
            inner.project_contexts = contexts;
            inner.pending_thread_id = Some(thread_id);
            *self.agent_handle.lock().unwrap() = None;
            *self.registry_handle.lock().unwrap() = None;
            drop(inner);
            if let Some(previous) = previous {
                tokio::spawn(async move { previous.lsp.shutdown().await });
            }
        }
        Ok(serialized)
    }

    async fn rename_thread(&self, thread_id: String, title: String) -> Result<(), String> {
        let inner = self.inner.lock().await;
        let checkpointer = match inner
            .session
            .as_ref()
            .and_then(|session| session.checkpointer.clone())
        {
            Some(checkpointer) => checkpointer,
            None => std::sync::Arc::new(
                crate::checkpoints::LibsqlCheckpointer::open_for_workspace(&inner.workspace)
                    .await?,
            ),
        };
        checkpointer
            .rename_thread_global(&thread_id, &title)
            .await
            .map_err(|error| error.to_string())
    }

    async fn export_thread(&self, thread_id: String) -> Result<String, String> {
        let inner = self.inner.lock().await;
        let state = match inner.session.as_ref() {
            Some(session) if thread_id == session.thread_id => {
                session.agent.state.lock().unwrap().clone()
            }
            Some(session) => session
                .checkpointer
                .as_ref()
                .ok_or_else(|| "conversation storage is unavailable".to_string())?
                .latest_state_global(&thread_id)
                .await
                .map_err(|error| error.to_string())?
                .ok_or_else(|| format!("conversation not found: {thread_id}"))?,
            None => crate::checkpoints::LibsqlCheckpointer::open_for_workspace(&inner.workspace)
                .await?
                .latest_state_global(&thread_id)
                .await
                .map_err(|error| error.to_string())?
                .ok_or_else(|| format!("conversation not found: {thread_id}"))?,
        };
        Ok(state_as_markdown(&state))
    }

    async fn duplicate_thread(&self, thread_id: String, branch: bool) -> Result<String, String> {
        let inner = self.inner.lock().await;
        let checkpointer = match inner
            .session
            .as_ref()
            .and_then(|session| session.checkpointer.clone())
        {
            Some(checkpointer) => checkpointer,
            None => std::sync::Arc::new(
                crate::checkpoints::LibsqlCheckpointer::open_for_workspace(&inner.workspace)
                    .await?,
            ),
        };
        let state = match inner.session.as_ref() {
            Some(session) if thread_id == session.thread_id => {
                session.agent.state.lock().unwrap().clone()
            }
            _ => checkpointer
                .latest_state_global(&thread_id)
                .await
                .map_err(|error| error.to_string())?
                .ok_or_else(|| format!("conversation not found: {thread_id}"))?,
        };
        use craftsman_harness::Checkpointer;
        let new_id = crate::checkpoints::LibsqlCheckpointer::new_thread_id();
        checkpointer
            .save(&new_id, 1, &state, None)
            .await
            .map_err(|error| error.to_string())?;
        let source_title = checkpointer
            .thread_info(&thread_id)
            .await
            .map_err(|error| error.to_string())?
            .map(|thread| thread.title)
            .unwrap_or_else(|| "Conversation".to_string());
        let title = if branch {
            format!("Branch of {source_title}")
        } else {
            format!("{source_title} copy")
        };
        checkpointer
            .rename_thread_global(&new_id, &title)
            .await
            .map_err(|error| error.to_string())?;
        checkpointer
            .copy_thread_project_contexts(
                &thread_id,
                &new_id,
                &inner.general_workspace.display().to_string(),
            )
            .await
            .map_err(|error| error.to_string())?;
        Ok(new_id)
    }

    async fn list_checkpoints(&self) -> Result<Vec<Value>, String> {
        let inner = self.inner.lock().await;
        let workspace = inner
            .session
            .as_ref()
            .and_then(|session| session.workspace_checkpoints.clone())
            .ok_or_else(|| "workspace checkpointing is unavailable".to_string())?;
        workspace
            .list()
            .await
            .map_err(|error| error.to_string())
            .map(|items| {
                items.into_iter().map(|item| json!({
            "id": item.id, "turn_seq": item.turn_seq, "created": item.created,
            "summary": item.summary, "file_count": item.file_count, "partial": item.partial,
        })).collect()
            })
    }

    async fn rewind_checkpoint(
        &self,
        checkpoint_id: Option<String>,
        force: bool,
    ) -> Result<Value, String> {
        let inner = self.inner.lock().await;
        let workspace = inner
            .session
            .as_ref()
            .and_then(|session| session.workspace_checkpoints.clone())
            .ok_or_else(|| "workspace checkpointing is unavailable".to_string())?;
        let report = workspace
            .rewind(checkpoint_id.as_deref(), force)
            .await
            .map_err(|error| error.to_string())?;
        Ok(
            json!({"checkpoint_id": report.checkpoint_id, "restored": report.restored, "conflicts": report.conflicts, "unrestorable": report.unrestorable}),
        )
    }

    async fn task_list(&self) -> Result<Vec<Value>, String> {
        let registry = self
            .registry_handle
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| "agent is not configured".to_string())?;
        Ok(registry.details().into_iter().map(|task| json!({
            "name": task.name, "status": format!("{:?}", task.status).to_ascii_lowercase(), "progress": task.progress,
        })).collect())
    }

    fn cancel_task(&self, name: &str) -> bool {
        self.registry_handle
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|registry| registry.abort(name))
    }

    fn message_task(&self, name: &str, message: String) -> bool {
        self.registry_handle
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|registry| registry.deliver(name, message))
    }

    async fn token_usage(&self) -> Result<Value, String> {
        let inner = self.inner.lock().await;
        let session = inner
            .session
            .as_ref()
            .ok_or_else(|| "agent is not configured".to_string())?;
        let (input, output) = *session.usage.lock().unwrap();
        Ok(json!({"input": input, "output": output, "cost_usd": session.cost_usd()}))
    }

    async fn diagnostics(&self) -> Value {
        let inner = self.inner.lock().await;
        json!({
            "agent_ready": inner.session.is_some(),
            "model_configured": inner.cfg.model.is_some(),
            "platform_url": inner.cfg.platform.base_url,
            "platform_configured": inner.cfg.platform_configured(),
            "direct_url": inner.cfg.base_url,
            "direct_api_key_configured": !inner.cfg.api_key.is_empty(),
            "persistence": inner.session.as_ref().is_some_and(|session| session.checkpointer.is_some()),
            "mcp_connected": inner.session.as_ref().map(|session| session.mcp.len()).unwrap_or(0),
            "mcp_errors": inner.session.as_ref().map(|session| session.mcp_errors.len()).unwrap_or(0),
            "lsp_configured": inner.cfg.effective_lsp_servers().len(),
            "route_source": inner.session.as_ref().map(|session| session.route_source.clone()),
        })
    }

    async fn set_voice_model(&self, model: String) -> Result<Value, String> {
        if self.voice.active() {
            return Err("stop voice input before changing its model".into());
        }
        let mut inner = self.inner.lock().await;
        let config = apply_voice_model(&mut inner.cfg, &model)?;
        inner.cfg.save().map_err(|error| error.to_string())?;
        drop(inner);
        self.voice.reconfigure(config).await;
        Ok(self.voice.status())
    }

    async fn set_voice_enabled(&self, enabled: bool) -> Result<Value, String> {
        let mut inner = self.inner.lock().await;
        let config = apply_voice_enabled(&mut inner.cfg, enabled);
        inner.cfg.save().map_err(|error| error.to_string())?;
        drop(inner);
        self.voice.reconfigure(config).await;
        Ok(self.voice.status())
    }

    fn steer(&self, message: String) -> bool {
        self.agent_handle
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|agent| agent.steer(message))
    }

    fn cancel(&self) -> bool {
        let accepted = self
            .agent_handle
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(craftsman_harness::Agent::request_cancel);
        if accepted {
            self.bridge.cancel_pending();
        }
        accepted
    }
}

#[async_trait]
impl DesktopDriver for DesktopRunner {
    async fn ready(&self) -> bool {
        DesktopRunner::ready(self).await
    }

    async fn current_thread(&self) -> String {
        DesktopRunner::current_thread(self).await
    }

    async fn current_model(&self) -> String {
        DesktopRunner::current_model(self).await
    }

    async fn current_workspace(&self) -> String {
        DesktopRunner::current_workspace(self).await
    }

    async fn switch_workspace(
        &self,
        workspace: String,
        thread_id: Option<String>,
    ) -> Result<Value, String> {
        DesktopRunner::switch_workspace(self, workspace, thread_id).await
    }

    async fn run_turn(&self, prompt: String, thread_id: String) -> Result<(), String> {
        DesktopRunner::run_turn(self, prompt, thread_id).await
    }

    async fn set_model(&self, model: String) -> (bool, Option<String>) {
        DesktopRunner::set_model(self, model).await
    }

    async fn history(&self, thread_id: String) -> Vec<Value> {
        DesktopRunner::history(self, thread_id).await
    }

    async fn list_threads(&self, offset: usize, limit: usize) -> Result<ThreadPage, String> {
        DesktopRunner::list_threads(self, offset, limit).await
    }

    async fn new_thread(&self) -> Result<String, String> {
        DesktopRunner::new_thread(self).await
    }

    async fn delete_thread(&self, thread_id: String) -> Result<String, String> {
        DesktopRunner::delete_thread(self, &thread_id).await
    }

    async fn list_models(&self) -> Result<Vec<String>, String> {
        DesktopRunner::list_models(self).await
    }

    async fn state_snapshot(&self) -> Value {
        DesktopRunner::state_snapshot(self).await
    }
    fn voice_status(&self) -> Value {
        self.voice.status()
    }
    fn start_voice(&self, connection_generation: u64) -> Result<(), String> {
        self.voice.start(connection_generation)
    }
    fn stop_voice(&self) -> bool {
        self.voice.stop_and_finalize()
    }
    fn cancel_voice_segment(&self) -> bool {
        self.voice.cancel_segment()
    }
    fn cancel_voice(&self) -> bool {
        self.voice.cancel()
    }
    async fn set_voice_model(&self, model: String) -> Result<Value, String> {
        DesktopRunner::set_voice_model(self, model).await
    }
    async fn set_voice_enabled(&self, enabled: bool) -> Result<Value, String> {
        DesktopRunner::set_voice_enabled(self, enabled).await
    }
    async fn configure(&self, request: Value) -> Result<Value, String> {
        DesktopRunner::configure(self, request).await
    }
    async fn logout(&self) -> Result<Value, String> {
        DesktopRunner::logout(self).await
    }
    async fn switch_thread(&self, thread_id: String) -> Result<Vec<Value>, String> {
        DesktopRunner::switch_thread(self, thread_id).await
    }
    async fn set_thread_contexts(
        &self,
        thread_id: String,
        project_contexts: Vec<String>,
    ) -> Result<Vec<String>, String> {
        DesktopRunner::set_thread_contexts(self, thread_id, project_contexts).await
    }
    async fn rename_thread(&self, thread_id: String, title: String) -> Result<(), String> {
        DesktopRunner::rename_thread(self, thread_id, title).await
    }
    async fn export_thread(&self, thread_id: String) -> Result<String, String> {
        DesktopRunner::export_thread(self, thread_id).await
    }
    async fn duplicate_thread(&self, thread_id: String, branch: bool) -> Result<String, String> {
        DesktopRunner::duplicate_thread(self, thread_id, branch).await
    }
    async fn workspace_diff(&self) -> Result<String, String> {
        DesktopRunner::workspace_diff(self).await
    }
    async fn list_checkpoints(&self) -> Result<Vec<Value>, String> {
        DesktopRunner::list_checkpoints(self).await
    }
    async fn rewind_checkpoint(
        &self,
        checkpoint_id: Option<String>,
        force: bool,
    ) -> Result<Value, String> {
        DesktopRunner::rewind_checkpoint(self, checkpoint_id, force).await
    }
    async fn task_list(&self) -> Result<Vec<Value>, String> {
        DesktopRunner::task_list(self).await
    }
    fn cancel_task(&self, name: &str) -> bool {
        DesktopRunner::cancel_task(self, name)
    }
    fn message_task(&self, name: &str, message: String) -> bool {
        DesktopRunner::message_task(self, name, message)
    }
    async fn token_usage(&self) -> Result<Value, String> {
        DesktopRunner::token_usage(self).await
    }
    async fn diagnostics(&self) -> Value {
        DesktopRunner::diagnostics(self).await
    }

    fn steer(&self, message: String) -> bool {
        DesktopRunner::steer(self, message)
    }

    fn cancel(&self) -> bool {
        DesktopRunner::cancel(self)
    }
}

async fn build_desktop_session(
    cfg: &Config,
    approval: &Arc<Mutex<ApprovalState>>,
    bridge: &Arc<DesktopBridge>,
    workspace: &Path,
    project_contexts: &[PathBuf],
    thread_id: Option<String>,
) -> Option<Session> {
    let token_bridge = bridge.clone();
    let activity_bridge = bridge.clone();
    let approval_bridge = bridge.clone();
    let question_bridge = bridge.clone();
    let tool_bridge = bridge.clone();
    let extras = SessionExtras {
        token_sink: Some(Arc::new(move |text: &str| {
            token_bridge.send(json!({"type": "token", "text": text}));
        })),
        activity_sink: Some(Arc::new(move |text: &str| {
            activity_bridge.send(json!({"type": "activity", "text": text}));
        })),
        tool_activity_sink: Some(Arc::new(move |event| match event {
            ToolActivityEvent::Started {
                call_id,
                tool,
                summary,
                details,
            } => {
                tool_bridge.send(json!({"type": "tool_started", "call_id": call_id, "tool": tool, "summary": summary, "details": details}));
            }
            ToolActivityEvent::Finished {
                call_id,
                tool,
                content,
                is_error,
            } => {
                tool_bridge.send(json!({"type": "tool_finished", "call_id": call_id, "tool": tool, "content": content, "is_error": is_error}));
            }
        })),
        approval_prompt: Some(Arc::new(move |tool, summary, details| {
            approval_bridge.request_approval(tool, summary, details)
        })),
        ask_user_prompt: Some(Arc::new(move |question| {
            question_bridge.request_answer(question)
        })),
        extra_tools: vec![Arc::new(RenderUiTool::new_with_sink(Arc::new({
            let bridge = bridge.clone();
            move |frame| bridge.send(frame)
        })))],
        workspace_root: Some(workspace.to_path_buf()),
        ..SessionExtras::default()
    };
    let extra_system = desktop_system_prompt(project_contexts);
    agent::build_session_ext(
        cfg,
        approval.clone(),
        &extra_system,
        true,
        false,
        thread_id,
        extras,
    )
    .await
}

fn desktop_system_prompt(project_contexts: &[PathBuf]) -> String {
    if project_contexts.is_empty() {
        return SYSTEM_PROMPT_SUFFIX.to_string();
    }
    let mut prompt = String::from(SYSTEM_PROMPT_SUFFIX);
    prompt.push_str("\n\n# Conversation project context\n");
    prompt.push_str("The following project directories are attached to this conversation. The first is the primary project and is the default directory for relative file paths and shell commands. Use absolute paths when working in another attached project. Do not treat unrelated directories as conversation context.\n");
    for (index, path) in project_contexts.iter().enumerate() {
        prompt.push_str(&format!("{}. {}\n", index + 1, path.display()));
    }
    prompt
}

#[derive(Clone)]
struct AppState {
    runner: Arc<dyn DesktopDriver>,
    bridge: Arc<DesktopBridge>,
}

fn authorized(headers: &HeaderMap, query_token: Option<&str>) -> bool {
    if let Some(secret) = std::env::var_os(PROXY_SECRET_ENV) {
        let secret = secret.to_string_lossy().into_owned();
        if !secret.is_empty() {
            let presented = headers
                .get(PROXY_HEADER)
                .and_then(|value| value.to_str().ok());
            return secret_matches(&secret, presented);
        }
    }
    if let Some(secret) = std::env::var_os(DESKTOP_TOKEN_ENV) {
        let secret = secret.to_string_lossy().into_owned();
        if !secret.is_empty() {
            return secret_matches(&secret, query_token);
        }
    }
    true
}

fn secret_matches(secret: &str, presented: Option<&str>) -> bool {
    presented.is_some_and(|value| constant_time_eq(value.as_bytes(), secret.as_bytes()))
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0u8, |difference, (a, b)| difference | (a ^ b))
        == 0
}

fn provider_error_class(message: &str) -> (&'static str, &'static str) {
    provider_connection_error_class(message).unwrap_or(("list_models_failed", "model"))
}

fn configuration_error_class(message: &str) -> (&'static str, &'static str) {
    provider_connection_error_class(message).unwrap_or(("configuration_failed", "configuration"))
}

fn provider_connection_error_class(message: &str) -> Option<(&'static str, &'static str)> {
    let normalized = message.to_ascii_lowercase();
    if normalized.contains("401")
        || normalized.contains("403")
        || normalized.contains("unauthorized")
        || normalized.contains("forbidden")
        || normalized.contains("invalid api key")
        || normalized.contains("incorrect api key")
        || normalized.contains("authentication")
    {
        Some(("provider_authentication_failed", "connection"))
    } else if normalized.contains("timed out")
        || normalized.contains("timeout")
        || normalized.contains("connection refused")
        || normalized.contains("failed to connect")
        || normalized.contains("dns")
        || normalized.contains("resolve host")
        || normalized.contains("network")
        || normalized.contains("http status server error (500")
        || normalized.contains("http status server error (502")
        || normalized.contains("http status server error (503")
        || normalized.contains("http status server error (504")
        || normalized.contains("500 internal server error")
        || normalized.contains("502 bad gateway")
        || normalized.contains("503 service unavailable")
        || normalized.contains("504 gateway timeout")
        || normalized.contains("service unavailable")
        || normalized.contains("bad gateway")
        || normalized.contains("rate limit")
        || normalized.contains("too many requests")
        || normalized.contains("429")
    {
        Some(("provider_unavailable", "connection"))
    } else {
        None
    }
}

fn build_router(runner: Arc<dyn DesktopDriver>, bridge: Arc<DesktopBridge>) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/chat", get(chat_upgrade))
        .with_state(AppState { runner, bridge })
}

async fn healthz(State(state): State<AppState>) -> Json<Value> {
    Json(json!({
        "status": "ok",
        "bridge_ready": true,
        "agent_ready": state.runner.ready().await,
        "protocol": PROTOCOL_VERSION,
    }))
}

async fn chat_upgrade(
    State(state): State<AppState>,
    Query(query): Query<ChatQuery>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if !authorized(&headers, query.token.as_deref()) {
        return StatusCode::FORBIDDEN.into_response();
    }
    ws.on_upgrade(move |socket| chat_session(socket, state))
}

#[derive(serde::Deserialize)]
struct ChatQuery {
    token: Option<String>,
}

async fn chat_session(socket: WebSocket, state: AppState) {
    let (mut socket_tx, mut socket_rx) = socket.split();
    let (frame_tx, mut frame_rx) = tokio::sync::mpsc::unbounded_channel::<Value>();

    let (close_writer, mut writer_close) = tokio::sync::oneshot::channel::<()>();
    let writer = tokio::spawn(async move {
        loop {
            let frame = tokio::select! {
                biased;
                _ = &mut writer_close => break,
                frame = frame_rx.recv() => match frame {
                    Some(frame) => frame,
                    None => break,
                },
            };
            if socket_tx
                .send(WsMessage::Text(frame.to_string().into()))
                .await
                .is_err()
            {
                break;
            }
        }
        let _ = tokio::time::timeout(WRITER_SHUTDOWN, socket_tx.close()).await;
    });
    let _writer_guard = WriterGuard(writer.abort_handle());

    let Some(connection) = DesktopConnection::attach(&state, frame_tx.clone()).await else {
        send_operation_error(
            &frame_tx,
            "client_already_connected",
            "connection",
            "another desktop client is already connected",
            None,
            None,
        );
        drop(frame_tx);
        finish_writer(writer).await;
        return;
    };
    let connection_generation = connection.generation;
    let _ = frame_tx.send(json!({
        "type": "hello",
        "protocol": PROTOCOL_VERSION,
        "model": state.runner.current_model().await,
        "thread_id": state.runner.current_thread().await,
        "a2ui": true,
        "capabilities": ["a2ui", "activity", "structured_tools", "permissions", "ask_user", "threads", "conversation_catalog", "conversation_management", "project_contexts", "models", "steering", "cancellation", "setup", "workspace_switch", "workspace_diff", "checkpoints", "tasks", "tokens", "diagnostics", "voice"],
    }));
    send_snapshot(&state, &frame_tx, None, connection_generation).await;

    while let Some(Ok(message)) = socket_rx.next().await {
        if matches!(message, WsMessage::Close(_)) {
            break;
        }
        let WsMessage::Text(raw) = message else {
            continue;
        };
        let Ok(request) = serde_json::from_str::<Value>(&raw) else {
            send_operation_error(
                &frame_tx,
                "invalid_json",
                "protocol",
                "invalid json",
                None,
                None,
            );
            continue;
        };
        match request["type"].as_str().unwrap_or_default() {
            "message" => {
                let prompt = request["content"].as_str().unwrap_or_default().to_string();
                if prompt.is_empty() {
                    continue;
                }
                let thread_id = match request["thread_id"].as_str().filter(|id| !id.is_empty()) {
                    Some(id) => id.to_string(),
                    None => {
                        let current = state.runner.current_thread().await;
                        if current.is_empty() {
                            crate::checkpoints::LibsqlCheckpointer::new_thread_id()
                        } else {
                            current
                        }
                    }
                };
                let Some(turn) = state
                    .bridge
                    .begin_turn(thread_id.clone(), connection_generation)
                else {
                    let active = state.bridge.current_turn();
                    send_operation_error_with_turn(
                        &frame_tx,
                        "turn_active",
                        "turn",
                        "a turn is already in progress",
                        operation_id(&request),
                        active.as_ref(),
                    );
                    continue;
                };
                let _ = frame_tx.send(json!({"type": "turn_started", "turn_id": turn.turn_id, "thread_id": turn.thread_id}));
                spawn_turn(state.runner.clone(), prompt, turn, state.bridge.clone());
            }
            "ui_action" => {
                let name = request["name"].as_str().unwrap_or_default();
                if name.is_empty() {
                    send_operation_error(
                        &frame_tx,
                        "invalid_ui_action",
                        "a2ui",
                        "ui_action needs a name",
                        operation_id(&request),
                        None,
                    );
                    continue;
                }
                let payload = json!({
                    "context": request.get("context").cloned().unwrap_or_else(|| json!({})),
                    "name": name,
                    "source_component_id": request["source_component_id"].as_str().unwrap_or(""),
                    "surface_id": request["surface_id"].as_str().unwrap_or(""),
                });
                let thread_id = match request["thread_id"].as_str().filter(|id| !id.is_empty()) {
                    Some(id) => id.to_string(),
                    None => state.runner.current_thread().await,
                };
                let Some(turn) = state
                    .bridge
                    .begin_turn(thread_id.clone(), connection_generation)
                else {
                    let active = state.bridge.current_turn();
                    send_operation_error_with_turn(
                        &frame_tx,
                        "turn_active",
                        "turn",
                        "a turn is already in progress",
                        operation_id(&request),
                        active.as_ref(),
                    );
                    continue;
                };
                let _ = frame_tx.send(json!({"type": "turn_started", "turn_id": turn.turn_id, "thread_id": turn.thread_id}));
                spawn_turn(
                    state.runner.clone(),
                    format!("[ui_action] {payload}"),
                    turn,
                    state.bridge.clone(),
                );
            }
            "permission_response" => {
                let request_id = request["request_id"].as_str().unwrap_or_default();
                let decision = request["decision"].as_str().unwrap_or_default();
                let feedback = request["feedback"].as_str();
                if !state
                    .bridge
                    .resolve_approval(request_id, decision, feedback)
                {
                    let turn = state.bridge.current_turn();
                    send_operation_error_with_turn(
                        &frame_tx,
                        "invalid_permission_response",
                        "permission",
                        "unknown permission request or invalid decision",
                        operation_id(&request),
                        turn.as_ref(),
                    );
                }
            }
            "ask_user_response" => {
                let request_id = request["request_id"].as_str().unwrap_or_default();
                let answer = request
                    .get("answer")
                    .and_then(Value::as_str)
                    .map(ToString::to_string);
                if !state.bridge.resolve_question(request_id, answer) {
                    let turn = state.bridge.current_turn();
                    send_operation_error_with_turn(
                        &frame_tx,
                        "invalid_question_response",
                        "question",
                        "unknown ask_user request",
                        operation_id(&request),
                        turn.as_ref(),
                    );
                }
            }
            "steer" => {
                let content = request["content"].as_str().unwrap_or_default().to_string();
                let accepted = state.runner.steer(content.clone());
                let _ = frame_tx.send(json!({
                    "type": "steer",
                    "operation_id": operation_id(&request),
                    "accepted": accepted,
                    "content": content,
                }));
            }
            "cancel" => {
                let accepted = state.runner.cancel();
                let _ = frame_tx.send(json!({
                    "type": "cancel",
                    "operation_id": operation_id(&request),
                    "accepted": accepted,
                }));
            }
            "set_model" => {
                if state.bridge.current_turn().is_some() {
                    let active = state.bridge.current_turn();
                    send_operation_error_with_turn(
                        &frame_tx,
                        "turn_active",
                        "model",
                        "cannot switch models during an active turn",
                        operation_id(&request),
                        active.as_ref(),
                    );
                    continue;
                }
                let model = request["model"].as_str().unwrap_or_default().to_string();
                let operation_id = operation_id(&request).map(ToString::to_string);
                let runner = state.runner.clone();
                let tx = frame_tx.clone();
                tokio::spawn(async move {
                    let (ok, message) = runner.set_model(model).await;
                    let _ = tx.send(json!({
                        "type": "model",
                        "operation_id": operation_id,
                        "model": runner.current_model().await,
                        "ok": ok,
                        "message": message,
                    }));
                });
            }
            "list_models" => {
                let operation_id = operation_id(&request).map(ToString::to_string);
                let runner = state.runner.clone();
                let tx = frame_tx.clone();
                tokio::spawn(async move {
                    match runner.list_models().await {
                        Ok(models) => {
                            let _ = tx.send(json!({
                                "type": "models",
                                "operation_id": operation_id,
                                "current_model": runner.current_model().await,
                                "models": models,
                            }));
                        }
                        Err(message) => {
                            let (code, scope) = provider_error_class(&message);
                            send_operation_error(
                                &tx,
                                code,
                                scope,
                                &message,
                                operation_id.as_deref(),
                                None,
                            )
                        }
                    }
                });
            }
            "history" => {
                let operation_id = operation_id(&request).map(ToString::to_string);
                let runner = state.runner.clone();
                let tx = frame_tx.clone();
                let requested = request["thread_id"]
                    .as_str()
                    .filter(|id| !id.is_empty())
                    .map(ToString::to_string);
                tokio::spawn(async move {
                    let thread_id = match requested {
                        Some(id) => id,
                        None => runner.current_thread().await,
                    };
                    let messages = runner.history(thread_id.clone()).await;
                    let _ = tx.send(json!({
                        "type": "history",
                        "operation_id": operation_id,
                        "thread_id": thread_id,
                        "messages": messages,
                    }));
                });
            }
            "list_threads" => {
                let operation_id = operation_id(&request).map(ToString::to_string);
                let (offset, limit) = thread_page_request(&request);
                let runner = state.runner.clone();
                let tx = frame_tx.clone();
                tokio::spawn(async move {
                    match runner.list_threads(offset, limit).await {
                        Ok(page) => {
                            let _ = tx.send(json!({
                                "type": "threads",
                                "operation_id": operation_id,
                                "current_thread_id": runner.current_thread().await,
                                "threads": page.threads,
                                "offset": offset,
                                "next_offset": page.next_offset,
                                "has_more": page.next_offset.is_some(),
                            }));
                        }
                        Err(message) => send_operation_error(
                            &tx,
                            "list_threads_failed",
                            "conversation",
                            &message,
                            operation_id.as_deref(),
                            None,
                        ),
                    }
                });
            }
            "list_all_threads" => {
                let operation_id = operation_id(&request).map(ToString::to_string);
                let (offset, limit) = thread_page_request(&request);
                let tx = frame_tx.clone();
                tokio::spawn(async move {
                    match crate::checkpoints::LibsqlCheckpointer::list_all_local_threads_page(
                        offset, limit,
                    )
                    .await
                    {
                        Ok((threads, has_more)) => {
                            let consumed = threads.len();
                            let threads = threads
                                .into_iter()
                                .map(thread_summary_value)
                                .collect::<Vec<_>>();
                            let _ = tx.send(json!({
                                "type": "all_threads",
                                "operation_id": operation_id,
                                "threads": threads,
                                "offset": offset,
                                "next_offset": has_more.then_some(offset.saturating_add(consumed)),
                                "has_more": has_more,
                            }));
                        }
                        Err(message) => send_operation_error(
                            &tx,
                            "list_all_threads_failed",
                            "conversation",
                            &message,
                            operation_id.as_deref(),
                            None,
                        ),
                    }
                });
            }
            "new_thread" => {
                if state.bridge.current_turn().is_some() {
                    let active = state.bridge.current_turn();
                    send_operation_error_with_turn(
                        &frame_tx,
                        "turn_active",
                        "conversation",
                        "cannot create a conversation during an active turn",
                        operation_id(&request),
                        active.as_ref(),
                    );
                    continue;
                }
                let operation_id = operation_id(&request).map(ToString::to_string);
                let runner = state.runner.clone();
                let tx = frame_tx.clone();
                tokio::spawn(async move {
                    match runner.new_thread().await {
                        Ok(thread_id) => {
                            let _ = tx.send(json!({
                                "type": "thread",
                                "operation_id": operation_id,
                                "action": "created",
                                "thread_id": thread_id,
                                "current_thread_id": runner.current_thread().await,
                            }));
                            if let Ok(page) = runner.list_threads(0, DEFAULT_THREAD_PAGE_SIZE).await
                            {
                                let _ = tx.send(thread_page_frame(
                                    page,
                                    runner.current_thread().await,
                                    0,
                                    None,
                                ));
                            }
                        }
                        Err(message) => send_operation_error(
                            &tx,
                            "new_thread_failed",
                            "conversation",
                            &message,
                            operation_id.as_deref(),
                            None,
                        ),
                    }
                });
            }
            "delete_thread" => {
                if state.bridge.current_turn().is_some() {
                    let active = state.bridge.current_turn();
                    send_operation_error_with_turn(
                        &frame_tx,
                        "turn_active",
                        "conversation",
                        "cannot delete a conversation during an active turn",
                        operation_id(&request),
                        active.as_ref(),
                    );
                    continue;
                }
                let thread_id = request["thread_id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                if thread_id.is_empty() {
                    send_operation_error(
                        &frame_tx,
                        "invalid_thread",
                        "conversation",
                        "delete_thread needs a thread_id",
                        operation_id(&request),
                        None,
                    );
                    continue;
                }
                let operation_id = operation_id(&request).map(ToString::to_string);
                let runner = state.runner.clone();
                let tx = frame_tx.clone();
                tokio::spawn(async move {
                    match runner.delete_thread(thread_id.clone()).await {
                        Ok(current_thread_id) => {
                            let _ = tx.send(json!({
                                "type": "thread",
                                "operation_id": operation_id,
                                "action": "deleted",
                                "thread_id": thread_id,
                                "current_thread_id": current_thread_id,
                            }));
                            if let Ok(page) = runner.list_threads(0, DEFAULT_THREAD_PAGE_SIZE).await
                            {
                                let _ = tx.send(thread_page_frame(
                                    page,
                                    runner.current_thread().await,
                                    0,
                                    None,
                                ));
                            }
                        }
                        Err(message) => send_operation_error_for_thread(
                            &tx,
                            "delete_thread_failed",
                            "conversation",
                            &message,
                            operation_id.as_deref(),
                            Some(thread_id.as_str()),
                        ),
                    }
                });
            }
            "switch_workspace" => {
                let workspace = request["workspace"]
                    .as_str()
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                if workspace.is_empty() {
                    send_operation_error(
                        &frame_tx,
                        "invalid_workspace",
                        "workspace",
                        "switch_workspace needs a workspace directory",
                        operation_id(&request),
                        None,
                    );
                    continue;
                }
                if !state.bridge.begin_workspace_switch() {
                    send_operation_error_with_turn(
                        &frame_tx,
                        "workspace_busy",
                        "workspace",
                        "cannot switch workspaces during an active turn, pending interaction, or another switch",
                        operation_id(&request),
                        state.bridge.current_turn().as_ref(),
                    );
                    continue;
                }
                let requested_thread = request["thread_id"]
                    .as_str()
                    .filter(|value| !value.is_empty())
                    .map(ToString::to_string);
                let op = operation_id(&request).map(ToString::to_string);
                let switch_state = state.clone();
                let switch_guard = WorkspaceSwitchGuard {
                    bridge: state.bridge.clone(),
                };
                let tx = frame_tx.clone();
                tokio::spawn(async move {
                    let _switch_guard = switch_guard;
                    let result = switch_state
                        .runner
                        .switch_workspace(workspace, requested_thread)
                        .await;
                    match result {
                        Ok(snapshot) => {
                            let workspace = snapshot["workspace"]
                                .as_str()
                                .unwrap_or_default()
                                .to_string();
                            let thread_id = switch_state.runner.current_thread().await;
                            let _ = tx.send(json!({
                                "type": "workspace_switched",
                                "operation_id": op,
                                "workspace": workspace,
                                "thread_id": thread_id,
                            }));
                            send_snapshot(&switch_state, &tx, op.as_deref(), connection_generation)
                                .await;
                            if let Ok(page) = switch_state
                                .runner
                                .list_threads(0, DEFAULT_THREAD_PAGE_SIZE)
                                .await
                            {
                                let _ =
                                    tx.send(thread_page_frame(page, thread_id.clone(), 0, None));
                            }
                            let messages = switch_state.runner.history(thread_id.clone()).await;
                            let _ = tx.send(json!({
                                "type": "history",
                                "thread_id": thread_id,
                                "messages": messages,
                                "switched_workspace": true,
                            }));
                        }
                        Err(message) => send_operation_error(
                            &tx,
                            "workspace_switch_failed",
                            "workspace",
                            &message,
                            op.as_deref(),
                            None,
                        ),
                    }
                });
            }
            "get_state" | "get_config" => {
                send_snapshot(
                    &state,
                    &frame_tx,
                    operation_id(&request),
                    connection_generation,
                )
                .await;
            }
            "configure" => {
                let runner = state.runner.clone();
                let tx = frame_tx.clone();
                let op = operation_id(&request).map(ToString::to_string);
                tokio::spawn(async move {
                    match runner.configure(request).await {
                        Ok(config) => {
                            let _ = tx.send(
                                json!({"type": "configured", "operation_id": op, "config": config}),
                            );
                        }
                        Err(message) => {
                            let (code, scope) = configuration_error_class(&message);
                            send_operation_error(&tx, code, scope, &message, op.as_deref(), None)
                        }
                    }
                });
            }
            "logout" => {
                let runner = state.runner.clone();
                let tx = frame_tx.clone();
                let op = operation_id(&request).map(ToString::to_string);
                tokio::spawn(async move {
                    match runner.logout().await {
                        Ok(config) => {
                            let _ = tx.send(
                                json!({"type": "configured", "operation_id": op, "config": config}),
                            );
                        }
                        Err(message) => send_operation_error(
                            &tx,
                            "logout_failed",
                            "configuration",
                            &message,
                            op.as_deref(),
                            None,
                        ),
                    }
                });
            }
            "switch_thread" => {
                if state.bridge.current_turn().is_some() {
                    let active = state.bridge.current_turn();
                    send_operation_error_with_turn(
                        &frame_tx,
                        "turn_active",
                        "conversation",
                        "cannot switch conversations during an active turn",
                        operation_id(&request),
                        active.as_ref(),
                    );
                    continue;
                }
                let thread_id = request["thread_id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                let runner = state.runner.clone();
                let tx = frame_tx.clone();
                let op = operation_id(&request).map(ToString::to_string);
                tokio::spawn(async move {
                    match runner.switch_thread(thread_id.clone()).await {
                        Ok(messages) => {
                            let snapshot = runner.state_snapshot().await;
                            let project_contexts = snapshot
                                .get("project_contexts")
                                .cloned()
                                .unwrap_or_else(|| json!([]));
                            let _ = tx.send(json!({
                                "type":"history",
                                "operation_id":op,
                                "thread_id":thread_id,
                                "messages":messages,
                                "switched":true,
                                "project_contexts":project_contexts,
                            }));
                        }
                        Err(message) => send_operation_error_for_thread(
                            &tx,
                            "switch_thread_failed",
                            "conversation",
                            &message,
                            op.as_deref(),
                            Some(thread_id.as_str()),
                        ),
                    }
                });
            }
            "set_thread_contexts" => {
                let thread_id = request["thread_id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                let Some(items) = request["project_contexts"].as_array() else {
                    send_operation_error(
                        &frame_tx,
                        "invalid_project_contexts",
                        "conversation",
                        "set_thread_contexts needs a project_contexts array",
                        operation_id(&request),
                        None,
                    );
                    continue;
                };
                if items.iter().any(|item| !item.is_string()) {
                    send_operation_error(
                        &frame_tx,
                        "invalid_project_contexts",
                        "conversation",
                        "project_contexts must contain only directory paths",
                        operation_id(&request),
                        None,
                    );
                    continue;
                }
                let project_contexts = items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(ToString::to_string)
                    .collect::<Vec<_>>();
                if !state.bridge.begin_workspace_switch() {
                    send_operation_error_with_turn(
                        &frame_tx,
                        "conversation_busy",
                        "conversation",
                        "cannot change project context during an active turn, pending interaction, or another context change",
                        operation_id(&request),
                        state.bridge.current_turn().as_ref(),
                    );
                    continue;
                }
                let runner = state.runner.clone();
                let tx = frame_tx.clone();
                let op = operation_id(&request).map(ToString::to_string);
                let switch_guard = WorkspaceSwitchGuard {
                    bridge: state.bridge.clone(),
                };
                tokio::spawn(async move {
                    let _switch_guard = switch_guard;
                    match runner
                        .set_thread_contexts(thread_id.clone(), project_contexts)
                        .await
                    {
                        Ok(project_contexts) => {
                            let _ = tx.send(json!({
                                "type": "thread_contexts",
                                "operation_id": op,
                                "thread_id": thread_id,
                                "project_contexts": project_contexts,
                            }));
                        }
                        Err(message) => send_operation_error_for_thread(
                            &tx,
                            "set_thread_contexts_failed",
                            "conversation",
                            &message,
                            op.as_deref(),
                            Some(thread_id.as_str()),
                        ),
                    }
                });
            }
            "rename_thread" => {
                let runner = state.runner.clone();
                let tx = frame_tx.clone();
                let op = operation_id(&request).map(ToString::to_string);
                let thread_id = request["thread_id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                let title = request["title"].as_str().unwrap_or_default().to_string();
                tokio::spawn(async move {
                    match runner.rename_thread(thread_id.clone(), title.clone()).await {
                        Ok(()) => {
                            let _=tx.send(json!({"type":"thread", "action":"renamed", "operation_id":op, "thread_id":thread_id, "title":title, "current_thread_id":runner.current_thread().await}));
                        }
                        Err(message) => send_operation_error_for_thread(
                            &tx,
                            "rename_thread_failed",
                            "conversation",
                            &message,
                            op.as_deref(),
                            Some(thread_id.as_str()),
                        ),
                    }
                });
            }
            "export_thread" => {
                let runner = state.runner.clone();
                let tx = frame_tx.clone();
                let op = operation_id(&request).map(ToString::to_string);
                let thread_id = request["thread_id"]
                    .as_str()
                    .filter(|id| !id.is_empty())
                    .map(ToString::to_string)
                    .unwrap_or(runner.current_thread().await);
                tokio::spawn(async move {
                    match runner.export_thread(thread_id.clone()).await {
                        Ok(content) => {
                            let _=tx.send(json!({"type":"thread_export", "operation_id":op, "thread_id":thread_id, "format":"markdown", "content":content}));
                        }
                        Err(message) => send_operation_error_for_thread(
                            &tx,
                            "export_thread_failed",
                            "conversation",
                            &message,
                            op.as_deref(),
                            Some(thread_id.as_str()),
                        ),
                    }
                });
            }
            "duplicate_thread" | "branch_thread" => {
                if state.bridge.current_turn().is_some() {
                    let active = state.bridge.current_turn();
                    send_operation_error_with_turn(
                        &frame_tx,
                        "turn_active",
                        "conversation",
                        "cannot duplicate a conversation during an active turn",
                        operation_id(&request),
                        active.as_ref(),
                    );
                    continue;
                }
                let runner = state.runner.clone();
                let tx = frame_tx.clone();
                let op = operation_id(&request).map(ToString::to_string);
                let thread_id = request["thread_id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                let branch = request["type"] == "branch_thread";
                tokio::spawn(async move {
                    match runner.duplicate_thread(thread_id.clone(), branch).await {
                        Ok(new_id) => {
                            let _ = tx.send(json!({"type":"thread", "operation_id":op, "action": if branch {"branched"} else {"duplicated"}, "thread_id":new_id, "source_thread_id":thread_id, "current_thread_id":runner.current_thread().await}));
                            if let Ok(page) = runner.list_threads(0, DEFAULT_THREAD_PAGE_SIZE).await
                            {
                                let _ = tx.send(thread_page_frame(
                                    page,
                                    runner.current_thread().await,
                                    0,
                                    None,
                                ));
                            }
                        }
                        Err(message) => send_operation_error_for_thread(
                            &tx,
                            "duplicate_thread_failed",
                            "conversation",
                            &message,
                            op.as_deref(),
                            Some(thread_id.as_str()),
                        ),
                    }
                });
            }
            "workspace_diff" => {
                let runner = state.runner.clone();
                let tx = frame_tx.clone();
                let op = operation_id(&request).map(ToString::to_string);
                tokio::spawn(async move {
                    match runner.workspace_diff().await {
                        Ok(diff) => {
                            let _ = tx.send(
                                json!({"type":"workspace_diff","operation_id":op,"diff":diff}),
                            );
                        }
                        Err(message) => send_operation_error(
                            &tx,
                            "workspace_diff_failed",
                            "workspace",
                            &message,
                            op.as_deref(),
                            None,
                        ),
                    }
                });
            }
            "list_checkpoints" => {
                let runner = state.runner.clone();
                let tx = frame_tx.clone();
                let op = operation_id(&request).map(ToString::to_string);
                tokio::spawn(async move {
                    match runner.list_checkpoints().await {
                        Ok(checkpoints) => {
                            let _=tx.send(json!({"type":"checkpoints","operation_id":op,"checkpoints":checkpoints}));
                        }
                        Err(message) => send_operation_error(
                            &tx,
                            "checkpoints_failed",
                            "workspace",
                            &message,
                            op.as_deref(),
                            None,
                        ),
                    }
                });
            }
            "rewind_checkpoint" => {
                let runner = state.runner.clone();
                let tx = frame_tx.clone();
                let op = operation_id(&request).map(ToString::to_string);
                let checkpoint = request["checkpoint_id"].as_str().map(ToString::to_string);
                let force = request["force"].as_bool().unwrap_or(false);
                tokio::spawn(async move {
                    match runner.rewind_checkpoint(checkpoint, force).await {
                        Ok(report) => {
                            let _ =
                                tx.send(json!({"type":"rewind","operation_id":op,"report":report}));
                        }
                        Err(message) => send_operation_error(
                            &tx,
                            "rewind_failed",
                            "workspace",
                            &message,
                            op.as_deref(),
                            None,
                        ),
                    }
                });
            }
            "list_tasks" => {
                let runner = state.runner.clone();
                let tx = frame_tx.clone();
                let op = operation_id(&request).map(ToString::to_string);
                tokio::spawn(async move {
                    match runner.task_list().await {
                        Ok(tasks) => {
                            let _ =
                                tx.send(json!({"type":"tasks","operation_id":op,"tasks":tasks}));
                        }
                        Err(message) => send_operation_error(
                            &tx,
                            "tasks_failed",
                            "tasks",
                            &message,
                            op.as_deref(),
                            None,
                        ),
                    }
                });
            }
            "cancel_task" => {
                let accepted = state
                    .runner
                    .cancel_task(request["name"].as_str().unwrap_or_default());
                let _=frame_tx.send(json!({"type":"task_action","operation_id":operation_id(&request),"action":"cancel","accepted":accepted}));
            }
            "message_task" => {
                let accepted = state.runner.message_task(
                    request["name"].as_str().unwrap_or_default(),
                    request["message"].as_str().unwrap_or_default().to_string(),
                );
                let _=frame_tx.send(json!({"type":"task_action","operation_id":operation_id(&request),"action":"message","accepted":accepted}));
            }
            "token_usage" => {
                let runner = state.runner.clone();
                let tx = frame_tx.clone();
                let op = operation_id(&request).map(ToString::to_string);
                tokio::spawn(async move {
                    match runner.token_usage().await {
                        Ok(usage) => {
                            let _ = tx.send(
                                json!({"type":"token_usage","operation_id":op,"usage":usage}),
                            );
                        }
                        Err(message) => send_operation_error(
                            &tx,
                            "token_usage_failed",
                            "runtime",
                            &message,
                            op.as_deref(),
                            None,
                        ),
                    }
                });
            }
            "diagnostics" => {
                let diagnostics = state.runner.diagnostics().await;
                let _=frame_tx.send(json!({"type":"diagnostics","operation_id":operation_id(&request),"diagnostics":diagnostics}));
            }
            "get_voice_status" => {
                let _ = frame_tx.send(json!({
                    "type": "voice_status",
                    "operation_id": operation_id(&request),
                    "voice": state.runner.voice_status(),
                }));
            }
            "voice_start" => match state.runner.start_voice(connection_generation) {
                Ok(()) => {
                    let _ = frame_tx.send(json!({
                        "type": "voice_action",
                        "operation_id": operation_id(&request),
                        "action": "start",
                        "accepted": true,
                    }));
                }
                Err(message) => send_operation_error(
                    &frame_tx,
                    "voice_start_failed",
                    "voice",
                    &message,
                    operation_id(&request),
                    None,
                ),
            },
            "voice_stop" => {
                let accepted = state.runner.stop_voice();
                let _ = frame_tx.send(json!({
                    "type": "voice_action",
                    "operation_id": operation_id(&request),
                    "action": "stop",
                    "accepted": accepted,
                }));
            }
            "voice_cancel_segment" => {
                let accepted = state.runner.cancel_voice_segment();
                let _ = frame_tx.send(json!({
                    "type": "voice_action",
                    "operation_id": operation_id(&request),
                    "action": "cancel_segment",
                    "accepted": accepted,
                }));
            }
            "voice_cancel" => {
                let accepted = state.runner.cancel_voice();
                let _ = frame_tx.send(json!({
                    "type": "voice_action",
                    "operation_id": operation_id(&request),
                    "action": "cancel",
                    "accepted": accepted,
                }));
            }
            "set_voice_model" => {
                let runner = state.runner.clone();
                let tx = frame_tx.clone();
                let op = operation_id(&request).map(ToString::to_string);
                let model = request["model"].as_str().unwrap_or_default().to_string();
                tokio::spawn(async move {
                    match runner.set_voice_model(model).await {
                        Ok(voice) => {
                            let _ = tx.send(
                                json!({"type":"voice_status", "operation_id":op, "voice":voice}),
                            );
                        }
                        Err(message) => send_operation_error(
                            &tx,
                            "voice_model_failed",
                            "voice",
                            &message,
                            op.as_deref(),
                            None,
                        ),
                    }
                });
            }
            "set_voice_enabled" => {
                let runner = state.runner.clone();
                let tx = frame_tx.clone();
                let op = operation_id(&request).map(ToString::to_string);
                let enabled = request["enabled"].as_bool().unwrap_or(false);
                tokio::spawn(async move {
                    match runner.set_voice_enabled(enabled).await {
                        Ok(voice) => {
                            let _ = tx.send(
                                json!({"type":"voice_status", "operation_id":op, "voice":voice}),
                            );
                        }
                        Err(message) => send_operation_error(
                            &tx,
                            "voice_configuration_failed",
                            "voice",
                            &message,
                            op.as_deref(),
                            None,
                        ),
                    }
                });
            }
            _ => {
                send_operation_error(
                    &frame_tx,
                    "unknown_operation",
                    "protocol",
                    "unknown desktop operation",
                    operation_id(&request),
                    None,
                );
            }
        }
    }

    // The owned admission lease stays held through real waiter/turn cleanup.
    // A new client cannot be admitted and then cancelled by the old reader.
    drop(connection);
    drop(frame_tx);
    let _ = close_writer.send(());
    finish_writer(writer).await;
}

async fn finish_writer(mut writer: tokio::task::JoinHandle<()>) {
    if tokio::time::timeout(WRITER_SHUTDOWN, &mut writer)
        .await
        .is_err()
    {
        writer.abort();
        let _ = tokio::time::timeout(WRITER_SHUTDOWN, writer).await;
    }
}

/// Dropping/aborting the reader must not detach an in-flight socket writer.
struct WriterGuard(tokio::task::AbortHandle);
impl Drop for WriterGuard {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// One reader owns admission until its synchronous disconnect cleanup is done.
/// The bounded, event-driven wait lets an already-closing client hand off its
/// slot without replacing a live client or changing protocol-v4 frames.
struct DesktopConnection {
    bridge: Arc<DesktopBridge>,
    runner: Arc<dyn DesktopDriver>,
    generation: u64,
    _owner: tokio::sync::OwnedMutexGuard<()>,
}

impl DesktopConnection {
    async fn attach(state: &AppState, frames: FrameTx) -> Option<Self> {
        let owner = tokio::time::timeout(
            CONNECTION_HANDOFF,
            state.bridge.connection_owner.clone().lock_owned(),
        )
        .await
        .ok()?;
        let generation = state.bridge.attach(frames)?;
        Some(Self {
            bridge: state.bridge.clone(),
            runner: state.runner.clone(),
            generation,
            _owner: owner,
        })
    }
}

impl Drop for DesktopConnection {
    fn drop(&mut self) {
        let owns_slot = self
            .bridge
            .slot
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|connection| connection.generation == self.generation);
        if owns_slot {
            let _ = self.runner.cancel_voice();
            let _ = self.runner.cancel();
            self.bridge.detach(self.generation);
        }
        // _owner unlocks only AFTER this cleanup, including an unwinding reader.
    }
}

fn spawn_turn(
    runner: Arc<dyn DesktopDriver>,
    prompt: String,
    turn: TurnContext,
    bridge: Arc<DesktopBridge>,
) {
    tokio::spawn(async move {
        let result = runner.run_turn(prompt, turn.thread_id.clone()).await;
        let frame = match result {
            Ok(()) => json!({"type": "done", "turn_id": turn.turn_id, "thread_id": turn.thread_id}),
            Err(message) => {
                let (code, scope) =
                    provider_connection_error_class(&message).unwrap_or(("turn_failed", "turn"));
                json!({"type": "turn_error", "code": code, "scope": scope, "turn_id": turn.turn_id, "thread_id": turn.thread_id, "message": message})
            }
        };
        bridge.finish_turn(&turn.turn_id);
        bridge.send_for_turn(&turn, frame);
    });
}

fn operation_id(request: &Value) -> Option<&str> {
    request["operation_id"].as_str()
}

fn thread_page_request(request: &Value) -> (usize, usize) {
    let offset = request["offset"]
        .as_u64()
        .unwrap_or(0)
        .min(usize::MAX as u64) as usize;
    let limit = request["limit"]
        .as_u64()
        .unwrap_or(DEFAULT_THREAD_PAGE_SIZE as u64)
        .clamp(1, MAX_THREAD_PAGE_SIZE as u64) as usize;
    (offset, limit)
}

fn thread_page_frame(
    page: ThreadPage,
    current_thread_id: String,
    offset: usize,
    operation_id: Option<String>,
) -> Value {
    json!({
        "type": "threads",
        "operation_id": operation_id,
        "current_thread_id": current_thread_id,
        "threads": page.threads,
        "offset": offset,
        "next_offset": page.next_offset,
        "has_more": page.next_offset.is_some(),
    })
}

fn send_operation_error_with_turn(
    tx: &FrameTx,
    code: &str,
    scope: &str,
    message: &str,
    operation_id: Option<&str>,
    turn: Option<&TurnContext>,
) {
    let _ = tx.send(json!({"type":"operation_error", "code":code, "scope":scope, "message":message, "operation_id":operation_id, "turn_id":turn.map(|turn| turn.turn_id.as_str()), "thread_id":turn.map(|turn| turn.thread_id.as_str())}));
}

fn send_operation_error_for_thread(
    tx: &FrameTx,
    code: &str,
    scope: &str,
    message: &str,
    operation_id: Option<&str>,
    thread_id: Option<&str>,
) {
    let _ = tx.send(json!({"type":"operation_error", "code":code, "scope":scope, "message":message, "operation_id":operation_id, "thread_id":thread_id}));
}

fn send_operation_error(
    tx: &FrameTx,
    code: &str,
    scope: &str,
    message: &str,
    operation_id: Option<&str>,
    turn_id: Option<&str>,
) {
    let _ = tx.send(json!({"type":"operation_error", "code":code, "scope":scope, "message":message, "operation_id":operation_id, "turn_id":turn_id}));
}

async fn send_snapshot(
    state: &AppState,
    tx: &FrameTx,
    operation_id: Option<&str>,
    connection_generation: u64,
) {
    let mut snapshot = state.runner.state_snapshot().await;
    if let Some(object) = snapshot.as_object_mut() {
        object.insert(
            "active_turn".into(),
            state
                .bridge
                .current_turn()
                .filter(|turn| turn.connection_generation == connection_generation)
                .map(|turn| json!({"turn_id":turn.turn_id,"thread_id":turn.thread_id}))
                .unwrap_or(Value::Null),
        );
        object.insert(
            "pending_interactions".into(),
            Value::Array(state.bridge.pending_snapshot()),
        );
        object.insert("voice".into(), state.runner.voice_status());
        object.insert(
            "capabilities".into(),
            json!([
                "a2ui",
                "structured_tools",
                "permissions",
                "ask_user",
                "threads",
                "conversation_catalog",
                "conversation_management",
                "project_contexts",
                "models",
                "steering",
                "cancellation",
                "setup",
                "workspace_switch",
                "workspace_diff",
                "checkpoints",
                "tasks",
                "tokens",
                "diagnostics",
                "voice"
            ]),
        );
    }
    let _ =
        tx.send(json!({"type":"state_snapshot", "operation_id":operation_id, "state":snapshot}));
}

fn history_values(messages: &[craftsman_harness::Msg]) -> Vec<Value> {
    messages
        .iter()
        .filter_map(|message| match message {
            craftsman_harness::Msg::User { content } if !content.is_empty() => {
                Some(json!({"role":"user","content":content}))
            }
            craftsman_harness::Msg::Assistant { content, .. } if !content.is_empty() => {
                Some(json!({"role":"assistant","content":content}))
            }
            _ => None,
        })
        .collect()
}

fn thread_summary_value(thread: craftsman_harness::ThreadInfo) -> Value {
    json!({
        "id": thread.id,
        "created": thread.created,
        "title": thread.title,
        "checkpoints": thread.checkpoints,
        "updated": thread.updated,
        "project_contexts": thread.project_contexts,
    })
}

fn state_as_markdown(state: &craftsman_harness::AgentState) -> String {
    let mut out = String::from("# Craftsman conversation export\n\n");
    for message in &state.messages {
        match message {
            craftsman_harness::Msg::User { content } => {
                out.push_str(&format!("## You\n\n{content}\n\n"))
            }
            craftsman_harness::Msg::Assistant { content, .. } if !content.trim().is_empty() => {
                out.push_str(&format!("## Craftsman\n\n{content}\n\n"))
            }
            craftsman_harness::Msg::Tool {
                content, is_error, ..
            } => out.push_str(&format!(
                "> Tool {}: {}\n\n",
                if *is_error { "error" } else { "result" },
                crate::util::one_line(content, 2_000)
            )),
            _ => {}
        }
    }
    out
}

pub async fn run(cfg: Config, host: &str, port: u16) -> anyhow::Result<()> {
    let bridge = Arc::new(DesktopBridge::new());
    let workspace = std::env::current_dir()?.canonicalize()?;
    let runner = Arc::new(DesktopRunner::new(cfg, bridge.clone(), workspace));
    let app = build_router(runner.clone(), bridge);
    let listener = tokio::net::TcpListener::bind((host, port)).await?;
    let address = listener.local_addr()?;
    eprintln!("craftsman desktop-serve listening on {address}");
    tokio::spawn(async move { runner.initialize().await });
    axum::serve(listener, app).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use std::sync::atomic::AtomicBool;

    mod connection_tests;

    struct StubDesktopDriver {
        bridge: Arc<DesktopBridge>,
        workspace: Mutex<String>,
        current_thread: Mutex<String>,
        threads: Mutex<Vec<Value>>,
        turn_active: AtomicBool,
        blocked_turn_started: Option<Arc<tokio::sync::Notify>>,
        blocked_turn_release: Option<Arc<tokio::sync::Notify>>,
        configuration_error: Option<String>,
        voice_available: bool,
        voice_start_generation: AtomicU64,
        voice_cancelled: AtomicBool,
    }

    #[async_trait]
    impl DesktopDriver for StubDesktopDriver {
        async fn ready(&self) -> bool {
            true
        }

        async fn current_thread(&self) -> String {
            self.current_thread.lock().unwrap().clone()
        }

        async fn current_model(&self) -> String {
            "stub-model".into()
        }

        async fn current_workspace(&self) -> String {
            self.workspace.lock().unwrap().clone()
        }

        async fn switch_workspace(
            &self,
            workspace: String,
            thread_id: Option<String>,
        ) -> Result<Value, String> {
            *self.workspace.lock().unwrap() = workspace;
            if let Some(thread_id) = thread_id {
                *self.current_thread.lock().unwrap() = thread_id;
            }
            Ok(self.state_snapshot().await)
        }

        async fn run_turn(&self, prompt: String, _thread_id: String) -> Result<(), String> {
            self.turn_active.store(true, Ordering::SeqCst);
            self.bridge
                .send(json!({"type": "activity", "text": format!("Handling {prompt}")}));
            match prompt.as_str() {
                "blocked" => {
                    if let Some(started) = &self.blocked_turn_started {
                        started.notify_one();
                    }
                    if let Some(release) = &self.blocked_turn_release {
                        release.notified().await;
                    }
                    self.bridge
                        .send(json!({"type": "token", "text": "stale token"}));
                }
                "permission" => {
                    let decision = self.bridge.request_approval(
                        "execute",
                        "execute cargo test",
                        "command: cargo test",
                    );
                    let text = match decision {
                        ApprovalPromptResult::Approve => "approved once",
                        ApprovalPromptResult::ApproveAlways => "approved always",
                        ApprovalPromptResult::Reject(_) => "rejected",
                    };
                    self.bridge.send(json!({"type": "token", "text": text}));
                }
                "question" => {
                    let answer = self.bridge.request_answer("Which module?");
                    self.bridge.send(json!({
                        "type": "token",
                        "text": answer.unwrap_or_else(|| "no answer".into()),
                    }));
                }
                "provider-error" => {
                    self.turn_active.store(false, Ordering::SeqCst);
                    return Err("HTTP status client error (401 Unauthorized)".into());
                }
                _ => {
                    self.bridge.send(json!({"type": "token", "text": "ok"}));
                }
            }
            self.turn_active.store(false, Ordering::SeqCst);
            Ok(())
        }

        async fn set_model(&self, model: String) -> (bool, Option<String>) {
            (!model.is_empty(), None)
        }

        async fn history(&self, thread_id: String) -> Vec<Value> {
            vec![json!({"role": "user", "content": format!("history of {thread_id}")})]
        }

        async fn set_thread_contexts(
            &self,
            _thread_id: String,
            project_contexts: Vec<String>,
        ) -> Result<Vec<String>, String> {
            if project_contexts == ["blocked-context"] {
                if let Some(started) = &self.blocked_turn_started {
                    started.notify_one();
                }
                if let Some(release) = &self.blocked_turn_release {
                    release.notified().await;
                }
            }
            Ok(project_contexts)
        }

        async fn list_threads(&self, offset: usize, limit: usize) -> Result<ThreadPage, String> {
            let threads = self.threads.lock().unwrap();
            let end = offset.saturating_add(limit).min(threads.len());
            Ok(ThreadPage {
                threads: threads.get(offset..end).unwrap_or_default().to_vec(),
                next_offset: (end < threads.len()).then_some(end),
            })
        }

        async fn new_thread(&self) -> Result<String, String> {
            let thread_id = "t-new".to_string();
            *self.current_thread.lock().unwrap() = thread_id.clone();
            self.threads.lock().unwrap().insert(
                0,
                json!({
                    "id": thread_id,
                    "created": 30,
                    "title": "New conversation",
                    "checkpoints": 0,
                }),
            );
            Ok(thread_id)
        }

        async fn delete_thread(&self, thread_id: String) -> Result<String, String> {
            self.threads
                .lock()
                .unwrap()
                .retain(|thread| thread["id"].as_str() != Some(thread_id.as_str()));
            if *self.current_thread.lock().unwrap() == thread_id {
                *self.current_thread.lock().unwrap() = "t-main".into();
            }
            Ok(self.current_thread.lock().unwrap().clone())
        }

        async fn duplicate_thread(
            &self,
            thread_id: String,
            branch: bool,
        ) -> Result<String, String> {
            let id = if branch { "t-branch" } else { "t-copy" }.to_string();
            self.threads.lock().unwrap().push(json!({
                "id": id,
                "created": 40,
                "title": if branch { "Branch" } else { "Copy" },
                "checkpoints": 1,
            }));
            assert!(!thread_id.is_empty());
            Ok(id)
        }

        async fn list_models(&self) -> Result<Vec<String>, String> {
            Ok(vec!["stub-model".into(), "other-model".into()])
        }

        async fn configure(&self, _request: Value) -> Result<Value, String> {
            if let Some(error) = &self.configuration_error {
                Err(error.clone())
            } else {
                Ok(self.state_snapshot().await)
            }
        }

        fn steer(&self, message: String) -> bool {
            self.turn_active.load(Ordering::SeqCst) && !message.trim().is_empty()
        }

        fn cancel(&self) -> bool {
            let accepted = self.turn_active.swap(false, Ordering::SeqCst);
            if accepted {
                self.bridge.cancel_pending();
            }
            accepted
        }

        fn voice_status(&self) -> Value {
            json!({
                "available": self.voice_available,
                "enabled": self.voice_available,
                "state": "idle",
                "engine": if self.voice_available { "whisper" } else { "unavailable" },
                "model": if self.voice_available { "base-q5_1" } else { "" },
                "language": Value::Null,
                "auto_submit": false,
                "min_words_to_submit": 3,
                "partial_transcripts": self.voice_available,
                "segment_silence_ms": 800,
                "idle_timeout_ms": 30000,
                "max_session_minutes": 10,
                "models": [],
            })
        }

        fn start_voice(&self, connection_generation: u64) -> Result<(), String> {
            if !self.voice_available {
                return Err("voice input is unavailable".into());
            }
            self.voice_start_generation
                .store(connection_generation, Ordering::SeqCst);
            Ok(())
        }

        fn cancel_voice(&self) -> bool {
            self.voice_cancelled.store(true, Ordering::SeqCst);
            self.voice_available
        }
    }

    async fn spawn_stub_server() -> String {
        let bridge = Arc::new(DesktopBridge::new());
        let driver = Arc::new(StubDesktopDriver {
            bridge: bridge.clone(),
            workspace: Mutex::new("/stub/main".into()),
            current_thread: Mutex::new("t-main".into()),
            threads: Mutex::new(vec![json!({
                "id": "t-main",
                "created": 20,
                "title": "Main thread",
                "checkpoints": 2,
            })]),
            turn_active: AtomicBool::new(false),
            blocked_turn_started: None,
            blocked_turn_release: None,
            configuration_error: None,
            voice_available: false,
            voice_start_generation: AtomicU64::new(0),
            voice_cancelled: AtomicBool::new(false),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = build_router(driver, bridge);
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("ws://{address}/chat")
    }

    async fn spawn_blocked_stub_server()
    -> (String, Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>) {
        let bridge = Arc::new(DesktopBridge::new());
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let driver = Arc::new(StubDesktopDriver {
            bridge: bridge.clone(),
            workspace: Mutex::new("/stub/main".into()),
            current_thread: Mutex::new("t-main".into()),
            threads: Mutex::new(vec![json!({
                "id": "t-main",
                "created": 20,
                "title": "Main thread",
                "checkpoints": 2,
            })]),
            turn_active: AtomicBool::new(false),
            blocked_turn_started: Some(started.clone()),
            blocked_turn_release: Some(release.clone()),
            configuration_error: None,
            voice_available: false,
            voice_start_generation: AtomicU64::new(0),
            voice_cancelled: AtomicBool::new(false),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = build_router(driver, bridge);
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("ws://{address}/chat"), started, release)
    }

    async fn spawn_configuration_failure_server(message: &str) -> String {
        let bridge = Arc::new(DesktopBridge::new());
        let driver = Arc::new(StubDesktopDriver {
            bridge: bridge.clone(),
            workspace: Mutex::new("/stub/main".into()),
            current_thread: Mutex::new("t-main".into()),
            threads: Mutex::new(Vec::new()),
            turn_active: AtomicBool::new(false),
            blocked_turn_started: None,
            blocked_turn_release: None,
            configuration_error: Some(message.to_string()),
            voice_available: false,
            voice_start_generation: AtomicU64::new(0),
            voice_cancelled: AtomicBool::new(false),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = build_router(driver, bridge);
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("ws://{address}/chat")
    }

    async fn spawn_voice_tracking_server() -> (String, Arc<StubDesktopDriver>) {
        let bridge = Arc::new(DesktopBridge::new());
        let driver = Arc::new(StubDesktopDriver {
            bridge: bridge.clone(),
            workspace: Mutex::new("/stub/main".into()),
            current_thread: Mutex::new("t-main".into()),
            threads: Mutex::new(Vec::new()),
            turn_active: AtomicBool::new(false),
            blocked_turn_started: None,
            blocked_turn_release: None,
            configuration_error: None,
            voice_available: true,
            voice_start_generation: AtomicU64::new(0),
            voice_cancelled: AtomicBool::new(false),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = build_router(driver.clone(), bridge);
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("ws://{address}/chat"), driver)
    }

    async fn recv_json(
        socket: &mut tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    ) -> Value {
        loop {
            match socket.next().await.expect("socket open").expect("frame") {
                tokio_tungstenite::tungstenite::Message::Text(text) => {
                    return serde_json::from_str(&text).expect("json frame");
                }
                _ => continue,
            }
        }
    }

    #[test]
    fn approval_bridge_round_trip() {
        let bridge = Arc::new(DesktopBridge::new());
        let (frames, mut incoming) = tokio::sync::mpsc::unbounded_channel();
        let generation = bridge.attach(frames).expect("connection");
        bridge
            .begin_turn("thread-1".into(), generation)
            .expect("turn");
        let worker_bridge = bridge.clone();
        let worker = std::thread::spawn(move || {
            worker_bridge.request_approval("execute", "execute cargo test", "command: cargo test")
        });
        let frame = incoming.blocking_recv().expect("permission frame");
        assert_eq!(frame["type"], "permission_request");
        let request_id = frame["request_id"].as_str().unwrap();
        assert!(bridge.resolve_approval(request_id, "approve_always", None));
        assert_eq!(worker.join().unwrap(), ApprovalPromptResult::ApproveAlways);
    }

    #[test]
    fn ask_user_bridge_round_trip() {
        let bridge = Arc::new(DesktopBridge::new());
        let (frames, mut incoming) = tokio::sync::mpsc::unbounded_channel();
        let generation = bridge.attach(frames).expect("connection");
        bridge
            .begin_turn("thread-1".into(), generation)
            .expect("turn");
        let worker_bridge = bridge.clone();
        let worker = std::thread::spawn(move || worker_bridge.request_answer("Which module?"));
        let frame = incoming.blocking_recv().expect("question frame");
        let request_id = frame["request_id"].as_str().unwrap();
        assert!(bridge.resolve_question(request_id, Some("a new module".into())));
        assert_eq!(worker.join().unwrap().as_deref(), Some("a new module"));
    }

    #[test]
    fn disconnect_denies_pending_requests() {
        let bridge = Arc::new(DesktopBridge::new());
        let (frames, mut incoming) = tokio::sync::mpsc::unbounded_channel();
        let generation = bridge.attach(frames).expect("connection");
        bridge
            .begin_turn("thread-1".into(), generation)
            .expect("turn");
        let worker_bridge = bridge.clone();
        let worker = std::thread::spawn(move || {
            worker_bridge.request_approval("write_file", "write file.txt", "path: file.txt")
        });
        let _ = incoming.blocking_recv().expect("permission frame");
        bridge.cancel_pending();
        assert_eq!(
            worker.join().unwrap(),
            ApprovalPromptResult::Reject(String::new())
        );
    }

    #[test]
    fn secret_matching_is_constant_shape_and_rejects_missing_values() {
        assert!(secret_matches("desktop-secret", Some("desktop-secret")));
        assert!(!secret_matches("desktop-secret", Some("wrong")));
        assert!(!secret_matches("desktop-secret", None));
    }

    #[tokio::test]
    async fn headless_runner_is_snapshot_ready_before_agent_initialization() {
        let bridge = Arc::new(DesktopBridge::new());
        let runner =
            DesktopRunner::new(Config::default(), bridge, std::env::current_dir().unwrap());
        let starting = runner.state_snapshot().await;
        assert_eq!(starting["initializing"], true);
        assert_eq!(starting["agent_ready"], false);
        assert!(
            starting["capabilities"]
                .as_array()
                .unwrap()
                .iter()
                .any(|capability| capability == "workspace_switch")
        );
        assert!(
            starting["capabilities"]
                .as_array()
                .unwrap()
                .iter()
                .any(|capability| capability == "project_contexts")
        );
        assert_eq!(starting["project_contexts"], json!([]));
        assert!(
            starting["thread_id"]
                .as_str()
                .is_some_and(|id| !id.is_empty())
        );

        runner.initialize().await;
        let initialized = runner.state_snapshot().await;
        assert_eq!(initialized["initializing"], false);
    }

    #[tokio::test]
    async fn headless_workspace_switch_does_not_require_a_provider_session() {
        let initial = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let bridge = Arc::new(DesktopBridge::new());
        let runner = DesktopRunner::new(Config::default(), bridge, initial.path().to_path_buf());
        runner.initialize().await;

        // This checks absence of a provider dependency, not disk latency.
        // Initial metadata/database I/O on a shared Windows runner can exceed
        // 100 ms; keep a generous bound that still catches a wedged operation.
        let snapshot = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            runner.switch_workspace(target.path().display().to_string(), None),
        )
        .await
        .expect("metadata-only workspace switching must not build a session")
        .unwrap();
        assert_eq!(
            snapshot["workspace"],
            target.path().canonicalize().unwrap().display().to_string()
        );
        assert_eq!(snapshot["agent_ready"], false);
        assert!(
            snapshot["thread_id"]
                .as_str()
                .is_some_and(|id| !id.is_empty())
        );
    }

    #[test]
    fn project_contexts_are_canonical_deduplicated_and_described_to_the_agent() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let contexts = canonical_project_contexts(vec![
            first.path().display().to_string(),
            second.path().display().to_string(),
            first.path().display().to_string(),
        ])
        .unwrap();
        assert_eq!(contexts.len(), 2);
        let prompt = desktop_system_prompt(&contexts);
        assert!(prompt.contains("primary project"));
        assert!(prompt.contains(&first.path().canonicalize().unwrap().display().to_string()));
        assert!(prompt.contains(&second.path().canonicalize().unwrap().display().to_string()));
        let missing = first.path().join("missing");
        assert!(
            existing_project_contexts(&[
                missing.display().to_string(),
                second.path().display().to_string(),
            ])
            .is_err()
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn project_context_change_blocks_a_racing_turn_until_it_finishes() {
        let (url, started, release) = spawn_blocked_stub_server().await;
        let (mut socket, _) = tokio_tungstenite::connect_async(url).await.unwrap();
        assert_eq!(recv_json(&mut socket).await["type"], "hello");
        assert_eq!(recv_json(&mut socket).await["type"], "state_snapshot");

        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                json!({
                    "type": "set_thread_contexts",
                    "operation_id": "contexts-blocked",
                    "thread_id": "t-main",
                    "project_contexts": ["blocked-context"]
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        started.notified().await;
        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                json!({"type": "message", "content": "must wait", "thread_id": "t-main"})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        let blocked = recv_json(&mut socket).await;
        assert_eq!(blocked["type"], "operation_error");
        assert_eq!(blocked["code"], "turn_active");
        release.notify_one();
        let contexts = recv_json(&mut socket).await;
        assert_eq!(contexts["type"], "thread_contexts");
    }

    #[tokio::test]
    async fn headless_initialization_does_not_probe_or_build_the_configured_provider() {
        let cfg = Config {
            base_url: "http://127.0.0.1:1/v1".into(),
            api_key: "configured-key".into(),
            model: Some("configured-model".into()),
            ..Config::default()
        };
        let bridge = Arc::new(DesktopBridge::new());
        let runner = DesktopRunner::new(cfg, bridge, std::env::current_dir().unwrap());
        tokio::time::timeout(std::time::Duration::from_millis(100), runner.initialize())
            .await
            .expect("headless initialization must be local-only");
        let snapshot = runner.state_snapshot().await;
        assert_eq!(snapshot["provider_configured"], true);
        assert_eq!(snapshot["agent_ready"], false);
        assert_eq!(snapshot["model"], "configured-model");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn idle_disconnect_allows_immediate_reconnect() {
        let url = spawn_stub_server().await;
        let (mut socket, _) = tokio_tungstenite::connect_async(url.clone()).await.unwrap();
        assert_eq!(recv_json(&mut socket).await["type"], "hello");
        assert_eq!(recv_json(&mut socket).await["type"], "state_snapshot");
        socket.close(None).await.unwrap();

        let reconnect = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            tokio_tungstenite::connect_async(url),
        )
        .await
        .expect("idle disconnect must release the bridge")
        .unwrap();
        let mut socket = reconnect.0;
        assert_eq!(recv_json(&mut socket).await["type"], "hello");
        assert_eq!(recv_json(&mut socket).await["type"], "state_snapshot");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn workspace_switch_reuses_the_connection_and_refreshes_context() {
        let url = spawn_stub_server().await;
        let (mut socket, _) = tokio_tungstenite::connect_async(url).await.unwrap();
        let hello = recv_json(&mut socket).await;
        assert!(
            hello["capabilities"]
                .as_array()
                .unwrap()
                .iter()
                .any(|capability| capability == "workspace_switch")
        );
        let initial = recv_json(&mut socket).await;
        assert_eq!(initial["state"]["workspace"], "/stub/main");

        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                json!({
                    "type": "switch_workspace",
                    "operation_id": "workspace-1",
                    "workspace": "/stub/project",
                    "thread_id": "t-project",
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();

        let switched = recv_json(&mut socket).await;
        assert_eq!(switched["type"], "workspace_switched");
        assert_eq!(switched["operation_id"], "workspace-1");
        assert_eq!(switched["workspace"], "/stub/project");
        assert_eq!(switched["thread_id"], "t-project");

        let snapshot = recv_json(&mut socket).await;
        assert_eq!(snapshot["type"], "state_snapshot");
        assert_eq!(snapshot["operation_id"], "workspace-1");
        assert_eq!(snapshot["state"]["workspace"], "/stub/project");
        assert_eq!(recv_json(&mut socket).await["type"], "threads");
        let history = recv_json(&mut socket).await;
        assert_eq!(history["type"], "history");
        assert_eq!(history["thread_id"], "t-project");

        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                json!({"type":"get_state", "operation_id":"state-after-switch"})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        let state = recv_json(&mut socket).await;
        assert_eq!(state["state"]["workspace"], "/stub/project");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn workspace_switch_is_rejected_during_an_active_turn() {
        let (url, started, release) = spawn_blocked_stub_server().await;
        let (mut socket, _) = tokio_tungstenite::connect_async(url).await.unwrap();
        assert_eq!(recv_json(&mut socket).await["type"], "hello");
        assert_eq!(recv_json(&mut socket).await["type"], "state_snapshot");
        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                json!({"type":"message", "content":"blocked"})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        assert_eq!(recv_json(&mut socket).await["type"], "turn_started");
        assert_eq!(recv_json(&mut socket).await["type"], "activity");
        tokio::time::timeout(std::time::Duration::from_secs(1), started.notified())
            .await
            .expect("turn started");

        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                json!({
                    "type":"switch_workspace",
                    "operation_id":"workspace-busy",
                    "workspace":"/stub/other"
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        let error = recv_json(&mut socket).await;
        assert_eq!(error["type"], "operation_error");
        assert_eq!(error["code"], "workspace_busy");
        assert_eq!(error["operation_id"], "workspace-busy");
        release.notify_one();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn provider_recovery_is_reported_by_the_chat_turn_not_startup() {
        let url = spawn_stub_server().await;
        let (mut socket, _) = tokio_tungstenite::connect_async(url).await.unwrap();
        assert_eq!(recv_json(&mut socket).await["type"], "hello");
        let snapshot = recv_json(&mut socket).await;
        assert_eq!(snapshot["type"], "state_snapshot");
        assert!(snapshot["state"].get("provider_issue").is_none());

        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                json!({"type":"message", "content":"provider-error"})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        assert_eq!(recv_json(&mut socket).await["type"], "turn_started");
        assert_eq!(recv_json(&mut socket).await["type"], "activity");
        let error = recv_json(&mut socket).await;
        assert_eq!(error["type"], "turn_error");
        assert_eq!(error["code"], "provider_authentication_failed");
        assert_eq!(error["scope"], "connection");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn disconnect_cancels_pending_interactions_and_allows_reconnect() {
        for prompt in ["permission", "question"] {
            let url = spawn_stub_server().await;
            let (mut socket, _) = tokio_tungstenite::connect_async(url.clone()).await.unwrap();
            assert_eq!(recv_json(&mut socket).await["type"], "hello");
            assert_eq!(recv_json(&mut socket).await["type"], "state_snapshot");
            socket
                .send(tokio_tungstenite::tungstenite::Message::Text(
                    json!({"type": "message", "content": prompt})
                        .to_string()
                        .into(),
                ))
                .await
                .unwrap();
            assert_eq!(recv_json(&mut socket).await["type"], "turn_started");
            assert_eq!(recv_json(&mut socket).await["type"], "activity");
            let pending = recv_json(&mut socket).await;
            assert!(matches!(
                pending["type"].as_str(),
                Some("permission_request" | "ask_user")
            ));
            socket.close(None).await.unwrap();

            let reconnect = tokio::time::timeout(
                std::time::Duration::from_secs(1),
                tokio_tungstenite::connect_async(url),
            )
            .await
            .expect("pending waiter cancellation must release the bridge")
            .unwrap();
            let mut socket = reconnect.0;
            assert_eq!(recv_json(&mut socket).await["type"], "hello");
            assert_eq!(recv_json(&mut socket).await["type"], "state_snapshot");
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stale_turn_output_never_enters_a_reconnected_client() {
        let (url, started, release) = spawn_blocked_stub_server().await;
        let (mut socket, _) = tokio_tungstenite::connect_async(url.clone()).await.unwrap();
        assert_eq!(recv_json(&mut socket).await["type"], "hello");
        assert_eq!(recv_json(&mut socket).await["type"], "state_snapshot");
        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                json!({"type": "message", "content": "blocked"})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        assert_eq!(recv_json(&mut socket).await["type"], "turn_started");
        assert_eq!(recv_json(&mut socket).await["type"], "activity");
        tokio::time::timeout(std::time::Duration::from_secs(1), started.notified())
            .await
            .expect("old turn must reach its blocking point");
        socket.close(None).await.unwrap();

        let reconnect = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            tokio_tungstenite::connect_async(url),
        )
        .await
        .expect("disconnect must release the bridge")
        .unwrap();
        let mut socket = reconnect.0;
        assert_eq!(recv_json(&mut socket).await["type"], "hello");
        let snapshot = recv_json(&mut socket).await;
        assert_eq!(snapshot["type"], "state_snapshot");
        assert!(snapshot["state"]["active_turn"].is_null());

        release.notify_one();
        let leaked =
            tokio::time::timeout(std::time::Duration::from_millis(250), socket.next()).await;
        assert!(
            leaked.is_err(),
            "old turn emitted a frame into the new connection"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn voice_start_is_connection_scoped_and_disconnect_cancels_capture() {
        let (url, driver) = spawn_voice_tracking_server().await;
        let (mut socket, _) = tokio_tungstenite::connect_async(url).await.unwrap();
        assert_eq!(recv_json(&mut socket).await["type"], "hello");
        assert_eq!(recv_json(&mut socket).await["type"], "state_snapshot");
        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                json!({"type":"voice_start", "operation_id":"voice-start"})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        let started = recv_json(&mut socket).await;
        assert_eq!(started["type"], "voice_action");
        assert_eq!(started["operation_id"], "voice-start");
        assert_eq!(started["accepted"], true);
        assert!(driver.voice_start_generation.load(Ordering::SeqCst) > 0);

        socket.close(None).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while !driver.voice_cancelled.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("disconnect must cancel active voice capture");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn configure_provider_failures_keep_their_connection_error_type() {
        for (message, expected_code) in [
            (
                "provider connection test failed: HTTP status client error (401 Unauthorized)",
                "provider_authentication_failed",
            ),
            (
                "provider connection test failed: HTTP status server error (503 Service Unavailable)",
                "provider_unavailable",
            ),
        ] {
            let url = spawn_configuration_failure_server(message).await;
            let (mut socket, _) = tokio_tungstenite::connect_async(url).await.unwrap();
            assert_eq!(recv_json(&mut socket).await["type"], "hello");
            assert_eq!(recv_json(&mut socket).await["type"], "state_snapshot");
            socket
                .send(tokio_tungstenite::tungstenite::Message::Text(
                    json!({
                        "type": "configure",
                        "operation_id": "configure-recovery",
                        "route": "direct",
                        "base_url": "https://provider.example.com/v1",
                        "api_key": "secret",
                        "model": "model",
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .unwrap();
            let error = recv_json(&mut socket).await;
            assert_eq!(error["type"], "operation_error");
            assert_eq!(error["operation_id"], "configure-recovery");
            assert_eq!(error["code"], expected_code);
            assert_eq!(error["scope"], "connection");
        }
    }

    #[test]
    fn turn_scoped_operation_errors_include_thread_correlation() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let turn = TurnContext {
            turn_id: "turn-1".into(),
            thread_id: "thread-1".into(),
            connection_generation: 1,
        };
        send_operation_error_with_turn(
            &tx,
            "turn_active",
            "turn",
            "busy",
            Some("op-1"),
            Some(&turn),
        );
        let frame = rx.blocking_recv().unwrap();
        assert_eq!(frame["operation_id"], "op-1");
        assert_eq!(frame["turn_id"], "turn-1");
        assert_eq!(frame["thread_id"], "thread-1");
    }

    #[test]
    fn thread_scoped_operation_errors_include_target_thread() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        send_operation_error_for_thread(
            &tx,
            "switch_thread_failed",
            "conversation",
            "missing",
            Some("op-thread"),
            Some("thread-target"),
        );
        let frame = rx.blocking_recv().unwrap();
        assert_eq!(frame["operation_id"], "op-thread");
        assert_eq!(frame["thread_id"], "thread-target");
        assert!(frame["turn_id"].is_null());
    }

    #[test]
    fn provider_errors_distinguish_authentication_and_availability() {
        assert_eq!(
            provider_error_class("HTTP status client error (401 Unauthorized)"),
            ("provider_authentication_failed", "connection")
        );
        assert_eq!(
            provider_error_class("Incorrect API key provided"),
            ("provider_authentication_failed", "connection")
        );
        assert_eq!(
            provider_error_class("connection refused while requesting /models"),
            ("provider_unavailable", "connection")
        );
        assert_eq!(
            provider_error_class("HTTP status server error (503 Service Unavailable)"),
            ("provider_unavailable", "connection")
        );
        assert_eq!(
            provider_error_class("429 Too Many Requests: rate limit exceeded"),
            ("provider_unavailable", "connection")
        );
        assert_eq!(
            provider_error_class("model catalog response was malformed"),
            ("list_models_failed", "model")
        );
        assert_eq!(
            configuration_error_class(
                "provider connection test failed: HTTP status client error (401 Unauthorized)",
            ),
            ("provider_authentication_failed", "connection")
        );
        assert_eq!(
            configuration_error_class(
                "provider connection test failed: HTTP status server error (503 Service Unavailable)",
            ),
            ("provider_unavailable", "connection")
        );
        assert_eq!(
            configuration_error_class("model is required"),
            ("configuration_failed", "configuration")
        );
    }

    #[test]
    fn voice_events_preserve_structured_progress_and_transcript_fields() {
        let progress = voice_event_frame(
            crate::voice::VoiceEvent::DownloadProgress {
                downloaded: 25,
                total: 100,
            },
            "base-q5_1",
        );
        assert_eq!(progress["type"], "voice_event");
        assert_eq!(progress["event"], "download_progress");
        assert_eq!(progress["downloaded"], 25);
        assert_eq!(progress["total"], 100);

        let partial = voice_event_frame(
            crate::voice::VoiceEvent::Partial {
                committed: "hello ".into(),
                tentative: "world".into(),
            },
            "base-q5_1",
        );
        assert_eq!(partial["event"], "partial");
        assert_eq!(partial["committed"], "hello ");
        assert_eq!(partial["tentative"], "world");

        let finished = voice_event_frame(
            crate::voice::VoiceEvent::SessionFinished {
                dictated_text: "hello world".into(),
            },
            "base-q5_1",
        );
        assert_eq!(finished["event"], "session_finished");
        assert_eq!(finished["dictated_text"], "hello world");
    }

    #[test]
    fn voice_frames_are_scoped_to_the_starting_connection_generation() {
        let bridge = DesktopBridge::new();
        let (first_tx, _first_rx) = tokio::sync::mpsc::unbounded_channel();
        let first_generation = bridge.attach(first_tx).unwrap();
        assert!(bridge.detach(first_generation));
        let (second_tx, mut second_rx) = tokio::sync::mpsc::unbounded_channel();
        let second_generation = bridge.attach(second_tx).unwrap();
        assert!(!bridge.send_for_generation(
            first_generation,
            json!({"type":"voice_event", "event":"partial", "tentative":"stale"}),
        ));
        assert!(second_rx.try_recv().is_err());
        assert!(bridge.send_for_generation(
            second_generation,
            json!({"type":"voice_event", "event":"state", "state":"recording"}),
        ));
        assert_eq!(second_rx.try_recv().unwrap()["state"], "recording");
    }

    #[test]
    fn platform_configuration_selects_a_model_from_the_listing() {
        let mut cfg = Config::default();
        let models = vec!["model-b".to_string(), "model-a".to_string()];
        apply_configuration_model(&mut cfg, "platform", None, &models).unwrap();
        assert_eq!(cfg.model.as_deref(), Some("model-b"));
    }

    #[test]
    fn platform_configuration_prefers_a_valid_requested_or_saved_model() {
        let models = vec!["model-a".to_string(), "model-b".to_string()];
        let mut cfg = Config {
            model: Some("model-b".into()),
            ..Default::default()
        };
        apply_configuration_model(&mut cfg, "platform", None, &models).unwrap();
        assert_eq!(cfg.model.as_deref(), Some("model-b"));

        apply_configuration_model(&mut cfg, "platform", Some("model-a"), &models).unwrap();
        assert_eq!(cfg.model.as_deref(), Some("model-a"));
    }

    #[test]
    fn startup_model_uses_a_valid_default_then_falls_back_to_first_available() {
        let models = vec!["first".to_string(), "preferred".to_string()];
        let mut cfg = Config {
            default_model: Some("preferred".into()),
            ..Config::default()
        };
        apply_startup_model(&mut cfg, &models).unwrap();
        assert_eq!(cfg.model.as_deref(), Some("preferred"));

        let mut cfg = Config::default();
        apply_startup_model(&mut cfg, &models).unwrap();
        assert_eq!(cfg.model.as_deref(), Some("first"));
        assert!(apply_startup_model(&mut Config::default(), &[]).is_err());
    }

    #[test]
    fn direct_configuration_still_requires_an_explicit_model() {
        let mut cfg = Config::default();
        assert!(apply_configuration_model(&mut cfg, "direct", None, &[]).is_err());
        apply_configuration_model(&mut cfg, "direct", Some("direct-model"), &[]).unwrap();
        assert_eq!(cfg.model.as_deref(), Some("direct-model"));
    }

    #[test]
    fn voice_enabled_setting_preserves_shared_nested_configuration() {
        let mut cfg = Config {
            extra: serde_json::from_value(json!({
                "voice": {"enabled": true, "python_only": {"keep": 1}}
            }))
            .unwrap(),
            ..Config::default()
        };
        let voice = apply_voice_enabled(&mut cfg, false);
        assert!(!voice.enabled);
        assert_eq!(cfg.extra["voice"]["python_only"]["keep"], 1);
    }

    #[cfg(all(feature = "voice", any(target_os = "macos", target_os = "linux")))]
    #[test]
    fn voice_model_setting_uses_the_shared_cli_catalog() {
        let mut cfg = Config::default();
        let voice = apply_voice_model(&mut cfg, "base").unwrap();
        assert_eq!(voice.model, "base-q5_1");
        assert_eq!(
            cfg.voice_setting("model").and_then(Value::as_str),
            Some("base-q5_1")
        );
        assert!(apply_voice_model(&mut cfg, "unknown").is_err());
    }

    #[test]
    fn provider_route_selection_clears_the_competing_route() {
        let mut cfg = Config {
            platform: crate::config::PlatformConfig {
                base_url: "https://platform.example.com".into(),
                api_token: "mpat-token".into(),
            },
            base_url: "https://platform.example.com/apps/llm/api/v1".into(),
            api_key: "mpat-token".into(),
            ..Config::default()
        };
        apply_provider_route(
            &mut cfg,
            "direct",
            None,
            Some("https://direct.example.com/v1/"),
            Some("new-key"),
            None,
        )
        .unwrap();
        assert_eq!(cfg.base_url, "https://direct.example.com/v1");
        assert_eq!(cfg.api_key, "new-key");
        assert_eq!(cfg.platform, crate::config::PlatformConfig::default());
        cfg.model = Some("model".into());
        assert!(desktop_configuration_ready(&cfg));

        // Platform route: URL + API token; the LLM endpoint is derived.
        apply_provider_route(
            &mut cfg,
            "platform",
            Some("https://platform.example.com/"),
            None,
            None,
            Some("mpat-token"),
        )
        .unwrap();
        assert_eq!(cfg.platform.base_url, "https://platform.example.com");
        assert_eq!(cfg.platform.api_token, "mpat-token");
        assert_eq!(cfg.base_url, "https://platform.example.com/apps/llm/api/v1");
        assert_eq!(cfg.api_key, "mpat-token");
        assert!(desktop_configuration_ready(&cfg));
        assert!(desktop_provider_configured(&cfg));

        // The token is required for the platform route (login is gone).
        assert!(
            apply_provider_route(
                &mut cfg,
                "platform",
                Some("https://another-platform.example.com"),
                None,
                None,
                None,
            )
            .is_err()
        );
        // A changed platform URL drops the previous token.
        cfg.platform.api_token = "stale".into();
        apply_provider_route(
            &mut cfg,
            "platform",
            Some("https://another-platform.example.com"),
            None,
            None,
            Some("mpat-2"),
        )
        .unwrap();
        assert_eq!(
            cfg.platform.base_url,
            "https://another-platform.example.com"
        );
        assert_eq!(cfg.platform.api_token, "mpat-2");
        assert_eq!(
            cfg.base_url,
            "https://another-platform.example.com/apps/llm/api/v1"
        );
    }

    #[tokio::test]
    async fn task_listing_does_not_wait_for_the_active_session_lock() {
        let registry = Arc::new(craftsman_harness::AgentRegistry::default());
        registry.insert("worker", craftsman_harness::AgentStatus::Running);
        let runner = DesktopRunner {
            inner: tokio::sync::Mutex::new(RunnerInner {
                cfg: Config::default(),
                session: None,
                pending_thread_id: None,
                general_workspace: std::env::current_dir().unwrap(),
                workspace: std::env::current_dir().unwrap(),
                project_contexts: Vec::new(),
            }),
            approval: Arc::new(Mutex::new(ApprovalState::default())),
            bridge: Arc::new(DesktopBridge::new()),
            agent_handle: Mutex::new(None),
            registry_handle: Mutex::new(Some(registry)),
            voice: DesktopVoiceRuntime::new(&Config::default(), Arc::new(DesktopBridge::new())),
            initialized: AtomicBool::new(true),
            initialized_notify: tokio::sync::Notify::new(),
        };
        let _session_guard = runner.inner.lock().await;
        let tasks = tokio::time::timeout(std::time::Duration::from_millis(50), runner.task_list())
            .await
            .expect("task listing must remain live during a running turn")
            .unwrap();
        assert_eq!(tasks[0]["name"], "worker");
        assert_eq!(tasks[0]["status"], "running");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn websocket_protocol_is_duplex_for_hitl_questions_and_threads() {
        let url = spawn_stub_server().await;
        let (mut socket, _) = tokio_tungstenite::connect_async(url.clone()).await.unwrap();

        let hello = recv_json(&mut socket).await;
        assert_eq!(hello["protocol"], PROTOCOL_VERSION);
        assert_eq!(hello["thread_id"], "t-main");
        let snapshot = recv_json(&mut socket).await;
        assert_eq!(snapshot["type"], "state_snapshot");
        assert_eq!(snapshot["state"]["agent_ready"], true);
        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                json!({"type":"get_state", "operation_id":"state-1"})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        let refreshed = recv_json(&mut socket).await;
        assert_eq!(refreshed["type"], "state_snapshot");
        assert_eq!(refreshed["operation_id"], "state-1");

        let (mut duplicate, _) = tokio_tungstenite::connect_async(url).await.unwrap();
        let duplicate_error = recv_json(&mut duplicate).await;
        assert_eq!(duplicate_error["type"], "operation_error");
        assert_eq!(duplicate_error["code"], "client_already_connected");
        assert!(
            duplicate_error["message"]
                .as_str()
                .unwrap()
                .contains("already connected")
        );

        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                json!({"type": "message", "content": "permission"})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        assert_eq!(recv_json(&mut socket).await["type"], "turn_started");
        assert_eq!(recv_json(&mut socket).await["type"], "activity");
        let permission = recv_json(&mut socket).await;
        assert_eq!(permission["type"], "permission_request");
        assert_eq!(permission["details"], "command: cargo test");
        assert_eq!(permission["approve_always_scope"], "desktop_session");
        assert!(permission["turn_id"].as_str().is_some());
        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                json!({"type": "steer", "content": "focus on tests"})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        let steered = recv_json(&mut socket).await;
        assert_eq!(steered["type"], "steer");
        assert_eq!(steered["accepted"], true);
        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                json!({
                    "type": "permission_response",
                    "request_id": permission["request_id"],
                    "decision": "approve_once",
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        assert_eq!(recv_json(&mut socket).await["text"], "approved once");
        assert_eq!(recv_json(&mut socket).await["type"], "done");

        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                json!({"type": "message", "content": "question"})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        assert_eq!(recv_json(&mut socket).await["type"], "turn_started");
        assert_eq!(recv_json(&mut socket).await["type"], "activity");
        let question = recv_json(&mut socket).await;
        assert_eq!(question["type"], "ask_user");
        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                json!({
                    "type": "ask_user_response",
                    "request_id": question["request_id"],
                    "answer": "new module",
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        assert_eq!(recv_json(&mut socket).await["text"], "new module");
        assert_eq!(recv_json(&mut socket).await["type"], "done");

        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                json!({"type": "message", "content": "question"})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        assert_eq!(recv_json(&mut socket).await["type"], "turn_started");
        assert_eq!(recv_json(&mut socket).await["type"], "activity");
        assert_eq!(recv_json(&mut socket).await["type"], "ask_user");
        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                json!({"type": "cancel"}).to_string().into(),
            ))
            .await
            .unwrap();
        let mut saw_cancel = false;
        let mut saw_done = false;
        for _ in 0..3 {
            let frame = recv_json(&mut socket).await;
            saw_cancel |= frame["type"] == "cancel" && frame["accepted"] == true;
            saw_done |= frame["type"] == "done";
            if saw_cancel && saw_done {
                break;
            }
        }
        assert!(saw_cancel);
        assert!(saw_done);

        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                json!({"type": "list_threads", "offset": 0, "limit": 10})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        let threads = recv_json(&mut socket).await;
        assert_eq!(threads["type"], "threads");
        assert_eq!(threads["offset"], 0);
        assert_eq!(threads["has_more"], false);
        assert_eq!(threads["next_offset"], Value::Null);
        assert_eq!(threads["threads"][0]["title"], "Main thread");

        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                json!({
                    "type": "set_thread_contexts",
                    "operation_id": "contexts-1",
                    "thread_id": "t-main",
                    "project_contexts": ["/repo/api", "/repo/web"]
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        let contexts = recv_json(&mut socket).await;
        assert_eq!(contexts["type"], "thread_contexts");
        assert_eq!(contexts["operation_id"], "contexts-1");
        assert_eq!(contexts["thread_id"], "t-main");
        assert_eq!(
            contexts["project_contexts"],
            json!(["/repo/api", "/repo/web"])
        );

        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                json!({"type": "list_models"}).to_string().into(),
            ))
            .await
            .unwrap();
        let models = recv_json(&mut socket).await;
        assert_eq!(models["type"], "models");
        assert_eq!(models["models"][1], "other-model");

        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                json!({"type": "get_voice_status", "operation_id": "voice-status-1"})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        let voice = recv_json(&mut socket).await;
        assert_eq!(voice["type"], "voice_status");
        assert_eq!(voice["operation_id"], "voice-status-1");
        assert_eq!(voice["voice"]["available"], false);

        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                json!({"type": "voice_start", "operation_id": "voice-start-1"})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        let voice_error = recv_json(&mut socket).await;
        assert_eq!(voice_error["type"], "operation_error");
        assert_eq!(voice_error["operation_id"], "voice-start-1");
        assert_eq!(voice_error["code"], "voice_start_failed");
        assert_eq!(voice_error["scope"], "voice");

        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                json!({"type": "new_thread"}).to_string().into(),
            ))
            .await
            .unwrap();
        let created = recv_json(&mut socket).await;
        assert_eq!(created["action"], "created");
        assert_eq!(created["thread_id"], "t-new");
        assert_eq!(recv_json(&mut socket).await["type"], "threads");

        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                json!({"type": "duplicate_thread", "thread_id": "t-main", "operation_id": "dup-1"})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        let duplicated = recv_json(&mut socket).await;
        assert_eq!(duplicated["action"], "duplicated");
        assert_eq!(duplicated["thread_id"], "t-copy");
        assert_eq!(duplicated["operation_id"], "dup-1");
        assert_eq!(recv_json(&mut socket).await["type"], "threads");

        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                json!({"type": "delete_thread", "thread_id": "t-new"})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        let deleted = recv_json(&mut socket).await;
        assert_eq!(deleted["action"], "deleted");
        assert_eq!(deleted["current_thread_id"], "t-main");
        assert_eq!(recv_json(&mut socket).await["type"], "threads");
    }
}
