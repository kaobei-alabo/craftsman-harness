//! Local microphone capture and Whisper transcription for the terminal TUI.
//!
//! This module is compiled only for the default-on `voice` feature on macOS
//! and Linux. It owns native audio/model work and communicates with the TUI
//! exclusively through [`VoiceEvent`] values.

use crate::config::{self, Config};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample};
use futures_util::{FutureExt, StreamExt};
use rtrb::{Consumer, Producer, RingBuffer};
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, mpsc};
use unicode_segmentation::UnicodeSegmentation;
use voice_activity_detector::VoiceActivityDetector;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

const SAMPLE_RATE: usize = 16_000;
const VAD_CHUNK: usize = 512;
const DEFAULT_VAD_THRESHOLD: f32 = 0.35;
const MIN_VAD_THRESHOLD: f32 = 0.05;
const MAX_VAD_THRESHOLD: f32 = 0.95;
const ENERGY_ATTACK_CHUNKS: usize = 4;
const ENERGY_RELEASE_CHUNKS: usize = 6;
const MIN_ENERGY_TRIGGER_RMS: f32 = 0.001;
const MIN_ENERGY_RELEASE_RMS: f32 = 0.0005;
const MIN_EXPLICIT_AUDIO_RMS: f32 = 0.0005;
const MIN_EXPLICIT_AUDIO_SAMPLES: usize = SAMPLE_RATE / 4;
const PARTIAL_INTERVAL: Duration = Duration::from_millis(1_000);
const LEVEL_INTERVAL: Duration = Duration::from_millis(100);
const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_MAX_SESSION: Duration = Duration::from_secs(10 * 60);
const MODEL_REVISION: &str = "5359861c739e955e79d9a303bcbc70fb988958b1";
const MODEL_BASE_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve";

#[derive(Clone, Debug, PartialEq)]
pub struct VoiceConfig {
    pub enabled: bool,
    pub allow_model_download: bool,
    pub engine: String,
    pub model: String,
    pub language: Option<String>,
    pub auto_submit: bool,
    pub min_words_to_submit: usize,
    pub partial_transcripts: bool,
    pub vad_threshold: f32,
    pub vad_silence_ms: u64,
    pub idle_timeout_ms: u64,
    pub max_session_minutes: u64,
}

impl VoiceConfig {
    pub fn from_config(cfg: &Config) -> Self {
        let string = |key: &str| {
            cfg.voice_setting(key)
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        };
        let language = string("language").and_then(|value| {
            (!value.eq_ignore_ascii_case("auto")).then_some(value.to_ascii_lowercase())
        });
        Self {
            allow_model_download: cfg
                .voice_setting("allow_model_download")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            enabled: cfg
                .voice_setting("enabled")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true),
            engine: string("engine").unwrap_or_else(|| "whisper".to_string()),
            model: canonical_model_name(
                string("model").as_deref().unwrap_or("large-v3-turbo-q5_0"),
            )
            .unwrap_or("large-v3-turbo-q5_0")
            .to_string(),
            language,
            auto_submit: cfg
                .voice_setting("auto_submit")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            min_words_to_submit: cfg
                .voice_setting("min_words_to_submit")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(3)
                .clamp(1, 100) as usize,
            partial_transcripts: cfg
                .voice_setting("partial_transcripts")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true),
            vad_threshold: cfg
                .voice_setting("vad_threshold")
                .and_then(serde_json::Value::as_f64)
                .unwrap_or(f64::from(DEFAULT_VAD_THRESHOLD))
                .clamp(f64::from(MIN_VAD_THRESHOLD), f64::from(MAX_VAD_THRESHOLD))
                as f32,
            vad_silence_ms: cfg
                .voice_setting("segment_silence_ms")
                .or_else(|| cfg.voice_setting("vad_silence_ms"))
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(800)
                .clamp(250, 5_000),
            idle_timeout_ms: cfg
                .voice_setting("idle_timeout_ms")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(DEFAULT_IDLE_TIMEOUT.as_millis() as u64)
                .clamp(5_000, 300_000),
            max_session_minutes: cfg
                .voice_setting("max_session_minutes")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(DEFAULT_MAX_SESSION.as_secs() / 60)
                .clamp(1, 60),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ModelSpec {
    pub name: &'static str,
    pub filename: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
}

const MODELS: [ModelSpec; 3] = [
    ModelSpec {
        name: "large-v3-turbo-q5_0",
        filename: "ggml-large-v3-turbo-q5_0.bin",
        bytes: 574_041_195,
        sha256: "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2",
    },
    ModelSpec {
        name: "small-q5_1",
        filename: "ggml-small-q5_1.bin",
        bytes: 190_085_487,
        sha256: "ae85e4a935d7a567bd102fe55afc16bb595bdb618e11b2fc7591bc08120411bb",
    },
    ModelSpec {
        name: "base-q5_1",
        filename: "ggml-base-q5_1.bin",
        bytes: 59_707_625,
        sha256: "422f1ae452ade6f30a004d7e5c6a43195e4433bc370bf23fac9cc591f01a8898",
    },
];

pub fn model_names() -> impl Iterator<Item = &'static str> {
    MODELS.iter().map(|model| model.name)
}

pub fn canonical_model_name(name: &str) -> Option<&'static str> {
    let normalized = name.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "large" | "large-v3-turbo" | "large-v3-turbo-q5_0" => Some("large-v3-turbo-q5_0"),
        "small" | "small-q5_0" | "small-q5_1" => Some("small-q5_1"),
        "base" | "base-q5_0" | "base-q5_1" => Some("base-q5_1"),
        _ => None,
    }
}

fn model_spec(name: &str) -> Option<ModelSpec> {
    let canonical = canonical_model_name(name)?;
    MODELS.iter().copied().find(|model| model.name == canonical)
}

pub fn model_path(name: &str) -> Option<PathBuf> {
    model_spec(name).map(|model| config::config_dir().join("models").join(model.filename))
}

