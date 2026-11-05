# Voice input for craftsman-harness — design spec

Date: 2026-07-27
Status: implemented for TUI/Desktop and workspace browser PCM segments
Scope: the TUI/Desktop use the native microphone controller; `craftsman serve`
accepts browser-captured PCM segments for local pod Whisper transcription.

## 1. Goal

Let the user dictate prompts by voice in the craftsman TUI: tap a key, speak, watch
the transcript stream into the input box with a realtime feel, edit it, press
Enter to send. Fully local — no cloud STT, no LLM voice model, offline after a
one-time model download.

Decisions made during design review:

- **Dictation into the prompt, edit-before-send by default.** The transcript is
  inserted as ordinary editable text; the user presses Enter to submit. A config
  flag `voice.auto_submit` opts into auto-submission when the continuous
  listening session ends.
- **Local-only STT.** Dictated prompts contain proprietary code identifiers;
  audio never leaves the machine. A cloud engine remains possible later behind
  the `SttEngine` trait but is out of scope.
- **Broad multilingual** (~99 languages) via Whisper `large-v3-turbo` quantized.
- **No LLM voice model.** Voice *detection* is a 2 MB VAD model (Silero);
  *transcription* is a dedicated ASR model (Whisper). Audio-LLMs (e.g. Gemma 4)
  are heavier and less accurate for pure transcription.
- **Realtime transcription feel in v1** via rolling re-transcription with
  prefix commitment (LocalAgreement), not a second streaming model.

## 2. Non-goals (v1)

- Voice output / TTS / two-way voice conversation.
- Voice implementation in `desktop-serve` (protocol shape is specified below
  for the future, but no server code ships in v1).
- Wake words, hands-free open mic, global hotkeys (impossible in a TUI anyway).
- Windows support (matches the rest of craftsman-harness: macOS primary, Linux secondary).

## 3. Architecture

One UI-agnostic voice core in `craftsman-rs`; the TUI is its only v1 consumer.

```
mic ──► cpal capture ──► ring buffer ──► VoiceSession (state machine)
                                          │        │
                                    Silero VAD   SttEngine trait
                                    (endpoint)     └── WhisperEngine (whisper-rs)
                                          │
                                          ▼
                              VoiceEvent callbacks (partial / final / level / error)
                                          │
                              TUI: UiEvent::Voice* ──► composer insert / dim tail
```

### 3.1 New module: `crates/craftsman-rs/src/voice.rs` (+ submodules if it grows)

- `trait SttEngine` — mirrors the `SearchProvider` pattern in `web_tools.rs`:

  ```rust
  #[async_trait]
  pub trait SttEngine: Send + Sync {
      /// Transcribe a complete utterance (16 kHz mono f32 PCM).
      async fn transcribe(&self, audio: &[f32], lang: Option<&str>) -> Result<Transcript>;
      /// Whether this engine is fast enough for rolling partials (benchmarked at init).
      fn supports_partials(&self) -> bool;
  }
  ```

  v1 ships one impl, `WhisperEngine` (whisper-rs). The trait leaves room for
  Apple SpeechAnalyzer (macOS 26+ sidecar) and cloud engines later.

- `VoiceSession` — owns the capture stream, ring buffer, VAD, and partials
  loop. States: `Idle → Recording → Finalizing → Idle`. Emits `VoiceEvent`s
  through a `tokio::mpsc` sender supplied by the caller:

  ```rust
  pub enum VoiceEvent {
      Level(f32),                                  // for the footer meter, ~10 Hz
      Partial { committed: String, tentative: String },
      Final { text: String },
      Error(VoiceError),
  }
  ```

- `VoiceError` — four distinct variants, never one generic failure:
  `MicPermissionDenied`, `NoAudioCaptured`, `NoSpeechDetected`, `EngineFailed`.

### 3.2 Dependencies (all in `craftsman-rs`, never `deepagents-rs`)

| Crate | Purpose | Notes |
|---|---|---|
| `cpal` 0.18 | mic capture | Apache-2.0, MSRV 1.85 — matches workspace. Callback thread hands frames to tokio via lock-free channel. |
| `voice_activity_detector` (Silero via `ort`) | end-of-speech detection | ~2 MB model. Fallback option: `earshot` (pure Rust, no ONNX) if the `ort` dep proves painful. |
| `whisper-rs` | whisper.cpp bindings | MIT, static/vendored build with `metal` feature on macOS. Workspace already tolerates C FFI (libsql/bindgen precedent). |
| hand-rolled linear resampler | resample device rate → 16 kHz mono | ~30 lines; linear interpolation is sufficient for speech-band STT input. No `rubato` dep unless quality problems surface in testing. |

All of it sits behind a `voice` cargo feature, **default-on**, so constrained
builds can `--no-default-features` it away. `tests/dep_guard.rs` is unaffected
because nothing touches `deepagents-rs`.

### 3.3 Model management (built-in — no external tools)

No external runtime (no ollama, no Python, no daemon): whisper.cpp is compiled
into the `craftsman` binary via `whisper-rs`, and craftsman downloads the model file
itself.

