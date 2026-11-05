# Build and platform gating

Status: **implementing**

## Implemented

- Added default-on `voice` Cargo feature.
- Kept native CPAL, ONNX/VAD, ring-buffer, and Whisper dependencies inside
  `craftsman-rs`.
- Target-gated native voice dependencies to macOS and Linux.
- Added a compile-time stub for Windows and `--no-default-features`.
- Confirmed the Windows dependency graph excludes native voice crates.
- Added Linux release prerequisites and a macOS CMake check to release CI.
- Added installer prerequisite checks for CMake and Linux ALSA development
  files.
- Clean default and no-default-feature checks pass in an isolated target
  directory.
- Local locked macOS arm64 release builds pass with and without voice.
- The local release artifact is about 52 MB with voice and 26 MB without it;
  both execute `--version` without initializing a model or microphone.

## Pending

- Run/confirm Linux x86_64, Linux arm64, and Windows x86_64 release builds in CI.