pub fn model_downloaded(name: &str) -> bool {
    let Some(spec) = model_spec(name) else {
        return false;
    };
    model_path(name)
        .and_then(|path| std::fs::metadata(path).ok())
        .is_some_and(|metadata| metadata.len() == spec.bytes)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VoiceState {
    Idle,
    Initializing,
    Downloading,
    Recording,
    Finalizing,
    Paused,
}

#[derive(Clone, Debug)]
pub enum VoiceEvent {
    State(VoiceState),
    DownloadProgress {
        downloaded: u64,
        total: u64,
    },
    Level(f32),
    SpeechStarted,
    Partial {
        committed: String,
        tentative: String,
    },
    SegmentFinal {
        text: String,
        committed: String,
        partials_enabled: bool,
    },
    SegmentCancelled,
    SessionFinished {
        dictated_text: String,
    },
    Error(VoiceError),
}

#[derive(Clone, Debug, thiserror::Error)]
pub enum VoiceError {
    #[error("microphone permission denied")]
    MicPermissionDenied,
    #[error("no audio captured")]
    NoAudioCaptured,
    #[error("no speech detected")]
    NoSpeechDetected,
    #[error("voice engine failed: {0}")]
    EngineFailed(String),
}

impl VoiceError {
    pub fn user_message(&self, model: &str) -> String {
        match self {
            Self::MicPermissionDenied => "Microphone access denied. Grant it to your terminal in System Settings → Privacy & Security → Microphone (macOS attributes the mic to Terminal.app/iTerm2). VS Code's integrated terminal may fail to prompt — run craftsman from a real terminal.".to_string(),
            Self::NoAudioCaptured => "No audio detected from the microphone — is it muted or is another app holding it?".to_string(),
            Self::NoSpeechDetected => "Heard audio but no speech was recognized.".to_string(),
            Self::EngineFailed(detail) => format!(
                "Voice transcription failed: {detail}. Model file: {}",
                model_path(model)
                    .unwrap_or_else(|| config::config_dir().join("models"))
                    .display()
            ),
        }
    }
}

#[derive(Clone, Debug, Default)]
#[allow(dead_code)]
pub struct Transcript {
    pub text: String,
    pub segments: Vec<TranscriptSegment>,
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct TranscriptSegment {
    pub text: String,
    pub start_ms: u64,
    pub end_ms: u64,
}

struct WhisperEngine {
    context: Arc<WhisperContext>,
    partials: bool,
    transcribe_gate: Mutex<()>,
}

impl WhisperEngine {
    async fn load(path: PathBuf, partials_requested: bool) -> anyhow::Result<Self> {
        // whisper.cpp and ggml otherwise write initialization/Metal diagnostics
        // directly to stderr, bypassing Ratatui's single terminal owner and
        // corrupting the alternate-screen display. With no logging backend
        // feature enabled these hooks intentionally consume the native logs.
        whisper_rs::install_logging_hooks();
        let context = tokio::task::spawn_blocking(move || {
            WhisperContext::new_with_params(path, WhisperContextParameters::default())
                .map_err(|error| anyhow::anyhow!(error.to_string()))
        })
        .await
        .map_err(|error| anyhow::anyhow!("Whisper loader task failed: {error}"))??;
        let engine = Self {
            context: Arc::new(context),
            // Partial transcription is explicitly user-configured. A single
            // in-flight gate below already prevents slow models from building
            // an inference backlog, so a startup speed benchmark must not
            // silently disable real-time feedback.
            partials: partials_requested,
            transcribe_gate: Mutex::new(()),
        };
        Ok(engine)
    }

    fn transcribe_blocking(
        context: Arc<WhisperContext>,
        audio: Vec<f32>,
        language: Option<String>,
        abort: Arc<AtomicBool>,
    ) -> anyhow::Result<Transcript> {
        if audio.is_empty() {
            anyhow::bail!("empty audio buffer");
        }
        let mut state = context
            .create_state()
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_n_threads(
            std::thread::available_parallelism()
                .map(|threads| threads.get().min(8) as i32)
                .unwrap_or(4),
        );
        params.set_translate(false);
        params.set_no_context(true);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_suppress_blank(true);
        params.set_suppress_nst(true);
        params.set_language(language.as_deref());
        unsafe extern "C" fn abort_callback(data: *mut std::ffi::c_void) -> bool {
            // SAFETY: `data` points to the AtomicBool owned by the Arc kept alive
            // in `transcribe_blocking` for the entire synchronous `state.full` call.
            let abort = unsafe { &*(data as *const AtomicBool) };
            abort.load(Ordering::Relaxed)
        }
        unsafe {
            params.set_abort_callback(Some(abort_callback));
            params.set_abort_callback_user_data(Arc::as_ptr(&abort) as *mut std::ffi::c_void);
        }
        state
            .full(params, &audio)
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        let mut text = String::new();
        let mut segments = Vec::new();
        for segment in state.as_iter() {
            let segment_text = segment.to_string();
            text.push_str(&segment_text);
            segments.push(TranscriptSegment {
                text: segment_text,
                start_ms: segment.start_timestamp().max(0) as u64 * 10,
                end_ms: segment.end_timestamp().max(0) as u64 * 10,
            });
        }
        Ok(Transcript {
            text: normalize_transcript(&text),
            segments,
        })
    }

    async fn transcribe_with_abort(
        &self,
        audio: &[f32],
        lang: Option<&str>,
        abort: Arc<AtomicBool>,
    ) -> anyhow::Result<Transcript> {
        if abort.load(Ordering::Relaxed) {
            anyhow::bail!("voice transcription cancelled");
        }
        let _guard = self.transcribe_gate.lock().await;
        if abort.load(Ordering::Relaxed) {
            anyhow::bail!("voice transcription cancelled");
        }
        let context = self.context.clone();
        let audio = audio.to_vec();
        let language = lang.map(str::to_string);
        tokio::task::spawn_blocking(move || {
            Self::transcribe_blocking(context, audio, language, abort)
        })
        .await
        .map_err(|error| anyhow::anyhow!("Whisper worker failed: {error}"))?
    }

    fn supports_partials(&self) -> bool {
        self.partials
    }
}

fn normalize_transcript(text: &str) -> String {
    text.replace("[_BEG_]", "")
        .replace("[BLANK_AUDIO]", "")
        .trim()
        .to_string()
}

#[derive(Debug)]
enum ControllerCommand {
    Start,
    StopAndFinalize,
    CancelSegment,
    Cancel,
    Shutdown,
}

#[derive(Debug)]
enum SessionCommand {
    StopAndFinalize,
    CancelSegment,
}

#[derive(Debug)]
enum SessionOutcome {
    Success,
    Failed,
}

pub struct VoiceController {
    tx: mpsc::UnboundedSender<ControllerCommand>,
    join: tokio::task::JoinHandle<()>,
}

impl VoiceController {
    pub fn spawn(config: VoiceConfig, events: mpsc::UnboundedSender<VoiceEvent>) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let join = tokio::spawn(controller_loop(config, events, rx));
        Self { tx, join }
    }

    pub fn start(&self) {
        let _ = self.tx.send(ControllerCommand::Start);
    }

    pub fn stop_and_finalize(&self) {
        let _ = self.tx.send(ControllerCommand::StopAndFinalize);
    }

    pub fn cancel_segment(&self) {
        let _ = self.tx.send(ControllerCommand::CancelSegment);
    }

    pub fn cancel(&self) {
        let _ = self.tx.send(ControllerCommand::Cancel);
    }

    pub async fn shutdown(self) {
        let _ = self.tx.send(ControllerCommand::Shutdown);
        // The controller handles shutdown independently of capture/transcription:
        // it sets the native abort flag, aborts the async session (dropping CPAL),
        // and then drops the cached Whisper context before this join completes.
        let _ = self.join.await;
    }
}

async fn controller_loop(
    config: VoiceConfig,
    events: mpsc::UnboundedSender<VoiceEvent>,
    mut commands: mpsc::UnboundedReceiver<ControllerCommand>,
) {
    let engine_cache: Arc<Mutex<Option<Arc<WhisperEngine>>>> = Arc::new(Mutex::new(None));
    let (outcome_tx, mut outcome_rx) = mpsc::unbounded_channel();
    let mut active: Option<(
        mpsc::UnboundedSender<SessionCommand>,
        tokio::task::JoinHandle<()>,
        Arc<AtomicBool>,
    )> = None;
    let mut consecutive_failures = 0usize;

    loop {
        tokio::select! {
            Some(command) = commands.recv() => match command {
                ControllerCommand::Start if active.is_none() => {
                    if consecutive_failures >= 3 {
                        let _ = events.send(VoiceEvent::State(VoiceState::Paused));
                        let _ = events.send(VoiceEvent::Error(VoiceError::EngineFailed(
                            "voice paused after three consecutive failures; run /voice on to retry".to_string()
                        )));
                        continue;
                    }
                    let (session_tx, session_rx) = mpsc::unbounded_channel();
                    let config = config.clone();
                    let events = events.clone();
                    let outcomes = outcome_tx.clone();
                    let cache = engine_cache.clone();
                    let abort = Arc::new(AtomicBool::new(false));
                    let session_abort = abort.clone();
                    let join = tokio::spawn(async move {
                        let event_sink = events.clone();
                        let outcome = match std::panic::AssertUnwindSafe(run_session(
                            config,
                            events,
                            session_rx,
                            cache,
                            session_abort,
                        ))
                        .catch_unwind()
                        .await
                        {
                            Ok(outcome) => outcome,
                            Err(_) => {
                                let _ = event_sink.send(VoiceEvent::Error(VoiceError::EngineFailed(
                                    "voice worker terminated unexpectedly".to_string(),
                                )));
                                SessionOutcome::Failed
                            }
                        };
                        let _ = outcomes.send(outcome);
                    });
                    active = Some((session_tx, join, abort));
                }
                ControllerCommand::StopAndFinalize => {
                    if let Some((tx, _, _)) = &active {
                        let _ = tx.send(SessionCommand::StopAndFinalize);
                    }
                }
                ControllerCommand::CancelSegment => {
                    if let Some((tx, _, _)) = &active {
                        let _ = tx.send(SessionCommand::CancelSegment);
                    }
                }
                ControllerCommand::Cancel => {
                    if let Some((_, join, abort)) = active.take() {
                        abort.store(true, Ordering::Relaxed);
                        join.abort();
                        let _ = join.await;
                    }
                    let _ = events.send(VoiceEvent::State(VoiceState::Idle));
                }
                ControllerCommand::Shutdown => {
                    if let Some((_, join, abort)) = active.take() {
                        abort.store(true, Ordering::Relaxed);
                        join.abort();
                        let _ = join.await;
                    }
                    break;
                }
                ControllerCommand::Start => {}
            },
            Some(outcome) = outcome_rx.recv() => {
                if let Some((_, join, _)) = active.take() {
                    let _ = join.await;
                }
                match outcome {
                    SessionOutcome::Success => consecutive_failures = 0,
                    SessionOutcome::Failed => consecutive_failures += 1,
                }
                let _ = events.send(VoiceEvent::State(if consecutive_failures >= 3 {
                    VoiceState::Paused
                } else {
                    VoiceState::Idle
                }));
            }
            else => break,
        }
    }
}

async fn ensure_model(
    spec: ModelSpec,
    events: &mpsc::UnboundedSender<VoiceEvent>,
    allow_model_download: bool,
) -> anyhow::Result<PathBuf> {
    let dir = config::config_dir().join("models");
    ensure_model_in(&dir, spec, events, allow_model_download).await
}

async fn ensure_model_in(
    dir: &Path,
    spec: ModelSpec,
    events: &mpsc::UnboundedSender<VoiceEvent>,
    allow_model_download: bool,
) -> anyhow::Result<PathBuf> {
    tokio::fs::create_dir_all(&dir).await?;
    let final_path = dir.join(spec.filename);
    if tokio::fs::metadata(&final_path)
        .await
        .is_ok_and(|metadata| metadata.len() == spec.bytes)
    {
        return Ok(final_path);
    }

    let partial_path = dir.join(format!("{}.part", spec.filename));
    if tokio::fs::metadata(&partial_path)
        .await
        .is_ok_and(|metadata| metadata.len() == spec.bytes)
    {
        let candidate = partial_path.clone();
        let actual = tokio::task::spawn_blocking(move || sha256_file(&candidate))
            .await
            .map_err(|error| anyhow::anyhow!("model hash task failed: {error}"))??;
        if actual == spec.sha256 {
            if final_path.exists() {
                let _ = std::fs::remove_file(&final_path);
            }
            std::fs::rename(&partial_path, &final_path)?;
            return Ok(final_path);
        }
        let _ = tokio::fs::remove_file(&partial_path).await;
    }
    anyhow::ensure!(
        allow_model_download,
        "The local voice model is missing. Set voice.allow_model_download to true in config.json to download it."
    );
    let mut offset = tokio::fs::metadata(&partial_path)
        .await
        .map(|metadata| metadata.len().min(spec.bytes))
        .unwrap_or(0);
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .timeout(Duration::from_secs(30 * 60))
        .build()?;
    let url = format!(
        "{MODEL_BASE_URL}/{MODEL_REVISION}/{}?download=true",
        spec.filename
    );
    let mut request = client.get(url);
    if offset > 0 {
        request = request.header(reqwest::header::RANGE, format!("bytes={offset}-"));
    }
    let response = request.send().await?.error_for_status()?;
    let append = offset > 0 && response.status() == reqwest::StatusCode::PARTIAL_CONTENT;
    if !append {
        offset = 0;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(!append)
        .open(&partial_path)?;
    if append {
        file.seek(SeekFrom::End(0))?;
    }
    let _ = events.send(VoiceEvent::State(VoiceState::Downloading));
    let _ = events.send(VoiceEvent::DownloadProgress {
        downloaded: offset,
        total: spec.bytes,
    });
    let mut downloaded = offset;
    let mut last_progress = Instant::now();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        file.write_all(&chunk)?;
        downloaded = downloaded.saturating_add(chunk.len() as u64);
        if last_progress.elapsed() >= Duration::from_millis(100) || downloaded >= spec.bytes {
            let _ = events.send(VoiceEvent::DownloadProgress {
                downloaded,
                total: spec.bytes,
            });
            last_progress = Instant::now();
        }
    }
    file.sync_all()?;
    drop(file);
    if downloaded != spec.bytes {
        anyhow::bail!(
            "model download incomplete: expected {} bytes, received {}",
            spec.bytes,
            downloaded
        );
    }
    let partial_for_hash = partial_path.clone();
    let actual = tokio::task::spawn_blocking(move || sha256_file(&partial_for_hash))
        .await
        .map_err(|error| anyhow::anyhow!("model hash task failed: {error}"))??;
    if actual != spec.sha256 {
        let _ = std::fs::remove_file(&partial_path);
        anyhow::bail!(
            "model checksum mismatch: expected {}, got {}",
            spec.sha256,
            actual
        );
    }
    if final_path.exists() {
        let _ = std::fs::remove_file(&final_path);
    }
    std::fs::rename(&partial_path, &final_path)?;
    Ok(final_path)
}

fn sha256_file(path: &Path) -> anyhow::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

async fn get_engine(
    config: &VoiceConfig,
    events: &mpsc::UnboundedSender<VoiceEvent>,
    cache: &Arc<Mutex<Option<Arc<WhisperEngine>>>>,
) -> anyhow::Result<Arc<WhisperEngine>> {
    if !config.engine.eq_ignore_ascii_case("whisper") {
        anyhow::bail!("unsupported voice engine {}", config.engine);
    }
    let mut guard = cache.lock().await;
    if let Some(engine) = guard.as_ref() {
        return Ok(engine.clone());
    }
    let spec = model_spec(&config.model)
        .ok_or_else(|| anyhow::anyhow!("unknown voice model {}", config.model))?;
    let path = ensure_model(spec, events, config.allow_model_download).await?;
    let _ = events.send(VoiceEvent::State(VoiceState::Initializing));
    let engine = Arc::new(WhisperEngine::load(path, config.partial_transcripts).await?);
    *guard = Some(engine.clone());
    Ok(engine)
}

struct Capture {
    _stream: cpal::Stream,
    consumer: Consumer<f32>,
    sample_rate: u32,
    errors: mpsc::UnboundedReceiver<String>,
}

fn open_capture() -> anyhow::Result<Capture> {
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or_else(|| anyhow::anyhow!("no default microphone input device"))?;
    let supported = device.default_input_config()?;
    let sample_rate = supported.sample_rate();
    let channels = usize::from(supported.channels());
    let stream_config = supported.config();
    let (error_tx, error_rx) = mpsc::unbounded_channel();
    let capacity = sample_rate as usize * 4;

    macro_rules! build {
        ($sample:ty) => {{
            build_capture_stream::<$sample>(
                &device,
                &stream_config,
                channels,
                capacity,
                error_tx.clone(),
            )?
        }};
    }

    let (stream, consumer) = match supported.sample_format() {
        SampleFormat::I8 => build!(i8),
        SampleFormat::I16 => build!(i16),
        SampleFormat::I32 => build!(i32),
        SampleFormat::I64 => build!(i64),
        SampleFormat::U8 => build!(u8),
        SampleFormat::U16 => build!(u16),
        SampleFormat::U32 => build!(u32),
        SampleFormat::U64 => build!(u64),
        SampleFormat::F32 => build!(f32),
        SampleFormat::F64 => build!(f64),
        other => anyhow::bail!("unsupported microphone sample format {other}"),
    };
    stream.play()?;
    Ok(Capture {
        _stream: stream,
        consumer,
        sample_rate,
        errors: error_rx,
    })
}

fn build_capture_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    channels: usize,
    capacity: usize,
    error_tx: mpsc::UnboundedSender<String>,
) -> anyhow::Result<(cpal::Stream, Consumer<f32>)>
where
    T: SizedSample + Copy,
    f32: FromSample<T>,
{
    let (mut producer, consumer) = RingBuffer::<f32>::new(capacity.max(1));
    let stream = device.build_input_stream(
        *config,
        move |data: &[T], _| push_input(data, channels, &mut producer),
        move |error| {
            let _ = error_tx.send(error.to_string());
        },
        None,
    )?;
    Ok((stream, consumer))
}

