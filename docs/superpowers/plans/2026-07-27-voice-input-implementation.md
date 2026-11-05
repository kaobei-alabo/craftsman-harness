# Voice input for craftsman-harness — implementation plan

Date: 2026-07-27
Status: implemented in code; cross-target and live verification tracked under `docs/superpowers/progress/voice-input/`
Design: `docs/superpowers/specs/2026-07-27-voice-input-design.md`

## 1. Scope and delivery strategy

Implement the approved v1 TUI-only voice dictation design without changing
`deepagents-rs` or the desktop/serve protocols. Ship it in small, independently
verifiable slices so native audio, ONNX/VAD, and whisper.cpp build risk is
resolved before the TUI becomes dependent on it.

Recommended delivery order:

1. Native dependency and release-target spike.
2. Feature/config/model-management foundation.
3. Whisper engine and benchmark.
4. Audio capture, resampling, VAD, and session state machine.
5. Rolling partials and LocalAgreement.
6. TUI composer/rendering/commands.
7. Test, release, documentation, and manual verification pass.

Do not begin desktop `voice_*` protocol work in this implementation. The v1.5
extension point remains design-only.

## 2. Current codebase findings

- There is no voice implementation or voice dependency in the workspace yet.
- `crates/craftsman-rs/src/tui_repl.rs` already has the right single-owner event
  loop. `UiEvent`, `UiBridge`, `handle_ui_event`, and the app request flags are
  the integration path; audio/STT tasks must never draw or mutate `App`
  directly.
- Composer insertion currently flows through `App::insert()`, which already
  records undo state. Rendering, however, is a plain `String` from
  `layout_input()`, so tentative voice text needs a styled, virtual-text layout
  rather than being stored as ordinary input.
- `handle_key()` is synchronous and has no runtime handles. Voice keys should
  set app requests, and the outer async loop should execute them, matching the
  existing cancel/mode/copy request pattern.
- Config compatibility is maintained through `Config::extra`. Voice accessors
  and updates must preserve both top-level unknown fields and unknown keys
  inside the nested `voice` object.
- The release workflow builds macOS arm64, Linux x86_64, Linux arm64, and
  Windows x86_64. The design calls Windows a non-goal, but a default-on feature
  must still leave the Windows release build green by target-gating the voice
  module and native dependencies.
- Linux CPAL builds require ALSA development headers, so the release workflow
  and developer documentation need an explicit native-package step.
- The approved design's smaller preset names need correction before coding:
  the maintained whisper.cpp model repository provides `base-q5_1` and
  `small-q5_1`, not `base-q5_0` and `small-q5_0`. The default
  `large-v3-turbo-q5_0` file is valid. Use a pinned manifest with full SHA-256
  values rather than accepting arbitrary filenames.

## 3. Implementation clarifications

These details are required to turn the design into deterministic code.

### 3.1 Compile-time and runtime availability

- Add a default-on `voice` Cargo feature.
- Compile native voice code only for macOS and Linux.
- On Windows or `--no-default-features`, omit the command/menu/hints and avoid
  compiling CPAL, VAD, or whisper.cpp.
- At runtime, treat `SSH_CONNECTION` or `SSH_TTY` as unavailable. Hide the
  slash-menu entry and key hint; a manually typed `/voice` should return one
  concise explanation instead of attempting capture.
- Do not initialize the model, VAD, or microphone at process startup. The first
  explicit activation (`Ctrl+G`, empty-prompt Space, or bare `/voice`) starts
  lazy initialization.

### 3.2 Runtime control and events

Use an async controller around the UI-agnostic core:

```text
VoiceController
  commands: Start | StopAndFinalize | Cancel | Reconfigure | Shutdown
  events:   State | DownloadProgress | Level | Partial | Final | Error
```

The design's four user-facing errors remain unchanged. `State` and
`DownloadProgress` are additional lifecycle events needed to render first-run
model download and initialization without blocking or flooding transcript
history.

Use monotonically increasing session/generation IDs. Results from a cancelled
recording or an older rolling STT job must be ignored.

### 3.3 Realtime callback boundary

The CPAL callback must only:

1. Convert the device sample type to `f32`.
2. Downmix channels.
3. Push frames into a bounded SPSC/ring buffer.
4. Report stream errors through a non-blocking channel.

