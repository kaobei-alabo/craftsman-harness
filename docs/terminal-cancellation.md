# Terminal prompt cancellation

The working terminal agent uses cooperative cancellation. Ctrl-C can now reach
the active turn while an approval, an agent question, or another dialog is open.
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

## Remaining lifecycle work

This change releases human-input waits and reaches the existing cooperative
stop checks. It does not immediately interrupt a successful provider stream,
running shell/build, blocking filesystem work, or a foreground subagent. A
background agent that continues after main-turn completion has its normal prompt
lifecycle. There is no whole-process quiescence or rollback claim.

The accepted P2c/P2g work still requires shared cancellation through every role,
signal routing, terminal restore and bounded native cleanup. Unix process groups
remain off by default. Desktop v4, standard ACP, public exec availability,
usage/settlement and headless gadget registration are unchanged.