fn push_input<T>(data: &[T], channels: usize, producer: &mut Producer<f32>)
where
    T: Copy,
    f32: FromSample<T>,
{
    let channels = channels.max(1);
    for frame in data.chunks(channels) {
        let mono =
            frame.iter().copied().map(f32::from_sample).sum::<f32>() / frame.len().max(1) as f32;
        if producer.push(mono).is_err() {
            break;
        }
    }
}

#[derive(Debug)]
struct LinearResampler {
    ratio: f64,
    position: f64,
    buffer: Vec<f32>,
}

impl LinearResampler {
    fn new(input_rate: u32) -> Self {
        Self {
            ratio: f64::from(input_rate) / SAMPLE_RATE as f64,
            position: 0.0,
            buffer: Vec::new(),
        }
    }

    fn process(&mut self, input: &[f32], output: &mut Vec<f32>) {
        if input.is_empty() {
            return;
        }
        self.buffer.extend_from_slice(input);
        while self.position + 1.0 < self.buffer.len() as f64 {
            let left = self.position.floor() as usize;
            let fraction = (self.position - left as f64) as f32;
            let sample = self.buffer[left] * (1.0 - fraction) + self.buffer[left + 1] * fraction;
            output.push(sample);
            self.position += self.ratio;
        }
        let consumed = self.position.floor() as usize;
        if consumed > 0 {
            self.buffer.drain(..consumed.min(self.buffer.len()));
            self.position -= consumed as f64;
        }
    }
}