It must not allocate per frame, lock a mutex, run VAD, call Tokio async code, or
run Whisper. A Tokio task drains the buffer, resamples to 16 kHz, computes
levels, and feeds the pure session core.

### 3.4 Composer ownership model

Store only committed transcript text in `App::input`. Keep tentative text
virtual and track a byte-safe `voice_marker` where the next committed/final
suffix is inserted.

- Starting voice sets the marker to the current cursor.
- A newly committed delta inserts at the marker using the same undo-aware edit
  primitive as paste, then advances the marker.
- Normal user edits are allowed. Every edit adjusts the marker when text before
  it is inserted/deleted, so typed text and dictated text can interleave.
- Tentative text renders dimmed at the marker but is never included in history,
  submission, clipboard input, or undo state.
- Finalization inserts only the final uncommitted suffix; it must not duplicate
  already committed text.
- Esc discards the current tentative segment while preserving committed text;
  a later Esc with no active segment closes continuous listening.
- Enter finalizes the active segment, closes listening, and submits the complete
  composer. Ctrl+G closes without forced submission. Auto-submit is evaluated
  only when the continuous session ends.
- Ctrl+C while voice is active cancels voice first so the application cannot
  exit or clear the composer while the microphone task is still running.

### 3.5 Partial transcription window

A naive "last 12 seconds" text prefix cannot be appended safely after the
window start moves. Define `Transcript` with timestamped segments and implement
`RollingTranscriber` as follows:

- Keep a stable window start while collecting up to about 12 seconds.
- Compare two consecutive transcripts for that same overlapping window.
- Commit only agreed text ending at a known segment/token timestamp.
- Once the window must advance, retain a short audio overlap and carry forward
  only the still-uncommitted tail; deduplicate it against the prior pass.
- Keep at most one rolling Whisper job in flight. If new audio arrives while it
  runs, coalesce into the next scheduled pass rather than creating a backlog.
- Each segment boundary transcribes the segment and replaces only its
  tentative/uncommitted tail. Capture remains open and the next segment starts
  with fresh VAD/partial state; committed text remains immutable.

### 3.6 Submission gate

`min_words_to_submit` applies to the finalized dictated utterance, not the whole
composer. Count whitespace-delimited words for ordinary scripts; when no such
word boundaries exist, count CJK grapheme/character units. A successful final
resets the consecutive-failure count even when it is below the auto-submit
threshold.

## 4. Proposed files and module boundaries

```text
crates/craftsman-rs/src/voice/
  mod.rs          public types, controller, feature/platform availability
  config.rs       effective voice config and preset resolution (optional split)
  model.rs        model manifest, resumable verified downloader
  audio.rs        CPAL adapter, sample conversion, downmix, linear resampler
  vad.rs          VAD adapter and endpoint policy
  stt.rs          SttEngine, Transcript, WhisperEngine, init benchmark
  agreement.rs    LocalAgreement and rolling-window alignment
  session.rs      pure state machine plus async orchestration

crates/craftsman-rs/src/data/
  voice-benchmark.wav or voice-benchmark.pcm

crates/craftsman-rs/tests/
  voice_whisper.rs                 ignored real-engine integration test
  fixtures/voice/english-3s.wav    short redistributable test fixture
```

Likely existing-file changes:

- `crates/craftsman-rs/Cargo.toml`
- `crates/craftsman-rs/src/main.rs`
- `crates/craftsman-rs/src/config.rs`
- `crates/craftsman-rs/src/tui_repl.rs`
- `.github/workflows/release.yml`
- `README.md`
- `Cargo.lock`

Keep all voice dependencies and code out of `crates/deepagents-rs`.

## 5. Phase-by-phase work

### Phase 0 — native dependency and target spike

**Goal:** prove the native stack and release matrix before building product
behavior.

1. Add a temporary/minimal feature-gated module that can:
   - enumerate/open the default CPAL input device;
   - construct the chosen VAD implementation;
   - load a cached Whisper model and transcribe a short PCM buffer.
2. Pin compatible versions of `cpal`, `whisper-rs`, and the VAD crate.
3. Test macOS arm64 and both Linux release architectures. Check Windows and
   `--no-default-features` builds with all voice code compiled out.
4. Verify that `voice_activity_detector`/`ort` does not introduce link,
   runtime-library, or Linux-arm64 failures alongside whisper.cpp.
