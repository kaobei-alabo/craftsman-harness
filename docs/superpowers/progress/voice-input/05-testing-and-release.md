# Testing, release, and ship gate

Status: **implementing**

## Implemented

- Added config round-trip coverage for unknown nested voice keys.
- Added model alias/default, LocalAgreement, CJK word gate, resampler, and error
  mapping unit tests.
- Added TUI tests for activation keys, virtual tentative text, final insertion,
  Esc preservation, auto-submit gating, and hidden unavailable commands.
- Confirmed targeted voice tests pass in the voice-disabled build.
- Confirmed the native dependency stack compiles locally after installing
  CMake.
- Updated README, release workflow, installer, design model names, and the
  implementation plan.
- `cargo fmt --check`, default/no-default `cargo check`, and targeted
  default-feature voice tests pass after final code edits.
- The final full locked workspace suite passes: 97 framework tests, the
  dependency guard, 256 Craftsman tests (1 ignored), and 3 loop integration tests.
- The locked local release build passes with the default-on voice stack.

## Pending

- Execute all manual ship gates from the design specification.
- Change every progress topic and the index to `implemented` only after these
  checks pass.