async fn run_session(
    config: VoiceConfig,
    events: mpsc::UnboundedSender<VoiceEvent>,
    mut commands: mpsc::UnboundedReceiver<SessionCommand>,
    engine_cache: Arc<Mutex<Option<Arc<WhisperEngine>>>>,
    abort: Arc<AtomicBool>,
) -> SessionOutcome {
    let _ = events.send(VoiceEvent::State(VoiceState::Initializing));
    let engine = match get_engine(&config, &events, &engine_cache).await {
        Ok(engine) => engine,
        Err(error) => {
            let _ = events.send(VoiceEvent::Error(VoiceError::EngineFailed(
                error.to_string(),
            )));
            return SessionOutcome::Failed;
        }
    };
    let vad = match tokio::task::spawn_blocking(|| {
        VoiceActivityDetector::builder()
            .sample_rate(SAMPLE_RATE as i64)
            .chunk_size(VAD_CHUNK)
            .build()
    })
    .await
    {
        Ok(Ok(vad)) => vad,
        Ok(Err(error)) => {
            let _ = events.send(VoiceEvent::Error(VoiceError::EngineFailed(
                error.to_string(),
            )));
            return SessionOutcome::Failed;
        }
        Err(error) => {
            let _ = events.send(VoiceEvent::Error(VoiceError::EngineFailed(format!(
                "VAD initialization failed: {error}"
            ))));
            return SessionOutcome::Failed;
        }
    };
    let mut capture = match open_capture() {
        Ok(capture) => capture,
        Err(error) => {
            let mapped = map_capture_error(&error.to_string());
            let _ = events.send(VoiceEvent::Error(mapped));
            return SessionOutcome::Failed;
        }
    };
    let _ = events.send(VoiceEvent::State(VoiceState::Recording));

    let partials_enabled = config.partial_transcripts && engine.supports_partials();
    let mut processor = RecordingProcessor::new(
        capture.sample_rate,
        vad,
        config.vad_threshold,
        config.vad_silence_ms,
    );
    let session_started = Instant::now();
    let idle_timeout = Duration::from_millis(config.idle_timeout_ms);
    let max_session = Duration::from_secs(config.max_session_minutes.saturating_mul(60));
    let mut last_speech_activity = Instant::now();
    let mut last_partial = Instant::now();
    let mut last_level = Instant::now();
    let mut had_device_samples = false;
    let mut partial_inflight = false;
    let mut partial_sequence = 0u64;
    let mut session_text = String::new();
    let (partial_tx, mut partial_rx) =
        mpsc::unbounded_channel::<(u64, anyhow::Result<Transcript>)>();
    let mut end_reason = None;
    let mut poll = tokio::time::interval(Duration::from_millis(10));
    poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    while end_reason.is_none() {
        tokio::select! {
            Some(command) = commands.recv() => match command {
                SessionCommand::StopAndFinalize => end_reason = Some(SessionEnd::Explicit),
                SessionCommand::CancelSegment => {
                    partial_sequence = partial_sequence.wrapping_add(1);
                    partial_inflight = false;
                    processor.reset_segment();
                    last_partial = Instant::now();
                    last_speech_activity = Instant::now();
                    let _ = events.send(VoiceEvent::Level(0.0));
                    let _ = events.send(VoiceEvent::SegmentCancelled);
                    let _ = events.send(VoiceEvent::State(VoiceState::Recording));
                }
            },
            Some((sequence, result)) = partial_rx.recv() => {
                if sequence == partial_sequence {
                    partial_inflight = false;
                    if let Ok(transcript) = result {
                        if let Some((committed, tentative)) = processor.apply_partial(&transcript.text) {
                            let _ = events.send(VoiceEvent::Partial { committed, tentative });
                        }
                    }
                }
            }
            _ = poll.tick() => {
                if let Ok(error) = capture.errors.try_recv() {
                    let _ = events.send(VoiceEvent::Error(map_capture_error(&error)));
                    return SessionOutcome::Failed;
                }
                let mut device = Vec::with_capacity(capture.consumer.slots().min(8192));
                while let Ok(sample) = capture.consumer.pop() {
                    device.push(sample);
                    if device.len() >= 8192 {
                        break;
                    }
                }
                if !device.is_empty() {
                    had_device_samples = true;
                    let status = processor.push(&device);
                    if status.speech_detected {
                        last_speech_activity = Instant::now();
                    }
                    if status.started {
                        let _ = events.send(VoiceEvent::SpeechStarted);
                    }
                    if last_level.elapsed() >= LEVEL_INTERVAL {
                        let _ = events.send(VoiceEvent::Level(processor.take_level()));
                        last_level = Instant::now();
                    }
                }
                if processor.endpoint_reached() {
                    partial_sequence = partial_sequence.wrapping_add(1);
                    partial_inflight = false;
                    match finalize_segment(
                        &mut processor,
                        &engine,
                        &config,
                        &events,
                        &abort,
                        partials_enabled,
                    )
                    .await
                    {
                        Ok(Some(text)) => append_session_text(&mut session_text, &text),
                        Ok(None) => {
                            let _ = events.send(VoiceEvent::SegmentCancelled);
                        }
                        Err(error) => {
                            let _ = events.send(VoiceEvent::Error(error));
                            return SessionOutcome::Failed;
                        }
                    }
                    last_partial = Instant::now();
                    let _ = events.send(VoiceEvent::State(VoiceState::Recording));
                }
                if last_speech_activity.elapsed() >= idle_timeout {
                    end_reason = Some(SessionEnd::IdleTimeout);
                } else if session_started.elapsed() >= max_session {
                    end_reason = Some(SessionEnd::SessionCap);
                }
                if partials_enabled
                    && processor.has_partial_candidate()
                    && processor.audio().len() >= SAMPLE_RATE
                    && !partial_inflight
                    && last_partial.elapsed() >= PARTIAL_INTERVAL
                {
                    partial_sequence = partial_sequence.wrapping_add(1);
                    let sequence = partial_sequence;
                    let engine = engine.clone();
                    let language = config.language.clone();
                    let window_start = processor.audio().len().saturating_sub(SAMPLE_RATE * 12);
                    let mut audio = processor.audio()[window_start..].to_vec();
                    normalize_transcription_audio(&mut audio);
                    let tx = partial_tx.clone();
                    let transcribe_abort = abort.clone();
                    partial_inflight = true;
                    last_partial = Instant::now();
                    tokio::spawn(async move {
                        let result = engine
                            .transcribe_with_abort(&audio, language.as_deref(), transcribe_abort)
                            .await;
                        let _ = tx.send((sequence, result));
                    });
                }
            }
        }
    }

    drop(capture);
    if processor.speech_started()
        || (session_text.is_empty()
            && matches!(end_reason, Some(SessionEnd::Explicit))
            && processor.has_explicit_audio())
    {
        match finalize_segment(
            &mut processor,
            &engine,
            &config,
            &events,
            &abort,
            partials_enabled,
        )
        .await
        {
            Ok(Some(text)) => append_session_text(&mut session_text, &text),
            Ok(None) => {}
            Err(error) => {
                let _ = events.send(VoiceEvent::Error(error));
                return SessionOutcome::Failed;
            }
        }
    }

    let _ = events.send(VoiceEvent::Level(0.0));
    if session_text.is_empty() && !matches!(end_reason, Some(SessionEnd::Explicit)) {
        let _ = events.send(VoiceEvent::Error(if had_device_samples {
            VoiceError::NoSpeechDetected
        } else {
            VoiceError::NoAudioCaptured
        }));
        return SessionOutcome::Failed;
    }
    let _ = events.send(VoiceEvent::SessionFinished {
        dictated_text: session_text,
    });
    SessionOutcome::Success
}