5. If the ONNX path fails the cross-target gate, use the design-approved
   `earshot` fallback and record the deviation in the design spec before
   continuing.
6. Add explicit CI packages for Linux (at minimum ALSA development headers and
   `pkg-config`; add CMake/compiler packages if the pinned whisper build needs
   them).
7. Decide and lock the macOS Metal feature setup. Linux remains CPU-only in v1.

**Exit gate:** default, no-default-feature, and unsupported-target builds all
compile; a local smoke program captures samples and runs one transcription.

### Phase 1 — feature flag, config, and model manager

**Files:** `Cargo.toml`, `main.rs`, `config.rs`, `voice/model.rs`, tests.

1. Add `default = ["voice"]` and optional/target-specific dependencies.
2. Add a single `voice_compiled()`/availability boundary so TUI cfg logic does
   not spread throughout the file.
3. Implement an owned `VoiceConfig` with defaults and validation:
   - `enabled = true`
   - `engine = "whisper"`
   - `model = "large-v3-turbo-q5_0"`
   - `language = None` for `"auto"`
   - `auto_submit = false`
   - `min_words_to_submit = 3`
   - `partial_transcripts = true`
   - `segment_silence_ms = 800`, clamped to a safe range (`vad_silence_ms`
     remains a compatibility alias)
   - `idle_timeout_ms = 30000`
   - `max_session_minutes = 10`
4. Add nested config mutation helpers used by `/voice model`, preserving unknown
   nested keys. Do not add a typed top-level `voice` field to `Config`.
5. Define a closed model manifest containing config name, immutable download
   revision/URL, filename, byte size, and full SHA-256. Include:
   - `large-v3-turbo-q5_0`
   - `small-q5_1`
   - `base-q5_1`
6. Store models under `config_dir()/models`, honoring
   `CRAFTSMAN_CONFIG_DIR` in tests.
7. Implement resumable download:
   - write `<file>.part`;
   - send `Range` from the current length;
   - restart safely if the server ignores/rejects the range;
   - stream progress events;
   - verify final size and SHA-256;
   - atomically rename only after verification;
   - leave a valid existing model untouched on failure.
8. Add `wiremock` tests for fresh download, resume, ignored Range, checksum
   mismatch, interrupted response, and offline error text.

**Exit gate:** config round-trips unknown fields, model downloads are atomic and
resumable, and no model is fetched until explicit activation.

### Phase 2 — STT engine and performance benchmark

**Files:** `voice/stt.rs`, benchmark fixture, ignored integration test.

1. Define:
   - `Transcript { text, segments }`;
   - `TranscriptSegment { text, start_ms, end_ms }`;
   - the approved async `SttEngine` trait;
   - an internal engine-status structure for `/voice status`.
2. Implement `WhisperEngine` so model/context setup is reused across utterances.
3. Run blocking whisper.cpp calls on one dedicated blocking worker. Do not hold
   async locks across transcription and do not run C FFI on the UI task.
4. Configure decoding for plain transcription, timestamps, no cloud/network,
   and optional pinned language.
5. Normalize engine output in one place (trim model artifacts, preserve Unicode,
   avoid adding punctuation/spacing in TUI code).
6. Bundle a small redistributable benchmark sample. Measure its real-time
   factor and estimate the 10-second-window cost (`elapsed / sample_duration *
   10s`). Disable partials when the estimate exceeds the design threshold or
   when config disables them.
7. Emit initialization status and the underpowered-machine suggestion without
   treating disabled partials as a fatal error.
8. Add a fake-engine unit test suite and an ignored real-engine test that reads
   a cached/test model path and asserts a non-empty transcript for the WAV
   fixture. CI must not download the 574 MB model.

**Exit gate:** end-of-utterance transcription works from a file fixture and the
engine reports an effective partials capability.

### Phase 3 — capture, resampling, VAD, and the session core

**Files:** `voice/audio.rs`, `voice/vad.rs`, `voice/session.rs`.

1. Implement CPAL default-device/config selection for macOS and Linux.
2. Support the device sample formats returned by CPAL (`f32`, signed integer,
   unsigned integer), downmix N channels, and keep conversion allocation-free in
   the callback.
3. Implement a stateful linear resampler to 16 kHz mono. Preserve fractional
   phase across callback boundaries.
4. Compute an RMS/peak-derived normalized level and rate-limit `Level` events to
   about 10 Hz.
