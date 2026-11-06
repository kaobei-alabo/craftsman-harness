# Terminal cancellation

Ctrl-C reaches the active terminal turn while an approval, an agent question,
or another dialog is open. The TUI also interrupts a pending main-model request,
including a wait for HTTP headers or further streamed content. Running tools and
other lifecycle work keep cooperative cancellation and their existing cleanup.
Previously approval pickers ignored the key; feedback and question fields typed
`c` into the input. That left the agent waiting for a response.

## Ordering and behavior

1. The key handler marks the UI turn as stopping. It preserves prompt data until
   the main loop handles the actual cancellation request.
2. `session_bridge::apply_turn_cancellation` calls the real
   `Agent::request_cancel` before releasing any prompt waiter. The resumed tool
   loop therefore sees its cancellation at the next existing safe boundary.
3. Visible and queued approvals receive `Deny("")`; questions receive `None`.
   A highlighted "Always" choice never becomes a grant and partial answer or
   feedback text is not submitted. Every pending prompt sender is released.
4. The existing persistent stopping flag also refuses approval/question events
   that arrive later through the UI channel. Consuming the one-shot pending
   request flag does not re-enable those prompts.
5. Actual turn completion resets the UI stopping state. Subsequent prompts keep
   the ordinary approve/deny/answer behavior.

Unrelated editor/setup/selection dialogs and composer drafts retain their content
and order. When only a background prompt is visible and no main turn is active,
Ctrl-C refuses that prompt without creating a cancellation for a later main turn.
Control/Alt shortcuts in prompt text fields do not type their letter as feedback.
Normal text, including Shift-modified uppercase characters, still works.

Outside dialogs, existing selection-copy, Ctrl-Shift-C copy and voice-cancellation
behavior retains its precedence. Voice capture active under a dialog is also
cancelled when the dialog's Ctrl-C requests a stop. Rendering still passes through
`SafeBackend`; this change adds no terminal escape or public protocol channel.

## Validation

The model-wait implementation adds an explicit framework builder opt-in,
`interrupt_model_on_cancel`, default false. The TUI selects it through
`UiBridge::session_extras`; the shared application builder refuses to enable it
for private prepared runtimes. Other frontends and children retain the default.
The opted-in provider and `around_model` middleware must own their I/O and be
safe to drop without asynchronous cleanup. The normal terminal provider awaits
its HTTP/stream work directly; it does not spawn a detached request task.

A notification wakes the pending sampler; the mutex-protected active-turn flag
remains cancellation authority. Registering the waiter before checking that
flag closes the lost-notification window. Cancellation is checked before polling
a ready model future. If the provider itself requests cancellation while
returning a complete response in the same poll, the original response/usage is
committed and its tool calls receive cancelled results. A dropped incomplete
response adds no assistant message or completed usage. Partial screen text is
not a completed conversation result. Closing the local HTTP connection does
not prove remote generation or billing stopped, and unchanged usage aggregates
are not a claim that the interrupted request consumed zero tokens.

Six additional framework cases cover stalled requests, fresh subsequent turns,
the ready-response race, owned tool completion, cancellation before waiter
registration, model-wrapper cleanup before turn-end and original error types.
Two TUI cases drive the real key handler, bridge, `StreamingLlmProvider` and
loopback HTTP client. The synthetic peer never completes its response; after
Ctrl-C the real connection closes both before headers and after a partial SSE
delta. They use bounded waits and no sleeps, live model credentials, user config
or installed gadgets. Focused checks pass. The full local run preserves every
original named/ignored/doctest entry and passes core **1903 / 0 / 10** and gadgets
**2058 / 0 / 12** (passed/failed/ignored), with all eight additions in both modes
and no compiler warnings. The actual scaffold/edit/build/discover/call gadget
test also passes. Its script exit **1** is retained: provenance rejected a new
model-cancellation documentation phrase in the framework source. That phrase
was corrected without a new allow-list exception. Targeted formatting,
locked core/default builds, provenance and whitespace checks pass on corrected
source; the complete suites were not retried unchanged. Raw full evidence and
the inventory audit are retained in `target/verification/`. All four hosted
jobs/every step and independent merged-main verification still gate completion.

The original prompt-cancellation evidence below is retained separately.

Two new regression tests first reproduced the swallowed Ctrl-C behavior. The
implementation adds real synchronous UI bridge and framework Agent coverage:
both approval and question waiters return refusal, the agent records its existing
"Stopped by the user." outcome, and a second tool in the same model response
never runs. No further model-provider call occurs.

Further cases exercise multiple queued waiters, highlighted grants, editor and
composer preservation, idle background prompts, late real bridge callbacks,
normal prompts after completion, editor cancellation and modified text keys.
Tests use synthetic providers and in-memory channels, with bounded waits. They
do not open user configuration, run an installed gadget or spend model credits.
Existing tests/assertions, native deadlines and framework behavior stay intact.
Full local and hosted validation must be recorded before merge.

The first complete verification at `9104a04` retained core **1887 / 0 / 10** and
gadgets **2042 / 0 / 12** (passed/failed/ignored), with all nine new cases passing.
The overall script still exited **1**: the provenance checker rejected the new
interpreter post-step wording, and compiler checks reported a newly unused
`TranscriptKind` import. The import now lives only in the existing test module
that uses it, and the documentation uses ordinary interpreter terminology. No
warning suppression, broader provenance exception, changed assertion or deadline
is used. This first run remains separate from corrected-source verification.

Corrected-source verification at `0745e6e` passed all nine `scripts/verify.sh`
steps, exit **0**, with no compiler warnings: core **1887 / 0 / 10**, gadgets
**2042 / 0 / 12**. Every original named/ignored/doctest entry remains present,
with exactly nine cancellation cases added in each mode. The 30 request-budget
cases and the actual scaffold/edit/build/discover/call agent test also pass.
Formatting, locked core/default builds, exec conformance, benchmark-driver
contracts, provenance and unchanged goldens passed. Raw first/corrected logs and
inventory audits remain under `target/verification/`. Hosted checks and any
merged-main run are separate evidence; live model/microphone/native-cancellation
latency and full P2c/P2g readiness are not established by these tests.

## Remaining lifecycle work

The main TUI model wait and human-input waits are interruptible. A running
shell/build, blocking filesystem operation, preparation/compaction hook or
foreground subagent still finishes its owned operation before the main loop
stops. A
background agent that continues after main-turn completion has its normal prompt
lifecycle. There is no whole-process quiescence or rollback claim.

The accepted P2c/P2g work still requires shared cancellation through every role,
signal routing, terminal restore and bounded native cleanup. Unix process groups
remain off by default. Desktop v4, standard ACP, public exec availability,
usage/settlement and headless gadget registration are unchanged.