#[derive(Clone, Copy, Debug)]
enum SessionEnd {
    Explicit,
    IdleTimeout,
    SessionCap,
}

async fn finalize_segment(
    processor: &mut RecordingProcessor,
    engine: &Arc<WhisperEngine>,
    config: &VoiceConfig,
    events: &mpsc::UnboundedSender<VoiceEvent>,
    abort: &Arc<AtomicBool>,
    partials_enabled: bool,
) -> Result<Option<String>, VoiceError> {
    let (mut audio, committed) = processor.take_segment();
    if audio.is_empty() {
        return Ok(None);
    }
    normalize_transcription_audio(&mut audio);
    let _ = events.send(VoiceEvent::State(VoiceState::Finalizing));
    let transcript = engine
        .transcribe_with_abort(&audio, config.language.as_deref(), abort.clone())
        .await
        .map_err(|error| VoiceError::EngineFailed(error.to_string()))?;
    if transcript.text.trim().is_empty() {
        return Ok(None);
    }
    let text = transcript.text;
    let _ = events.send(VoiceEvent::SegmentFinal {
        text: text.clone(),
        committed,
        partials_enabled,
    });
    Ok(Some(text))
}

fn append_session_text(session: &mut String, segment: &str) {
    let segment = segment.trim();
    if segment.is_empty() {
        return;
    }
    if !session.is_empty() && !session.ends_with(char::is_whitespace) {
        session.push(' ');
    }
    session.push_str(segment);
}