5. Put endpoint policy behind an internal `VadEngine` interface so the session
   core is testable without ONNX or a microphone.
6. Separate a pure `VoiceSessionCore` from I/O adapters. Feed it commands,
   16-kHz frames, VAD scores, clock ticks, and scripted STT completions.
7. Implement states and transitions:
   - `Idle -> Initializing -> Recording <-> Finalizing -> Idle`
   - each VAD finalization returns to `Recording` with capture still open;
   - session cancel returns to idle and bumps the generation.
8. Endpoint rules:
   - after speech starts, finalize the segment after configured silence;
   - keep listening across segment boundaries;
   - stop after the configured whole-session idle timeout or session cap;
   - Ctrl+G finalizes and closes; Enter finalizes, closes, and submits;
   - Esc cancels the current segment, then closes when no segment is active.
9. Distinguish:
   - no callback/device samples -> `NoAudioCaptured`;
   - samples/no VAD speech or empty transcript -> `NoSpeechDetected`;
   - permission/open-device errors -> `MicPermissionDenied` where detectable;
   - model/worker/panic/decoding errors -> `EngineFailed`.
10. Implement the three-consecutive-failure circuit breaker. Reset on success;
    `/voice on` explicitly resets a paused session.
11. Ensure stream drop and controller shutdown release the microphone on every
    path, including application exit and task panic.

**Exit gate:** deterministic state-machine tests cover silence, tone/speech,
manual stop, cancel, hard cap, device loss, errors, and circuit breaker without
opening a real microphone.

### Phase 4 — rolling partials and LocalAgreement

**Files:** `voice/agreement.rs`, `voice/session.rs` tests.

1. Implement Unicode-safe transcript units. Use word/punctuation units where
   boundaries exist and grapheme units for CJK/no-space text; never slice a
   UTF-8 string by an unchecked byte offset.
2. Compare two consecutive passes and return `{ committed_delta, tentative }`.
3. Use timestamped segment boundaries when advancing the rolling audio window.
4. Keep one partial job in flight and one coalesced pending request at most.
5. Ignore stale results using the session generation and partial sequence.
6. On final pass, align the full text with the immutable committed prefix and
   emit only the final uncommitted suffix plus the full finalized utterance for
   auto-submit word counting.
7. If a partial pass fails, keep recording and disable further partials for that
   utterance; only a final-pass failure emits the fatal engine error.
8. Add tests for repeated words, punctuation changes, corrections in the tail,
   CJK, window rollover, stale results, and final text that does not byte-match
   the committed prefix after normalization.

**Exit gate:** synthesized scripted transcripts never duplicate or retract
committed composer text and maintain a bounded STT queue.

### Phase 5 — TUI integration

**Files:** `tui_repl.rs` and TUI tests.

1. Extend `UiEvent` with voice lifecycle, download, level, partial, final, and
   error variants. Map `VoiceEvent` to the existing `UiBridge` sender.
2. Add `VoiceUiState` to `App`, containing lifecycle state, level, tentative
   text, marker, effective-partials flag, and any pending auto-submit state.
3. Add request flags/actions for start, stop/finalize, cancel, session enable,
   and model reconfiguration. Execute them in the outer async event loop where
   the lazy `VoiceController` lives.
4. Update composer editing primitives so all insert/delete/undo/history/clear
   paths maintain or intentionally clear the voice marker. Store marker state in
   undo snapshots where necessary.
5. Replace the plain composer layout result with a grapheme-aware styled layout
   that:
   - inserts tentative text virtually at the marker;
   - dims only tentative spans;
   - keeps cursor coordinates correct before and after virtual text;
   - preserves current five-row scrolling behavior.
6. Controls:
   - `Ctrl+G`: start when idle; stop and finalize when recording;
   - Space: start only when the composer is empty, idle, available, and no modal
     is open; otherwise insert a normal space;
   - Esc: cancel the active segment; a later Esc closes continuous listening;
   - Ctrl+C: cancel the whole voice session first;
   - Enter while voice is active: finalize, close, and submit the composer.
7. Render states:
   - initializing/downloading/transcribing in the input title;
   - red `● listening` while recording;
   - compact level meter in the footer;
   - normal title/footer restored after final/error/cancel.
