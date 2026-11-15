# Terminal status reporting

Craftsman's interactive terminal reports its lifecycle through the
[Program Status Protocol (OSC 7501)](https://www.superlogical.com/rex/docs/build/program-status).
Compatible terminal applications can show working, approval, question,
completion and error indicators without parsing the chat or its spinner.
Reporting is enabled by default when running the interactive TUI. Standard
ACP, the ACP bridge, noninteractive invocations and framework/private runtimes
keep their existing output; OSC records never enter ACP JSON-RPC stdout.

To disable it, add this setting to `~/.craftsman/config.json`, preserving the
other terminal settings:

```json
{
  "terminal": {
    "max_rounds": 200,
    "osc7501": false
  }
}
```

The missing setting defaults to true. Only boolean values are accepted;
invalid settings refuse interactive startup before raw terminal entry or
session construction, with fixed text that does not echo the value. Craftsman
does not rewrite the user's configuration to enable reporting.

## Lifecycle semantics

| Actual terminal activity | Report |
| --- | --- |
| Waiting for the next instruction | `state=idle` |
| Initializing/reconfiguring, executing a turn or a foreground helper | `state=working` |
| Displaying an approval or awaiting a suspended approval | `state=blocked:kind=permission` |
| Displaying an agent question | `state=blocked:kind=question` |
| Paused at the terminal turn budget; needs a follow-up | `state=blocked:kind=question` |
| Main turn returned after its actual end hooks and save | `state=done` |
| Main turn or a typed catalog operation returned an error | `state=error` |
| Cancellation requested but owned work has not returned | `state=working`, message `Stopping` |
| Cancelled turn returned successfully | `state=idle` |

An actual save failure remains an error even after cancellation. A tool failure
inside a continuing turn does not turn the whole session into `error`; the
agent may still repair it. Existing untyped inline-shell/manual-compaction
completion events return to `idle` without inferring success from their text.
Model/effort/permission menus and editor drafts are not agent questions.

`done` means the response is ready. It is not independent task-quality,
verification, usage/billing or process-containment evidence. The configured
verification policy, permissions, recovery and native cleanup limits remain
authoritative. Cancellation does not establish that a remote model stopped
generating or charging.

Tracked delegated tasks have separate child records. Their real registry
statuses map `Running` to `working`, `Suspended` to permission-blocked, `Done`
to `done`, and `Failed` to `error`. A scoped background stop remains working
while its actual cleanup runs. The root can have a response ready while a
child is still working; both records are retained. While model reconfiguration
temporarily hides the old registry, its child records remain until the new
session is ready. Removing an unobserved child clears its record and does not
invent a completion.

On normal exit, after the existing owned cleanup returns, Craftsman clears
only its child namespace and reports the last ready/error outcome or idle for
interrupted work. Idle exit does not manufacture another work/completion cycle.
Cleanup timeout, drawing failure or save failure reports an error. The existing
30-second cleanup error still says work may be completing. Panic restoration
attempts one fixed error report, seals further reports and never prints the
panic payload; this is best-effort terminal reporting, not cleanup proof.

## Wire format and terminal safety

For example, an approval report has this form (`ESC` is byte `0x1b`):

```text
ESC ]7501;state=blocked:app=craftsman:kind=permission:msg=V2FpdGluZyBmb3IgYXBwcm92YWw= ESC \
```

The message decodes to `Waiting for approval`. Every report repeats
`app=craftsman` because a report replaces its whole record. Reports use the
escape-byte/backslash terminator, fixed single-line base64 messages and no fabricated
percentage. No heartbeat or terminal query is sent, and no input is consumed
for feature detection. Unsupported terminals ignore the sequence; the
terminal host chooses its own presentation.

Only the owned TUI's `TerminalGuard` emits the records. It accepts typed states
and locally assigned numeric child ids, not arbitrary terminal text. Prompts,
tool arguments/results, repository content, model names, endpoints, credentials,
task names and progress strings never enter the OSC messages or ids. All normal
rendering continues through `SafeBackend`, including text that attempts to
forge OSC 7501 or other terminal control sequences.

The reporter retains at most 32 individual child records, with private name
lookup keys bounded to 512 bytes, plus one aggregate for omitted live tasks.
The aggregate remains working or permission-blocked according to observed
statuses. Numeric ids never wrap or recycle. Reclaiming a finished slot clears
its old record before publishing the new one. Children use
`craftsman/tasks/<number>`; shutdown clears `craftsman/tasks`, leaving unrelated
child namespaces alone. Each report is under the protocol's 4096-byte cap;
stored reporter state and each batch are bounded. Registry observation retains
the existing `AgentRegistry::list` API and cost.

Only changes are written and flushed. Cache state commits after successful
output; a partial write or flush failure disables further status writes and
enters the existing terminal error/cleanup path. There is no status IO retry,
new provider request, transport retry or model/tool replay.

## Orca and ACP

The inspected Orca implementation accepts OSC 7501 with arbitrary `app`
identifiers, including `craftsman`; its
[status projector](https://github.com/stablyai/orca/blob/c0bc27438dfabddffcf900fc6d0aa8830a0f22e6/src/shared/program-status/agent-status-projector.ts)
maps root and child records into its agent status view. Run the ordinary
`craftsman` terminal command in a compatible managed terminal to emit these
reports. Native Orca chat still requires its own Craftsman ACP registration;
this feature does not install host-specific hooks or change Orca settings.

ACP remains the detailed conversation/tool/permission interface. See
[ACP integration](acp.md) and the separate
[owned terminal lifecycle](terminal-lifecycle.md).

## Validation

New tests cover protocol syntax, base64/control safety, fixed identities,
change-only output, child capacity/overflow, reconfiguration, id exhaustion and
partial-write/flush failure. A marked isolated child exercises actual UI
callbacks and the actual terminal builder, loopback HTTP, approved edit,
question/answer and SQLite final checkpoint. New Unix PTYs exercise the real
TUI/status writer through approval, question, completion, cancellation, error,
opt-out and panic, recording actual modes before process exit. Only Darwin's
kernel-managed `PENDIN` bit is excluded from the new configured-mode comparison.
Helpers refuse unmarked/unknown startup before config or terminal access.

The first new Darwin PTY initializer's pointer-type failure and the raw-word
capture failure are retained separately. The corrected test checks escaped
rendered control text and the whole actual saved answer; original assertions,
native deadlines and production runners remain intact. Full local/all-four
hosted/every-step/both-native-mode inventories and warning/tree audits gate
merge, with independent main validation before local fast-forward/install.
No paid model call, user/global environment mutation, SDK/WIT/dependency/SQL
change, frame/golden update or binary/container publication is involved.

Entry points: `src/tui/program_status.rs` (codec/cache and bounded task records),
`src/tui/terminal.rs` (owned writer/restoration), `src/tui/app.rs` (actual root
lifecycle), and `src/tui/session_bridge.rs` (helper/catalog returns), all under
`crates/craftsman-rs/`.