- Default model: `ggml-large-v3-turbo-q5_0.bin` (~574 MB, MIT), stored in
  `~/.craftsman/models/`.
- Downloaded automatically on first voice activation (direct HTTPS fetch from
  Hugging Face via the existing `reqwest` client) with a progress display and
  SHA-256 verification; resumable; clear failure message if offline. Fully
  offline thereafter.
- `voice.model` config can select smaller models (`small-q5_1` ~190 MB,
  `base-q5_1` ~60 MB) for weak hardware.
- At engine init, a short benchmark transcribes a bundled ~2 s sample; if the
  machine can't re-transcribe a 10 s window in < ~1.5 s, partials are disabled
  (`supports_partials() == false`) and transcription happens only at
  end-of-speech. On clearly underpowered CPUs the init message suggests
  `voice.model = "small"`.

## 4. Realtime transcription feel (rolling partials)

Whisper is batch, not streaming; the live feel is produced by:

1. Audio accumulates in the ring buffer while recording.
2. Every ~1 s, re-transcribe a sliding window of the most recent ~12 s.
3. **LocalAgreement prefix commitment**: when two consecutive passes agree on a
   prefix, that prefix is committed — rendered as normal input text and never
   revised. Only the still-changing tail is shown dimmed.
4. On each VAD segment boundary, one final segment pass replaces the tail;
   capture remains open and a fresh segment begins.

Expected feel: text trails the voice by ~1–1.5 s on Apple Silicon (Metal).
On hardware where the benchmark fails, the dimmed-tail UX is skipped entirely
and each finalized segment appears after its silence boundary.

## 5. TUI UX

### 5.1 Activation — tap-to-toggle

- `Ctrl+G` toggles the mic anywhere in the composer (rebindable later if a
  keybinding system lands; constant for v1).
- `Space` in an **empty** prompt also starts listening (discoverable, zero-cost
  — a space in an empty prompt has no other meaning). While listening, `Space`
  types a space as normal. VAD commits sentence segments but keeps listening;
  `Ctrl+G`, Enter, Esc on an idle segment, or a safety timeout ends the session.
- Bare `/voice` (submitted as a slash command) also starts listening — the
  discoverable entry point. Its slash-menu description advertises the hotkey
  (`/voice — dictate by voice (Ctrl+G)`) so the command teaches the fast path.
  Stopping is key-driven (`Ctrl+G`, Enter, or Esc with no active segment) or by
  the idle/session safety timeout. Typing a slash command while the mic is open
  would corrupt the transcript.
- No hold-to-talk: terminals have no key-up events; the key-repeat warmup hack
  is not worth it when VAD endpointing exists.

### 5.2 While listening

- Input block title changes to `● listening` (red dot) and the footer shows a
  compact level meter (`▁▂▅▇▅▂`) driven by `VoiceEvent::Level` — reuses the
  existing title/spinner machinery in `draw()`; no overlay, transcript history
  stays visible.
- Committed partial text inserts at the cursor as normal text; the tentative
  tail renders dimmed after it (the composer's `Paragraph` gains styled spans
  for the tail region).
- Typing while listening is allowed and interleaves at the cursor.

### 5.3 End of speech and continuous sessions

- Silero VAD fires after `voice.segment_silence_ms` (default 800 ms) of silence
  → final pass for that segment → tail replaced by final editable text → VAD
  state resets while the microphone stays open for the next sentence.
- `Ctrl+G` finalizes any active segment and closes listening. Enter finalizes,
  closes listening, and submits the complete composer. `Esc` discards the
  current tentative segment; a later Esc with no active segment closes listening.
- Safety nets: auto-stop after `voice.idle_timeout_ms` (default 30 s) without
  detected speech and a hard session cap of `voice.max_session_minutes`
  (default 10). Any active segment is finalized before a safety stop.

### 5.4 Submission

- **Default (`auto_submit: false`)**: transcript sits in the composer as
  ordinary editable text. Enter submits, exactly like typed input. The user can
  edit, append more dictation, or clear it.
- **Opt-in (`auto_submit: true`)**: the prompt auto-submits only when the
  continuous listening session ends, gated by `min_words_to_submit` (default 3;
  CJK counts characters as words). Segment boundaries never submit on their own.
- Mid-turn (agent running): finalized dictation follows the exact rules typed
  text has today — routed to `steer` or queued via the existing composer paths.
- `Esc` while a segment is active discards its tentative tail and keeps the mic
  open; committed text stays. Esc again with no active segment closes listening;
  the normal composer clear remains available after voice is idle.

### 5.5 Errors (distinct, actionable strings)

| Condition | Message sketch |
|---|---|
| Mic permission denied | "Microphone access denied. Grant it to your terminal in System Settings → Privacy & Security → Microphone (macOS attributes the mic to Terminal.app/iTerm2). VS Code's integrated terminal may fail to prompt — run craftsman from a real terminal." |
| No audio captured | "No audio detected from the microphone — is it muted or is another app holding it?" |
| No speech recognized | "Heard audio but no speech was recognized." |
| Engine/model failure | "Voice transcription failed: {detail}. Model file: ~/.craftsman/models/…" |