8. Apply partial committed deltas at the marker and render tentative tail only.
9. Apply each segment-final suffix at the marker, clear per-segment tentative
   state, and keep listening. At session end, set `submit_requested` for Enter or
   when `auto_submit` is true and the dictated-text gate passes. Existing
   steer/queue behavior then handles active agent turns.
10. Add `/voice` to the TUI help and slash list only when compiled and locally
    available:
    - bare `/voice`: start;
    - `/voice status`: compiled/runtime availability, enabled state, engine,
      model path/downloaded state, loaded state, partials configured/effective;
    - `/voice on|off`: session override only; `off` cancels active capture;
    - `/voice model <name>`: validate manifest name, persist nested config,
      unload/recreate the lazy engine on the next activation.
11. Map each `VoiceError` to the approved actionable wording. Show one message,
    preserve committed text, clear tentative state, and restore the composer.
12. On TUI exit, send `Shutdown` and await bounded cleanup before leaving the
    alternate screen.

**Exit gate:** TUI unit tests cover all keybindings, slash commands, marker
adjustment, dim-tail layout/cursor math, Esc preservation, auto-submit gate,
active-turn steer/queue reuse, SSH hiding, and no-feature behavior.

### Phase 6 — full verification and release hardening

1. Run the repository gates:

   ```bash
   cargo fmt --check
   cargo check --locked
   cargo check --locked -p craftsman-rs --no-default-features
   cargo test --locked
   cargo build --release --locked -p craftsman-rs
   ```

2. Exercise the release workflow targets, especially Linux arm64 and the
   voice-disabled Windows artifact.
3. Check binary size and startup time. Voice initialization must remain lazy,
   so normal `craftsman`, one-shot mode, `serve`, and `desktop-serve` do not load
   Whisper/VAD or touch microphone APIs.
4. Confirm `crates/deepagents-rs/tests/dep_guard.rs` remains unchanged and green.
5. Run ignored real-engine integration tests explicitly on one macOS arm64 and
   one Linux machine with cached models.
6. Test downloader behavior against the real pinned host, including one manual
   interruption/resume.

### Phase 7 — documentation and manual ship gate

1. Update `README.md` with:
   - Ctrl+G, empty-Space, and `/voice` usage;
   - config example and valid model names;
   - first-use download size/offline behavior;
   - Linux native build prerequisites;
   - `--no-default-features` opt-out;
   - macOS microphone permission troubleshooting;
   - SSH unavailability.
2. Amend the design spec's invalid `small/base-q5_0` names and record the VAD
   fallback only if Phase 0 uses it.
3. Complete the design's manual matrix:
   - Terminal.app and iTerm2 permission prompts;
   - denied permission;
   - English and Chinese dictation;
   - partials enabled and benchmark-disabled;
   - edit before Enter;
   - auto-submit with a below-threshold utterance;
   - Esc cancel and committed-text preservation;
   - active-turn steer/queue path;
   - Bluetooth/input-device loss;
   - SSH hidden state;
   - clean download plus interrupt/resume;
   - 15-second no-speech stop and 2-minute cap.

## 6. Suggested pull-request slices

1. **voice/build-foundation** — Phase 0, Cargo feature, CI prerequisites, target
   gating.
2. **voice/model-and-stt** — config, model manifest/downloader, Whisper engine,
   benchmark, fixture test.
3. **voice/session-core** — audio adapter, resampler, VAD, state machine,
   LocalAgreement.
4. **voice/tui** — events, composer marker/layout, controls, `/voice`, errors,
   auto-submit.
5. **voice/hardening** — release matrix, ignored live tests, README/spec fixes,
   manual verification fixes.

Each slice should keep `cargo test --locked` green; do not merge a slice that
requires a later slice merely to restore non-voice behavior.

## 7. Definition of done

Voice v1 is complete when:

- macOS and Linux users can activate dictation from all three specified entry
  points, see partial/tentative text when supported, edit the final text, and
  submit through existing TUI paths;
- audio and model data remain local except for the explicit first-use model
  download;
- all four error classes and the three-failure circuit breaker are observable
  and actionable;
- first-use download is resumable and SHA-256 verified;
- default, no-default-feature, Linux release, macOS release, and Windows release
  builds pass;
- no voice dependency enters `deepagents-rs`;
- no desktop/serve voice protocol behavior is shipped in v1;
- the full automated and manual verification gates above pass.
