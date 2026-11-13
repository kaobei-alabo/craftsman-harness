# Agent Client Protocol

`craftsman acp-serve` speaks standard stable ACP v1 over newline-delimited
JSON-RPC 2.0 on stdin/stdout. `craftsman acp-bridge --port 8080 -- craftsman
acp-serve` carries the same frames over WebSocket and exposes the existing
health/statistics endpoints. The retired `serve` and `desktop-serve` commands
are not restored. The [compatibility audit](acp-compatibility-audit.md) records
the original failures, pinned official schema and subsequent validation.

## Sessions and directories

Initialize the connection, then call `session/new` with an absolute existing
`cwd` directory and `mcpServers` array. The agent returns its session ID; include
that ID in subsequent prompts, loads and cancellations. Each session owns its
runtime, canonical directory, configuration, approvals, usage and output
callbacks. Relative filesystem and shell tools use that directory; the process
working directory and user configuration are not changed.

The connection retains up to 64 sessions and runs one main prompt at a time.
During a prompt, another new/load/prompt/model-change request returns busy
(`-32001`). This prevents session replacement or output relabelling. Idle
sessions can be selected independently by their IDs and retain separate history.
Unknown prompt IDs are refused. `session/load` replays a known live conversation
or restores an actual stored checkpoint, then returns an object. An unknown
stored ID returns an error and is not available for a subsequent prompt. Cached
loads must match that session's directory and MCP configuration. For a stored
session reopened in a new connection, the caller supplies the new runtime's
directory and MCP configuration; persistence is not a filesystem confinement
boundary.

`_meta.craftsman.threadId` remains an optional create-only extension on
`session/new`. Custom IDs must contain 1–128 ASCII letters, digits, dots, hyphens
or underscores and cannot be `.` or `..`. Unsafe/malformed values and already
saved IDs are refused, rather than silently replaced or used as journal paths.
Use `session/load` to resume a saved ID.

## Permissions and history

Every reported invocation gets a presentation ID independent of the model's raw
call ID. The tool start, permission request and actual completion share that ID;
a repeated model ID in a later round does not reuse it. Delegated invocations
receive their own IDs at the actual tool wrapper, with start and completion
updates based on the real outcome. The model's original IDs and portable
assistant/tool messages are retained internally.

Permission requests offer `allow-once`, `allow-always` and `reject-once`. Only a
standard `result.outcome` of `selected` with an offered option ID grants
permission. Unknown options, booleans, loose strings/objects, missing selections,
errors and cancelled outcomes refuse it. The existing shared permission policy
still determines whether a tool needs approval and what “Allow always” changes.
No callback reads the JSON-RPC input pipe as terminal approval input.

Load replay includes user/assistant content and committed tool calls/results,
including denied/error results. Replay IDs distinguish separate assistant/tool
groups even when a raw model call ID repeats. An unmatched historical result is
shown as content with `_meta.craftsman.orphanedToolResult`; an unresolved call
stays pending. Replay invokes no model, tool, approval callback or turn hook.
Assistant content chunks keep one `messageId` per model round and use a distinct
ID for later rounds and model replacements.

## Cancellation and shutdown

`session/cancel` first requests cancellation on the actual active agent, then
refuses that session's permission/question waiters. It closes admission for late
prompts before releasing waiters. The running turn owns its completion; it sends
all foreground tool and A2UI updates before its final `cancelled` response.
Stale permission replies cannot grant the cancelled operation. The optional
usage metadata includes accumulated completed provider responses, including on
cancellation; it does not describe remote billing for an interrupted request.

ACP explicitly opts into dropping pending **main-agent** model sampling. Tools,
compaction and delegated/background model work retain their existing cooperative
behavior and cleanup/time limits. ACP does not select the terminal's durable
checkpoint/native cancellation scope. A cancellation is not immediate native
process interruption, remote request cancellation, rollback or OS containment.
Background work is independent of the main prompt and may produce its own
session activity; this audit's final-response checks cover foreground turn work.

On EOF the server stops admitting prompts, cancels an active main turn, refuses
all parked UI waiters and drains actual turn/background/LSP owners and output
under one 30-second bound. Timeout is an error that states work may still be
completing or final frames may be undelivered; it does not claim quiescence.

## MCP and model selection

Standard ACP stdio `mcpServers` entries (`name`, `command`, `args`, and an array
of `env` name/value pairs) are added to the session configuration. Children start
in the session directory, and supplied environment values are literal. Existing
user-config server names are preserved; duplicate names/keys, malformed entries,
`${...}` expansion in client environment values, unsupported HTTP/SSE entries
and connection failures are explicitly refused. The existing MCP client protocol
and its 30-second RPC bound remain unchanged. Additional directories and client
system prompts are unsupported and are refused when supplied.

`_craftsman/session/set_model` accepts `modelId` and an optional `sessionId`;
without a session ID it uses the currently selected session. Active prompts or
running background tasks refuse replacement. A successful rebuild uses the
existing guarded idle conversation transfer, retains committed history,
thread/sequence, effective approvals and accumulated main usage, and keeps the
new runtime's model/tools/registry. A failed rebuild retains the old session.
The transfer performs no sampling, tool execution or history replay.

Craftsman-only methods remain advertised in `_meta.craftsman.extensions`:
`_craftsman/session/request_input`, `_craftsman/session/set_model`, and
`_craftsman/session/list`. Thread IDs, A2UI and usage annotations use
`_meta.craftsman.*`; A2UI content remains an ACP resource block. Unadvertised
optional ACP methods and capabilities are outside the implemented surface.
No new major protocol version, SDK/WIT, SQL layout or dependency pin is introduced.

## Validation

`cargo test --locked --no-default-features -p craftsman-rs --test golden_frames`
runs the retained stdio/bridge recordings and seven additional native audit
regressions. Those regressions independently check real file effects, unknown
options and loose replies, directory/session isolation, SQLite reopen/replay,
reused IDs, delegated approval/results, EOF/cancellation ordering and model
transfer. A marked ignored native MCP child checks actual launch directory,
literal environment, discovery and tool execution; unmarked runs return before
reading stdin, environment or configuration. Both full native feature modes and
all four hosted validation jobs remain required before delivery.

The two intentional ACP golden diffs correct permission/tool/message identity
and include accumulated usage on the now normally completed cancellation path.
The bridge recording, model-request goldens, schemas, native cleanup limits,
SDK/WIT and publication hold are unchanged. These checks establish the tested
ACP behavior; they do not establish paid-model quality, every third-party client,
full MCP-version interoperability, public headless execution or OS containment.

The ACP bridge accepts clients without a platform inbox worker. Legacy platform
credentials do not start polling or scheduled agent turns. Its retained HTTP
health/stats, proxy headers and ACP frames are unchanged; see
[connections](connections.md).