- Circuit breaker: after 3 consecutive failures, voice pauses for the session
  with a one-line notice.
- Gating: the feature is hidden (keybinding inert, no hint shown) when
  `SSH_CONNECTION`/`SSH_TTY` is set — there is no local mic over SSH.

### 5.6 TUI wiring (concrete anchor points in `tui_repl.rs`)

- New `UiEvent` variants cover level, speech-start, partial, segment-final,
  segment-cancelled, session-finished, lifecycle, download progress, and error
  events. They are sent from `VoiceSession` through the existing UI channel and
  handled by `handle_ui_event`.
- Segment-final insertion uses the same path as bracketed paste
  (`app.insert()`), preserving undo/redo behavior.
- Keybinding lands in `handle_key()`; listening state lives on `App` alongside
  the existing spinner/mode fields.
- `/voice` slash command: bare `/voice` starts listening (see §5.1);
  subcommands `status` (model, engine, partials on/off), `on|off`
  (enable/disable for the session), `model` (selectable picker), and
  `model <name>` (direct selection).

## 6. Server mode — workspace browser transcription

Implemented for `craftsman serve` so platform-web workspace audio uses the same
local Whisper model catalog as Craftsman Desktop/TUI without requiring a microphone
device inside the container:

- `hello.capabilities` includes `"voice"` when the build supports local voice.
- Client → server: `get_voice_status`, `set_voice_model`, and
  `voice_transcribe { audio: <base64 little-endian pcm16>, sample_rate,
  operation_id }`.
- Server → client: `voice_status` and operation-scoped `voice_event` frames for
  state, model download progress, final segment text, and errors.
- The browser owns microphone capture, silence segmentation, decoding, mono mix,
  and 16 kHz PCM conversion. The pod owns model download/verification and
  Whisper inference.
- Submission policy remains client-side: final text enters the editable
  composer and is sent as a normal chat message only when the user submits.

This segment RPC intentionally reuses the browser's continuous silence-based
segmentation instead of opening the pod's native CPAL microphone controller.

## 7. Configuration

Lives in the shared `~/.craftsman/config.json` under a `voice` object, accessed via
the `extra`-map accessor pattern (Python-CLI round-trip safe):

```json
{
  "voice": {
    "enabled": true,
    "engine": "whisper",
    "model": "large-v3-turbo-q5_0",
    "language": "auto",
    "auto_submit": false,
    "min_words_to_submit": 3,
    "partial_transcripts": true,
    "segment_silence_ms": 800,
    "idle_timeout_ms": 30000,
    "max_session_minutes": 10
  }
}
```

All fields optional with the defaults shown. `vad_silence_ms` remains a
compatibility alias for `segment_silence_ms`. `language` may pin a code (e.g.
`"zh"`) for better accuracy in a known-language workflow. `partial_transcripts`
is force-disabled when the init benchmark fails regardless of config.

## 8. Error handling & edge cases summary

- Model download interrupted → resumable; voice unavailable (with message)
  until complete.
- Device disappears mid-recording (Bluetooth headset dies) → `NoAudioCaptured`
  path, committed text preserved.
- Empty segment transcription (VAD fired on noise) → segment discarded while
  continuous listening remains open; an entirely empty timed-out session emits
  `NoSpeechDetected`.
- Agent turn running → same behavior as typed text (steer/queue), no special
  casing.
- Non-voice builds (`--no-default-features`) → `/voice` and keybindings absent.

## 9. Testing

- **Unit (no mic, no model)**: `VoiceSession` state machine with a fake
  `SttEngine` and synthesized PCM (silence / tone / recorded fixture) — VAD
  endpointing, partial commitment (LocalAgreement), word-count gate incl. CJK,
  error mapping, circuit breaker.
- **Unit**: config accessors round-trip unknown fields (existing config.rs test
  idiom).
- **Integration (feature-gated, `#[ignore]` in CI)**: real `WhisperEngine`
  against a checked-in ~3 s WAV fixture; asserts non-empty transcript.
- **Manual live pass before ship** (per user-level verification gates):
  macOS Terminal.app + iTerm2 TCC prompt flows; mic-denied path; dictation in
  en + zh; edit-then-Enter; `auto_submit: true` with a 2-word utterance (must
  not send); Esc cancel; SSH session hides the feature; model download from
  scratch including an interrupt/resume.

## 10. Phasing

- **v1 (this spec)**: voice core + WhisperEngine + Silero VAD + model
  downloader; TUI tap-toggle, level meter, rolling partials with dimmed tail,
  edit-before-send default with `auto_submit` opt-in; four error states;
  SSH gating; `/voice` command.
- **v1.5**: `voice_*` desktop-protocol frames + `voice` capability (server
  mode), per §6.
- **v2 (optional)**: Apple SpeechAnalyzer engine (macOS 26+ Swift sidecar) as a
  quality tier; cloud `SttEngine` opt-in; keybinding customization.
