# Voice input implementation progress

Design: `../../specs/2026-07-27-voice-input-design.md`
Plan: `../../plans/2026-07-27-voice-input-implementation.md`
Overall status: **implementing**

| Topic | Status | Detail |
|---|---|---|
| [Build and platform gating](01-build-and-platform.md) | implementing | Default/no-voice checks and local release builds pass; CI target matrix remains. |
| [Configuration and model management](02-config-and-models.md) | implementing | Config accessors, model manifest, resume, progress, and checksum code are present; downloader tests remain. |
| [Capture, VAD, and transcription](03-capture-vad-stt.md) | implementing | CPAL, resampling, Silero VAD, Whisper, endpointing, and partials are implemented; live microphone/model passes remain. |
| [TUI and composer UX](04-tui-and-composer.md) | implementing | Controls, events, virtual tentative text, commands, meter, and auto-submit are implemented; final UX verification remains. |
| [Testing, release, and ship gate](05-testing-and-release.md) | implementing | Full automated suite and local release builds pass; cross-target/manual gates remain. |

Update the focused topic file whenever its status or remaining checklist changes.
Only change the overall status to `implemented` after every topic is implemented.