fn map_capture_error(detail: &str) -> VoiceError {
    let lower = detail.to_ascii_lowercase();
    if lower.contains("permission")
        || lower.contains("not permitted")
        || lower.contains("denied")
        || lower.contains("not authorized")
    {
        VoiceError::MicPermissionDenied
    } else {
        VoiceError::NoAudioCaptured
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct PushStatus {
    speech_detected: bool,
    started: bool,
}

#[derive(Debug)]
struct SpeechGate {
    vad_threshold: f32,
    noise_floor_rms: f32,
    energy_attack_chunks: usize,
    energy_release_chunks: usize,
    energy_active: bool,
    heard_candidate: bool,
}

impl SpeechGate {
    fn new(vad_threshold: f32) -> Self {
        Self {
            vad_threshold: vad_threshold.clamp(MIN_VAD_THRESHOLD, MAX_VAD_THRESHOLD),
            noise_floor_rms: MIN_ENERGY_RELEASE_RMS / 2.0,
            energy_attack_chunks: 0,
            energy_release_chunks: 0,
            energy_active: false,
            heard_candidate: false,
        }
    }

    fn observe(&mut self, probability: f32, rms: f32) -> bool {
        let probability = probability.clamp(0.0, 1.0);
        let rms = rms.max(0.0);
        if probability >= self.vad_threshold {
            self.energy_active = true;
            self.heard_candidate = true;
            self.energy_attack_chunks = ENERGY_ATTACK_CHUNKS;
            self.energy_release_chunks = 0;
            return true;
        }

        let fallback_probability = (self.vad_threshold * 0.05).clamp(0.015, 0.06);
        let trigger_rms = (self.noise_floor_rms * 3.0).max(MIN_ENERGY_TRIGGER_RMS);
        let release_rms = (self.noise_floor_rms * 1.8).max(MIN_ENERGY_RELEASE_RMS);

        if self.energy_active {
            if rms >= release_rms
                || probability >= (self.vad_threshold * 0.5).max(fallback_probability)
            {
                self.energy_release_chunks = 0;
                return true;
            }
            self.energy_release_chunks += 1;
            if self.energy_release_chunks < ENERGY_RELEASE_CHUNKS {
                return true;
            }
            self.energy_active = false;
            self.energy_attack_chunks = 0;
            self.energy_release_chunks = 0;
        }

        if probability >= fallback_probability && rms >= trigger_rms {
            self.heard_candidate = true;
            self.energy_attack_chunks += 1;
            if self.energy_attack_chunks >= ENERGY_ATTACK_CHUNKS {
                self.energy_active = true;
                self.energy_release_chunks = 0;
                return true;
            }
        } else {
            self.energy_attack_chunks = 0;
            self.adapt_noise_floor(rms);
        }
        false
    }

    fn reset_activity(&mut self) {
        self.energy_attack_chunks = 0;
        self.energy_release_chunks = 0;
        self.energy_active = false;
        self.heard_candidate = false;
    }

    fn adapt_noise_floor(&mut self, rms: f32) {
        let rms = rms.clamp(0.0, 0.02);
        self.noise_floor_rms = self.noise_floor_rms * 0.98 + rms * 0.02;
    }
}

struct RecordingProcessor {
    resampler: LinearResampler,
    vad: VoiceActivityDetector,
    speech_gate: SpeechGate,
    vad_buffer: Vec<f32>,
    audio: Vec<f32>,
    speech_started: bool,
    last_speech_sample: usize,
    vad_silence_samples: usize,
    level_sum: f64,
    level_count: usize,
    max_window_rms: f32,
    previous_candidate: String,
    committed: String,
}

impl RecordingProcessor {
    fn new(
        input_rate: u32,
        vad: VoiceActivityDetector,
        vad_threshold: f32,
        silence_ms: u64,
    ) -> Self {
        Self {
            resampler: LinearResampler::new(input_rate),
            vad,
            speech_gate: SpeechGate::new(vad_threshold),
            vad_buffer: Vec::with_capacity(VAD_CHUNK * 2),
            audio: Vec::new(),
            speech_started: false,
            last_speech_sample: 0,
            vad_silence_samples: (silence_ms as usize * SAMPLE_RATE) / 1_000,
            level_sum: 0.0,
            level_count: 0,
            max_window_rms: 0.0,
            previous_candidate: String::new(),
            committed: String::new(),
        }
    }

    fn push(&mut self, device: &[f32]) -> PushStatus {
        let was_started = self.speech_started;
        let start = self.audio.len();
        self.resampler.process(device, &mut self.audio);
        let fresh = &self.audio[start..];
        for sample in fresh {
            self.level_sum += f64::from(sample * sample);
            self.level_count += 1;
        }
        self.vad_buffer.extend_from_slice(fresh);
        let vad_buffer_start = self.audio.len().saturating_sub(self.vad_buffer.len());
        let mut consumed = 0usize;
        let mut speech_detected = false;
        while self.vad_buffer.len().saturating_sub(consumed) >= VAD_CHUNK {
            let chunk = &self.vad_buffer[consumed..consumed + VAD_CHUNK];
            let probability = self.vad.predict::<f32, _>(chunk.iter().copied());
            let rms = chunk_rms(chunk);
            self.max_window_rms = self.max_window_rms.max(rms);
            if self.speech_gate.observe(probability, rms) {
                speech_detected = true;
                self.speech_started = true;
                self.last_speech_sample = vad_buffer_start + consumed + VAD_CHUNK;
            }
            consumed += VAD_CHUNK;
        }
        if consumed > 0 {
            self.vad_buffer.drain(..consumed);
        }
        PushStatus {
            speech_detected,
            started: !was_started && self.speech_started,
        }
    }

    fn reset_segment(&mut self) {
        self.vad.reset();
        self.speech_gate.reset_activity();
        self.vad_buffer.clear();
        self.audio.clear();
        self.speech_started = false;
        self.last_speech_sample = 0;
        self.level_sum = 0.0;
        self.level_count = 0;
        self.max_window_rms = 0.0;
        self.previous_candidate.clear();
        self.committed.clear();
    }

    fn take_segment(&mut self) -> (Vec<f32>, String) {
        let audio = std::mem::take(&mut self.audio);
        let committed = std::mem::take(&mut self.committed);
        self.vad.reset();
        self.speech_gate.reset_activity();
        self.vad_buffer.clear();
        self.speech_started = false;
        self.last_speech_sample = 0;
        self.level_sum = 0.0;
        self.level_count = 0;
        self.max_window_rms = 0.0;
        self.previous_candidate.clear();
        (audio, committed)
    }

    fn take_level(&mut self) -> f32 {
        if self.level_count == 0 {
            return 0.0;
        }
        let rms = (self.level_sum / self.level_count as f64).sqrt() as f32;
        self.level_sum = 0.0;
        self.level_count = 0;
        (rms * 12.0).clamp(0.0, 1.0)
    }

    fn endpoint_reached(&self) -> bool {
        self.speech_started
            && self.audio.len().saturating_sub(self.last_speech_sample) >= self.vad_silence_samples
    }

    fn speech_started(&self) -> bool {
        self.speech_started
    }

    fn has_explicit_audio(&self) -> bool {
        self.audio.len() >= MIN_EXPLICIT_AUDIO_SAMPLES
            && self.max_window_rms >= MIN_EXPLICIT_AUDIO_RMS
            && self.speech_gate.heard_candidate
    }

    fn has_partial_candidate(&self) -> bool {
        self.speech_started || self.speech_gate.heard_candidate
    }

    fn audio(&self) -> &[f32] {
        &self.audio
    }

    fn apply_partial(&mut self, current: &str) -> Option<(String, String)> {
        let current = normalize_transcript(current);
        if current.is_empty() {
            return None;
        }
        let mut candidate = strip_committed_overlap(&self.committed, &current).to_string();
        if !self.previous_candidate.is_empty() {
            let agreed = stable_prefix(&self.previous_candidate, &candidate);
            if !agreed.is_empty() {
                self.committed.push_str(&agreed);
                candidate = candidate
                    .strip_prefix(&agreed)
                    .unwrap_or(candidate.as_str())
                    .to_string();
            }
        }
        self.previous_candidate = candidate.clone();
        Some((self.committed.clone(), candidate))
    }
}

fn chunk_rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum = samples
        .iter()
        .map(|sample| f64::from(sample * sample))
        .sum::<f64>();
    (sum / samples.len() as f64).sqrt() as f32
}

fn normalize_transcription_audio(audio: &mut [f32]) {
    let peak = audio
        .iter()
        .map(|sample| sample.abs())
        .fold(0.0_f32, f32::max);
    if peak <= 0.0 || peak >= 0.25 {
        return;
    }
    let gain = (0.25 / peak).clamp(1.0, 16.0);
    for sample in audio {
        *sample = (*sample * gain).clamp(-1.0, 1.0);
    }
}

fn strip_committed_overlap<'a>(committed: &str, current: &'a str) -> &'a str {
    if committed.is_empty() {
        return current;
    }
    if let Some(rest) = current.strip_prefix(committed) {
        return rest;
    }
    let mut boundaries = committed
        .grapheme_indices(true)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    boundaries.push(committed.len());
    for start in boundaries.into_iter().rev() {
        let suffix = &committed[start..];
        if !suffix.is_empty() && current.starts_with(suffix) {
            return &current[suffix.len()..];
        }
    }
    current
}

