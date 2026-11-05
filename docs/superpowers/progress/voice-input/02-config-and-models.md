# Configuration and model management

Status: **implementing**

## Implemented

- Added nested `voice` config readers and mutation that preserve unknown keys.
- Added defaults, validation, language handling, and model aliases.
- Corrected small/base model presets to `small-q5_1` and `base-q5_1`.
- Added an immutable model revision, exact file sizes, and SHA-256 hashes.
- Added `~/.craftsman/models` storage with `CRAFTSMAN_CONFIG_DIR` compatibility.
- Added lazy first-use download, `.part` resume, Range requests, throttled
  progress events, size validation, SHA-256 validation, and atomic rename.
- Added recovery for a fully downloaded `.part` file and removal of a corrupt
  partial.

## Pending

- Add isolated downloader tests for fresh download, resume, ignored Range,
  interruption, checksum failure, and offline errors.
- Manually interrupt and resume a real model download.
- Verify file permissions and real-host redirect/Range behavior on macOS and
  Linux.
