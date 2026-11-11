//! Compile-time stub for unsupported platforms and `--no-default-features`.
#![allow(dead_code)]

use crate::config::Config;
use std::path::PathBuf;
use tokio::sync::mpsc;

#[derive(Clone, Debug, PartialEq)]
pub struct VoiceConfig {
    pub enabled: bool,
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
    pub fn from_config(_cfg: &Config) -> Self {
        Self {
            enabled: false,
            engine: "unavailable".into(),
            model: "large-v3-turbo-q5_0".into(),
            language: None,
            auto_submit: false,
            min_words_to_submit: 3,
            partial_transcripts: false,
            vad_threshold: 0.35,
            vad_silence_ms: 800,
            idle_timeout_ms: 30_000,
            max_session_minutes: 10,
        }
    }
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

#[derive(Clone, Debug)]
pub enum VoiceError {
    MicPermissionDenied,
    NoAudioCaptured,
    NoSpeechDetected,
    EngineFailed(String),
}

impl VoiceError {
    pub fn user_message(&self, _model: &str) -> String {
        "Voice input is not available in this build.".into()
    }
}

pub struct VoiceController;
impl VoiceController {
    pub fn spawn(_config: VoiceConfig, _events: mpsc::UnboundedSender<VoiceEvent>) -> Self {
        Self
    }
    pub fn start(&self) {}
    pub fn stop_and_finalize(&self) {}
    pub fn cancel_segment(&self) {}
    pub fn cancel(&self) {}
    pub async fn shutdown(self) {}
}

pub fn model_names() -> impl Iterator<Item = &'static str> {
    std::iter::empty()
}
#[derive(Clone, Copy, Debug)]
pub struct ModelSpec {
    pub name: &'static str,
    pub filename: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
}

pub fn canonical_model_name(_name: &str) -> Option<&'static str> {
    None
}
pub fn model_path(_name: &str) -> Option<PathBuf> {
    None
}
pub fn model_downloaded(_name: &str) -> bool {
    false
}
pub fn dictated_unit_count(text: &str) -> usize {
    text.split_whitespace().count()
}