fn stable_prefix(left: &str, right: &str) -> String {
    let mut end = 0usize;
    for ((left_index, left_grapheme), (_, right_grapheme)) in left
        .grapheme_indices(true)
        .zip(right.grapheme_indices(true))
    {
        if left_grapheme != right_grapheme {
            break;
        }
        end = left_index + left_grapheme.len();
    }
    let raw = &left[..end];
    if raw.chars().any(char::is_whitespace) {
        let boundary = raw
            .char_indices()
            .rev()
            .find(|(_, ch)| ch.is_whitespace() || ch.is_ascii_punctuation())
            .map_or(0, |(index, ch)| index + ch.len_utf8());
        raw[..boundary].to_string()
    } else {
        raw.to_string()
    }
}

pub fn dictated_unit_count(text: &str) -> usize {
    let words = text
        .split_whitespace()
        .filter(|word| !word.is_empty())
        .count();
    if words > 1 || text.chars().any(char::is_whitespace) {
        return words;
    }
    text.graphemes(true)
        .filter(|grapheme| grapheme.chars().any(|ch| ch.is_alphanumeric()))
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cached_voice_models_work_offline_and_missing_models_require_configured_downloads() {
        let root = tempfile::tempdir().unwrap();
        let spec = ModelSpec {
            name: "fixture",
            filename: "fixture.bin",
            bytes: 4,
            sha256: "unused-for-an-existing-cache",
        };
        let (events, mut receiver) = mpsc::unbounded_channel();
        let missing = tokio::time::timeout(
            Duration::from_secs(1),
            ensure_model_in(root.path(), spec, &events, false),
        )
        .await
        .unwrap()
        .unwrap_err();
        assert!(missing.to_string().contains("voice.allow_model_download"));
        assert!(receiver.try_recv().is_err());
        let cached = root.path().join(spec.filename);
        std::fs::write(&cached, b"data").unwrap();
        assert_eq!(
            ensure_model_in(root.path(), spec, &events, false)
                .await
                .unwrap(),
            cached
        );
        assert_eq!(std::fs::read(cached).unwrap(), b"data");
        assert!(!VoiceConfig::from_config(&Config::default()).allow_model_download);
        let configured =
            serde_json::from_value(serde_json::json!({"voice":{"allow_model_download":true}}))
                .unwrap();
        assert!(VoiceConfig::from_config(&configured).allow_model_download);
    }

    #[tokio::test]
    async fn controller_shutdown_completes_without_starting_native_resources() {
        let (events, _receiver) = mpsc::unbounded_channel();
        let controller =
            VoiceController::spawn(VoiceConfig::from_config(&Config::default()), events);
        tokio::time::timeout(Duration::from_secs(1), controller.shutdown())
            .await
            .expect("voice controller should shut down promptly");
    }

    #[test]
    fn native_logging_hooks_are_safe_to_install_repeatedly() {
        whisper_rs::install_logging_hooks();
        whisper_rs::install_logging_hooks();
        let _ = WhisperContext::new_with_params(
            "/definitely/missing/craftsman-whisper-model.bin",
            WhisperContextParameters::default(),
        );
    }

    #[test]
    fn voice_config_defaults_and_aliases() {
        let cfg = Config::default();
        let voice = VoiceConfig::from_config(&cfg);
        assert!(voice.enabled);
        assert_eq!(voice.model, "large-v3-turbo-q5_0");
        assert!((voice.vad_threshold - DEFAULT_VAD_THRESHOLD).abs() < f32::EPSILON);
        assert_eq!(voice.vad_silence_ms, 800);
        assert_eq!(voice.idle_timeout_ms, 30_000);
        assert_eq!(voice.max_session_minutes, 10);
        assert_eq!(canonical_model_name("small-q5_0"), Some("small-q5_1"));
        assert_eq!(canonical_model_name("base"), Some("base-q5_1"));
        assert!(canonical_model_name("unknown").is_none());
    }

    #[test]
    fn voice_config_clamps_custom_vad_threshold() {
        let mut cfg = Config::default();
        cfg.set_voice_setting("vad_threshold", serde_json::json!(0.22));
        assert!((VoiceConfig::from_config(&cfg).vad_threshold - 0.22).abs() < f32::EPSILON);
        cfg.set_voice_setting("vad_threshold", serde_json::json!(-1.0));
        assert_eq!(
            VoiceConfig::from_config(&cfg).vad_threshold,
            MIN_VAD_THRESHOLD
        );
        cfg.set_voice_setting("vad_threshold", serde_json::json!(2.0));
        assert_eq!(
            VoiceConfig::from_config(&cfg).vad_threshold,
            MAX_VAD_THRESHOLD
        );
    }

    #[test]
    fn neural_vad_threshold_starts_speech_immediately() {
        let mut gate = SpeechGate::new(DEFAULT_VAD_THRESHOLD);
        assert!(gate.observe(DEFAULT_VAD_THRESHOLD, 0.0));
    }

    #[test]
    fn sustained_speech_energy_recovers_from_conservative_neural_vad() {
        let mut gate = SpeechGate::new(DEFAULT_VAD_THRESHOLD);
        for _ in 0..ENERGY_ATTACK_CHUNKS - 1 {
            assert!(!gate.observe(0.02, 0.006));
        }
        assert!(gate.observe(0.02, 0.006));
    }

    #[test]
    fn ambient_noise_and_brief_spikes_do_not_start_speech() {
        let mut gate = SpeechGate::new(DEFAULT_VAD_THRESHOLD);
        for _ in 0..20 {
            assert!(!gate.observe(0.005, 0.0006));
        }
        assert!(!gate.observe(0.08, 0.01));
        assert!(!gate.observe(0.005, 0.0006));
    }

    #[test]
    fn a_speech_candidate_can_start_partial_work_before_full_energy_attack() {
        let mut gate = SpeechGate::new(DEFAULT_VAD_THRESHOLD);
        assert!(!gate.observe(0.02, 0.006));
        assert!(gate.heard_candidate);
        assert!(!gate.energy_active);
    }

    #[test]
    fn scaled_voice_fixture_still_starts_the_combined_speech_gate() {
        let samples = include_bytes!("data/voice-benchmark.pcm")
            .chunks_exact(2)
            .map(|bytes| i16::from_le_bytes([bytes[0], bytes[1]]) as f32 / 32768.0 * 0.2)
            .collect::<Vec<_>>();
        let mut vad = VoiceActivityDetector::builder()
            .sample_rate(SAMPLE_RATE as i64)
            .chunk_size(VAD_CHUNK)
            .build()
            .unwrap();
        let mut gate = SpeechGate::new(DEFAULT_VAD_THRESHOLD);
        assert!(samples.chunks_exact(VAD_CHUNK).any(|chunk| {
            let probability = vad.predict::<f32, _>(chunk.iter().copied());
            gate.observe(probability, chunk_rms(chunk))
        }));
    }

    #[test]
    fn explicit_stop_audio_gate_requires_duration_and_audible_level() {
        let build_processor = || {
            RecordingProcessor::new(
                SAMPLE_RATE as u32,
                VoiceActivityDetector::builder()
                    .sample_rate(SAMPLE_RATE as i64)
                    .chunk_size(VAD_CHUNK)
                    .build()
                    .unwrap(),
                MAX_VAD_THRESHOLD,
                800,
            )
        };

        let mut audible = build_processor();
        audible.push(&vec![
            MIN_EXPLICIT_AUDIO_RMS * 1.2;
            MIN_EXPLICIT_AUDIO_SAMPLES + VAD_CHUNK
        ]);
        audible.speech_gate.heard_candidate = true;
        assert!(audible.has_explicit_audio());

        let mut too_short = build_processor();
        too_short.push(&vec![MIN_EXPLICIT_AUDIO_RMS * 1.2; VAD_CHUNK]);
        too_short.speech_gate.heard_candidate = true;
        assert!(!too_short.has_explicit_audio());

        let mut too_quiet = build_processor();
        too_quiet.push(&vec![
            MIN_EXPLICIT_AUDIO_RMS * 0.5;
            MIN_EXPLICIT_AUDIO_SAMPLES + VAD_CHUNK
        ]);
        too_quiet.speech_gate.heard_candidate = true;
        assert!(!too_quiet.has_explicit_audio());

        let mut no_speech_candidate = build_processor();
        no_speech_candidate.push(&vec![
            MIN_EXPLICIT_AUDIO_RMS * 1.2;
            MIN_EXPLICIT_AUDIO_SAMPLES + VAD_CHUNK
        ]);
        assert!(!no_speech_candidate.has_explicit_audio());
    }

    #[test]
    fn quiet_transcription_audio_is_boundedly_normalized() {
        let mut quiet = vec![0.01, -0.005, 0.002];
        normalize_transcription_audio(&mut quiet);
        assert!((quiet[0] - 0.16).abs() < 0.0001);
        assert!((quiet[1] + 0.08).abs() < 0.0001);

        let mut loud = vec![0.5, -0.4];
        normalize_transcription_audio(&mut loud);
        assert_eq!(loud, vec![0.5, -0.4]);
    }

    #[test]
    fn local_agreement_commits_only_stable_boundary() {
        assert_eq!(stable_prefix("hello wor", "hello world"), "hello ");
        assert_eq!(stable_prefix("你好世界", "你好世界啊"), "你好世界");
        assert_eq!(stable_prefix("alpha beta.", "alpha beta!"), "alpha ");
    }

    #[test]
    fn rolling_window_strips_committed_suffix_overlap() {
        assert_eq!(
            strip_committed_overlap("hello world ", "world how are you"),
            "how are you"
        );
        assert_eq!(strip_committed_overlap("你好世界", "世界今天"), "今天");
    }

    #[test]
    fn continuous_session_text_joins_segments_with_one_space() {
        let mut text = String::new();
        append_session_text(&mut text, "first sentence");
        append_session_text(&mut text, " second sentence ");
        assert_eq!(text, "first sentence second sentence");
    }

    #[test]
    fn dictated_word_gate_handles_cjk() {
        assert_eq!(dictated_unit_count("one two"), 2);
        assert_eq!(dictated_unit_count("你好世界"), 4);
        assert_eq!(dictated_unit_count(""), 0);
    }

    #[test]
    fn resampler_preserves_rate_ratio_across_chunks() {
        let mut resampler = LinearResampler::new(48_000);
        let mut output = Vec::new();
        resampler.process(&vec![0.5; 24_000], &mut output);
        resampler.process(&vec![0.5; 24_000], &mut output);
        assert!(
            (output.len() as isize - 16_000).abs() <= 2,
            "{}",
            output.len()
        );
        assert!(output.iter().all(|sample| (*sample - 0.5).abs() < 0.001));
    }

    #[test]
    fn capture_permission_errors_are_distinct() {
        assert!(matches!(
            map_capture_error("not authorized to use microphone"),
            VoiceError::MicPermissionDenied
        ));
        assert!(matches!(
            map_capture_error("device disappeared"),
            VoiceError::NoAudioCaptured
        ));
    }
}
