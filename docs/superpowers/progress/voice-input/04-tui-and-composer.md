# TUI and composer UX

Status: **implementing**

## Implemented

- Added voice events to the existing single-owner TUI event path.
- Added lazy controller creation and graceful shutdown before terminal teardown;
  active capture is dropped, native Whisper work receives an abort signal, and
  the cached model context is released with the controller.
- Added continuous listening through `Ctrl+G`, empty-composer Space, and bare
  `/voice`; VAD finalizes segments without closing the mic, Ctrl+G ends the
  session, Enter ends and submits, Esc cancels a segment/then closes, and Ctrl-C
  cancels immediately.
- Added recording, initialization, download, finalizing, and paused titles.
- Added footer microphone level meter.
- Added marker-aware committed insertion that follows normal composer edits.
- Added virtual dimmed tentative text without putting it into history or
  submission state.
- Added segment-tail insertion, multi-segment spacing, edit-before-send
  behavior, and a session-end-only auto-submit gate.
- Reused existing steer/queue submission behavior for active agent turns.
- Added `/voice status`, `/voice on|off`, a selectable `/voice model` picker,
  and direct `/voice model <name>` selection.
- Hid the command, hints, and hotkeys over SSH and in unsupported builds.
- Added actionable error display while preserving committed text.
- Smoke-tested the release TUI and `/voice status`; it reports continuous mode,
  800 ms segment silence, 30 s idle timeout, and the 10-minute session cap
  without triggering model download or microphone access.
- Installed no-op Whisper/ggml logging hooks before model initialization so
  native Metal/model diagnostics cannot write over Ratatui and corrupt chat.

## Pending

- Manually verify typing and cursor movement before/after the voice marker.
- Manually verify multiline wrapping and dim-tail rendering at narrow widths.
- Verify first Esc preserves committed text and cancels the active segment,
  while a later Esc closes listening without clearing the composer.
- Verify auto-submit below/above the gate, including CJK.
- Review spacing when dictation is inserted into the middle of existing text.
