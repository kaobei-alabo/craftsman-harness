# craftsman-harness

A Rust terminal coding agent that edits files, runs tools, delegates work and
can build reusable **WASM gadgets** for itself. The workspace contains the
**`craftsman-harness`** agent framework, **`craftsman-rs`** application and the
gadget SDK/macros. Model routes include OpenAI Responses, Claude Messages,
Gemini GenerateContent and OpenAI-compatible Chat Completions; platform skills and MCP
are optional integrations. See `docs/platform-token-refactor.md` for token
configuration.

The canonical repository is **[kaobei-alabo/craftsman-harness](https://github.com/kaobei-alabo/craftsman-harness)**.
On November 5, 2026 the project moved to its new owner with a fresh source-only
Git history. The [handover](handover.md) and dated notes in `docs/` preserve
engineering context from the inherited project; old commits and hosted checks
are historical evidence. The `craftsman` CLI, `craftsman-*` Cargo packages,
`.craftsman` configuration and `CRAFTSMAN_*` environment names are retained.
See [the identity migration notes](docs/repository-name.md) for existing-data
handling and the earlier namespace change.

Start with [production readiness](docs/production-readiness.md) for the current
overview, build/setup steps, self-built gadget workflow, validation record and
remaining gates. [Gadget authoring](docs/gadgets.md) describes the implemented
capability/approval boundaries. Run `./scripts/verify.sh` for both complete test
modes and the default build; it selects an installed Python 3.11+ interpreter.

[CI cost controls](docs/ci.md) keep draft PRs on formatting and small contract
checks, with the full platform matrix triggered when a PR is marked ready and
on main. Selective Cargo dependency caches preserve fresh workspace and gadget
fixture builds; full current-head validation remains required before merge.

The [ACP compatibility audit](docs/acp-compatibility-audit.md) preserves the
original stable-v1 failures and subsequent repair evidence. The corrected
adapter passes the same 39 local strict-client checks, with native regressions
for session directories/identity, exact permissions, MCP wiring, replay and
cancellation ordering. [ACP behavior and limits](docs/acp.md) describes the
implemented interface; full hosted/native validation remains a separate gate.

Native shell regression tests check actual stdout/stderr, exit and timeout
metadata, invalid UTF-8 refusal and stopping a running command after an approval
failure. Temporary test-only shell investigation instrumentation has been retired.

Binary publication is on hold at the owner's request. The release, workspace
image and service-agent image workflows have been removed and disabled in GitHub.
CI validates source; the optional gadget benchmark uploads only its JSON report.
Restoring any binary or container publication requires a new reviewed change and
the owner's explicit approval.

**New implementation:** the private
[request-budget adapter](docs/request-budget-arguments.md) accepts `--max-rounds`
(default 50, range 1..10000) and passes the selected limit to the existing shared
main/delegate/compaction meter. It preserves the original input and file-output
owners. Public `craftsman exec` remains unavailable. The new CI uses actual
macOS 15 labels, requires Windows tests and checks the default voice/gadget build.
The application declares Rust 1.99; the compiler remains pinned to 1.99.1.

This repository is the maintained Craftsman CLI implementation. Streaming, HITL
approval, planning, subagents, durable sessions and memory, MCP, skills,
model-aware harness profiles, WebSocket serve mode, and A2UI are implemented in
Rust.

**Implementation status (November 4, 2026, UTC):** gadgets and their authoring/migration
path are implemented and opt-in; headless v1 is accepted but `craftsman exec` is **not
yet exposed**. Immutable permissions, confined filesystem, explicit config/
discovery/session preparation and a shared-builder runtime adapter are private
implementation, not a complete unattended runner. Private byte-safe gadget
filesystem and source/approval snapshots reuse retained authority, including
explicit config handles. A private guest host now consumes them through an opaque
runtime-scope lease, with empty-capability describe and shared import-failure
observation. Captured credentials and bounded byte/text/structured redaction
merged as #104 with one frozen provider key shared by every role. The private
host now redacts guest JSON and shared stdout/stderr/logs before retention cuts,
using the SAME scope and fail latch (#105). Private shell stdout/stderr and early
empty-command errors now select frozen-seed redaction before head/tail cuts and
model ToolOutcome formatting, with original command/byte/status authority retained (#106).
Private filesystem views now consume a separate actual-builder scope: complete original
byte coverage precedes pagination, original line/query/path/mutation authority stays
separate, and formatting is redacted/verified before output cuts. Fixed gate denials
also select captured seeds (#107). Private web fetch/search now consume an actual-builder
WebScope: original bytes are redacted before body cuts, bounded HTML decoding is checked
again, and complete search JSON is structurally redacted before result/snippet selection.
Original URLs/queries/captured literal auth and typed attempt/byte facts stay separate (#108).
Concrete built-in todo/task/message results now select a separate actual-builder
FrameworkScope before the framework result budget. Original typed todos, task dispatch,
file merges and message delivery stay separate from checked views; fixed todo schema/status
collisions fail, not rename authority, and todo context views are checked too (#109).
Private main/child/direct-compaction provider requests now use an actual-builder
ProviderScope: copied text/argument/annotation views are checked before JSON encoding,
fixed identities/keys/schema/generation constraints are unchanged or refuse, and exact
checked bytes are bound to one client. Original dispatch/state/auth remain separate (#110).
Private HTTP/error diagnostics now use actual-client send/receipt witnesses and a per-request
lease: frozen-seed UTF-8 views precede retention/400-scalar cuts, with original observations
separate and all detail discarded on unsafe cuts. The bounded last failure is not a usage
ledger or public receipt. Successful SSE/control/state remain unselected (headless section 25).
These are not public runners: successful-response/transport/alternative-state/storage/output adapters and
compatible gadget provenance remain open. Human output, raw persistence and callable gadgets stay disconnected. See [docs/runtime-status.md](docs/runtime-status.md)
for the current architecture, merged/partial/planned work, limits and next gates.
A separate private input-first adapter now admits one explicit instruction and
bounded UTF-8 attachment files through the retained confined authority **before**
thread/store creation. Original data is model-only input, not a permission grant;
the old routes still refuse stdin, while the separate private byte-pipe route below
adds explicit capture; `exec` remains unavailable. See
[docs/headless-input.md](docs/headless-input.md) (#115). The actual runtime now also
selects bounded checked snapshots of retained messages without cloning or replacing
original state/control; files/todos/custom/child state and public output remain
unselected. See [docs/retained-message-views.md](docs/retained-message-views.md).
A separate private whole-message selector now tightens only those checked snapshots,
keeping a newest whole-record suffix with exact omission counts and rechecked framing;
original outcome/state and terminal/public publication remain separate. See
[docs/message-selection.md](docs/message-selection.md).
A separate private terminal authority now consumes the actual returned Run, permanently
closes execution and retains checked finalization fragments under an independent lease,
even after a real approval stop. It is not a public outcome/IO writer. See
[docs/terminal-publication.md](docs/terminal-publication.md).
That authority now has a separate private whole-message selector which preserves the
complete checked answer/facts and applies exact combined-fragment bounds only after
both channels pass admission. See
[docs/terminal-message-selection.md](docs/terminal-message-selection.md).
A separate private ASCII encoder now checks full projected channels, preserves the
complete answer/facts and selects whole messages by exact post-escape byte budgets.
It is not a public schema/IO or human-channel adapter. See
[docs/terminal-ascii.md](docs/terminal-ascii.md).
A separate private human renderer now checks the actual complete final answer and
fixed private-fact diagnostics, visibly escapes terminal controls and defangs
adjacent runner-command delimiters within an exact combined byte budget. It keeps
the same lease/typed-stop guards, not public text eligibility or stdout/stderr IO.
See [docs/terminal-human.md](docs/terminal-human.md).
A separate private file-invocation route now captures an explicit outside-workspace/
config destination before session creation, then publishes only its actual returned
terminal's checked fragment/preview with one guarded create-only attempt. Native
file IO is private and bounded; this is not public result/text/stdio/JSONL output.
Native Windows directory-handle fixtures keep production authority unchanged.
Each content pass counts seek/payload/EOF inside the unchanged native-call ceiling.
Final native results, including failed owned cleanup, recheck the original lease.
See [docs/guarded-output.md](docs/guarded-output.md).
A separate private stdin invocation now captures explicitly selected native byte
pipes with bounded UTF-8 and first-byte/total-read clocks before session creation.
Old input/file-output routes and public CLI stay unchanged; no blocking reader
thread, implicit stdin read or console/file support is claimed. See
[docs/named-stdin.md](docs/named-stdin.md). The first hosted revision failed both Windows
builds; native Pending/EOF, access-rights and synchronous-handle corrections keep
the real stdin/deadline assertions intact. A second hosted revision fixed false
EOF but still failed five real-stdin cases in each Windows build. Current Windows
polling preserves original grants: owned NOWAIT where available, otherwise native
readiness before any read-only data read; no access upgrade/blind idle read or
background reader. External-reader/native-call limits and fresh Windows evidence
remain mandatory; this is not a public stdin/frontend completion claim.
A separate private stdin/file invocation now captures the original guarded target
before stdin inspection, rechecks it after complete input admission and before
sessions/providers, then uses the unchanged paired FileRun/FileTerminal publisher.
There is no raw Run/target pairing, new reader/writer or public exec/stdio route.
See [docs/stdin-file-output.md](docs/stdin-file-output.md).
Its first hosted Windows run failed a new raw-path fixture and two unchanged
native-shell cases. Only the new fixture is corrected here; final-head checks
must also pass the original shell cases, despite Windows CI being informational.
That composition subsequently merged as #129 after all four corrected-head jobs
and every mandatory step passed, including both Windows builds. A new **private
instruction-argv route** now validates positional prompts, `--prompt-file`,
repeated `--file` and exact `-` before config/stdin inspection, then uses the
existing memory or guarded-file invocation chains. It is only the source/attachment
subset: full frontend/output/usage/deadline flags are refused, not ignored.
See [docs/input-arguments.md](docs/input-arguments.md); `craftsman exec` stays unexposed.
The first argument-source PR's full CI passed all new/native/composition cases,
but two unchanged Linux guest tests timed out. A focused SOURCE fixture now keeps
those IO/deadline behaviors and all describe probes without unrelated guest code;
original host/SDK/WIT/budgets/assertions remain unchanged. Fresh all-job/every-step
checks are required; see [docs/guest-fixture-loads.md](docs/guest-fixture-loads.md).
A separate private control/source factory now parses supported absolute scope,
explicit permission/env and session controls before IO, using the unchanged
Bootstrap and source/run owners. Unsupported output/usage/trace/gadget/deadline
and relative scope flags still refuse, not ignore. No `craftsman exec` route ships;
see [docs/control-arguments.md](docs/control-arguments.md).
The pre-exhaustion token-admission contract is **explicitly accepted November 1,
2026**, including optional metadata, truthful usage/calls and the documented v1
validator-compatibility risks. See [docs/token-admission.md](docs/token-admission.md).
This resolves #97's contract decision (#133 merged), not the complete token/usage
meter or public `exec`. A private shared logical-request cap now covers main,
subagent and compaction providers through one actual-builder source; usage/
HTTP receipts and settlement remain unwired. See [docs/request-admission.md](docs/request-admission.md);
public limit flags still refuse rather than being silently ignored. A separate
private atomic token-reservation foundation is implemented; its allocations remain
unsettled, not measured usage or public #97 facts. See
[docs/token-reservations.md](docs/token-reservations.md). CI now uses explicit
macOS15 after GitHub's November2, 2026 macOS14 retirement; the legacy required
`test (macos-14)` name is preserved, not a claim of macOS14 coverage. Test steps,
limits and branch protections are unchanged. A following private
admission-correlated send/header foundation now records actual polls/returned
headers/errors without inferring usage or settling reservations; see
[docs/admission-receipts.md](docs/admission-receipts.md). The same
slice includes a separate necessary security commit pinning the Wasmtime host to
48.0.8 for the AArch64 sandbox-escape and later48.x fixes; no capability/default-on
or release approval follows.
A November 3 maintenance correction replaces only the Windows partial-timeout
test input with direct flushed console writes and a blocking sleep. The original
2-second limits/assertions remain; a failed post-merge Windows baseline step is
not green merely because the informational workflow badge says success. See
[docs/windows-shell-fixtures.md](docs/windows-shell-fixtures.md) for the preserved
failure, additive native cases and all-platform verification gate. This does not
expose headless `exec` or complete usage/settlement.
A separate private adapter resolves bounded normal-relative scopes against an
explicit launch base and selects `--output-file` through the SAME guarded file
owner; public output-format flags still refuse instead of relabeling previews.
See [docs/scope-output-arguments.md](docs/scope-output-arguments.md).
The historical baseline and target diagrams remain in the harness plan.

```
crates/
  craftsman-harness/   framework: AgentMiddleware protocol (6 hooks), Backend
                   trait (StateFs), the Agent model→tools loop, and
                   built-in middleware for planning, tool-call repair, HITL,
                   subagents, memory, compaction, reminders, and result budgets.
                   Framework-only dependencies; no HTTP client, database, or UI.
  craftsman-rs/        the app: TUI, RealFs backend, HITL approval middleware,
                   route, MCP, platform REST, host tools, config, `craftsman` bin;
                   opt-in gadget host and private exec/ foundations.
  craftsman-gadget/   standalone typed gadget SDK and WIT contract.
  craftsman-gadget-macros/ proc macros for the SDK; tested independently of the host.
```

Requirements for the default voice-enabled build:

- the exact Rust toolchain in `rust-toolchain.toml` (currently 1.99.1; public crates only, no private git dependencies);
- CMake on macOS and Linux (`brew install cmake` on macOS);
- on Linux, `pkg-config`, Clang, and ALSA development headers.

```bash
./install.sh                 # install `craftsman` into ~/.cargo/bin
./install.sh --upgrade       # rebuild + replace an existing install
./install.sh --no-locked     # re-resolve Cargo.lock
                              # Remove with: cargo uninstall craftsman-rs

craftsman                        # interactive REPL
craftsman --session THREAD_ID    # resume a persisted conversation
craftsman -m "one-shot prompt"
craftsman --no-mouse             # optional terminal-native selection mode
craftsman setup                  # guided first-run configuration (LLM + platform)
craftsman service-agent --request request.json  # restricted shared-runtime turn
craftsman acp-serve              # Agent Client Protocol server over stdio
craftsman acp-bridge --port 8080  # WebSocket to this binary's stdio ACP server
craftsman-rs --version           # legacy compatibility command
```

Platform access is headless and token-configured (no login command): the
platform's LLM app is a stock OpenAI-compatible endpoint (`base_url` +
`api_key` in `~/.craftsman/config.json`, with `${ENV_VAR}` expansion), the
platform's `/mcp` is an ordinary `mcp_servers` entry, and
`platform.api_token` (or `$CRAFTSMAN_PLATFORM_TOKEN`) backs skills REST and the
workspace inbox. See `docs/platform-token-refactor.md`.

### Execution profiles

The retained CLI modes use local or restricted service-agent capabilities. The private
headless construction profile is test-only, not another runnable CLI surface.
Filesystem authority does not imply OS containment:

| Profile | Commands | Filesystem | Shell | Session storage |
|---|---|---|---|---|
| `local` | `craftsman`, `craftsman -m`, `acp-serve` | Real user filesystem | HITL-controlled native shell; no Craftsman OS sandbox | Local `~/.craftsman` |
| `service-agent` | `craftsman service-agent` | Scoped virtual files plus selected public-web tools | Unavailable | Supplied/owned by the external runtime |

`acp-bridge` is a transport front door, not a separate filesystem profile.
Deployment isolation remains external policy. ACP uses standard frames and
declared Craftsman extensions; the custom v2/v4 WebSocket servers are retired.
Legacy `-m` now denies non-TTY approval reads without blocking, silences child/
compaction deltas and stops quietly on a broken stdout pipe. It still honors
legacy user-config permissions and local storage; these fixes do not supply the
future `exec` isolation, shared budget, machine-output or signal contract.

`service-agent` never constructs `RealFs`, never registers `execute`, and never
opens local threads, memories, project skills, LSP, or checkpoints. Its file
shaped tools operate only on request-provided virtual mounts: `/skills` is
read-only, `/scratch` is writable and ephemeral, and `/memories` may be enabled
by a governing runtime.

The command reads a versioned JSON request from `--request PATH`, or from stdin
when the flag is omitted:

```json
{
  "version": 1,
  "system_prompt": "Use the approved skills when relevant.",
  "input": "Read the incident-response skill and summarize the procedure.",
  "messages": [],
  "allowed_tools": ["read_file", "ls", "glob", "grep"],
  "files": [
    {
      "path": "/skills/incident-response/SKILL.md",
      "content": "# Incident response\n..."
    }
  ],
  "writable_prefixes": ["/scratch"],
  "max_rounds": 8
}
```

The result is one JSON object containing the final text, token usage, and files
modified under writable virtual mounts. `allowed_tools` is an exact per-run
allowlist for `read_file`, `write_file`, `edit_file`, `ls`, `glob`, `grep`,
`web_fetch`, and `web_search`; omitted or empty means no optional tools. The
runtime-owned `write_todos` helper remains available for internal planning.
Service-agent web fetches require public HTTPS destinations on port 443 and
reject private, loopback, link-local, and cluster-local targets. `web_search`
uses the keyless public DuckDuckGo provider. Host paths, traversal components,
`execute`, unlisted tools, and writes to `/skills` are rejected.

Or build without installing: `cargo build --release && ./target/release/craftsman`.

Reads and writes `~/.craftsman/config.json`. Unknown fields are preserved
verbatim on write for backward compatibility with existing installations.
TUI mouse handling is enabled by default: trackpad/mouse wheel gestures scroll the
chat, and click-drag selects and copies visible transcript text. Ctrl-C copies the current app selection again; use `--no-mouse` for terminal-native selection with Cmd-C.
Plain ↑/↓ remains dedicated to multiline prompt movement and prompt history.
PgUp/PgDn page-scrolls the chat, and Shift-↑/↓ provides fine scrolling. Use
`--no-mouse` only if you prefer terminal-native selection instead of app-level
selection.

### Shell settings

The `shell` object in `~/.craftsman/config.json` tunes the `execute` tool and the
`!cmd` escape. Every setting that changes what the model sees is off by
default and stays off until a live scoreboard run clears it (harness plan,
P2b and P3b); with all defaults the model's request and tool results are
unchanged.

| Setting | Default | What it changes |
|---|---|---|
| `shell.process_group` | `false` | Unix: each command runs in a process group of its own, and a timeout, an abandoned call or an interrupted `craftsman -m` (SIGINT, SIGTERM, SIGHUP) kills the whole group instead of only `sh`. Off until P2c, because the TUI cannot yet route a closed terminal's SIGHUP to the group. Windows always runs commands in a Job Object that is killed with the call or with craftsman. |
| `shell.timeout_secs` | `120` | The limit of one `execute` call (and of `!cmd`), 1 to 3600 seconds. On timeout the command is killed (the Job Object on Windows, the group with `shell.process_group`). The timeout text names the limit, so a value other than 120 changes what the model reads. |
| `shell.timeout_param` | `false` | Model-visible: adds an integer `timeout` argument (seconds) to the `execute` tool's schema and honours it, rounded up and clamped to 1 to 600 seconds (or to `shell.timeout_secs` when that is larger). With the flag off the argument is neither offered nor read. |
| `shell.output_format` | `"legacy"` | Model-visible. `"legacy"` keeps today's text: a timeout returns `Error: command timed out after <limit>s` and discards the output. `"head_tail"` returns the output printed before the timeout after an error line naming the limit, and cuts a result larger than 50,000 bytes to the head and the tail of stdout, the head and tail of stderr and the exit code; the full output of each cut stream is kept in a spill file whose path the result names, under `$TMPDIR/craftsman-<pid>-<random>/` (0700; files 0600). A result that fits is the same text in both formats. Spill files are not removed at exit (removing them under `--ephemeral` is planned with that flag). |
| `shell.clean_output` | `false` | Model-visible: shows the output of `execute` and `!cmd` as a terminal would. Escape sequences (colours, cursor moves, titles, hyperlinks) are removed, and a carriage return or backspace overwrites earlier text on its line, so a progress bar collapses to its last state. A CRLF line ending is kept. |
| `shell.powershell_utf8` | `false` | Windows, model-visible: puts `[Console]::OutputEncoding = [Text.Encoding]::UTF8;` at the front of every PowerShell `-Command` string. A command started without a console window otherwise prints in the OEM code page, so non-ASCII output arrives garbled. ASCII output is unchanged. |
| `shell.env_scrub` | `true` | Commands started by `execute` and `!cmd` do not inherit variables whose names look like secrets (`*_KEY`, `*_TOKEN`, `*_SECRET`, `*PASSWORD*`, `AWS_*`, `DATABASE_URL`, compared without regard to case) or the variables Craftsman's own credentials come from (`CRAFTSMAN_PLATFORM_TOKEN` and every `${NAME}` placeholder of `api_key`, `platform.api_token` and the MCP servers' `headers` and `env`). A pass list is always kept (`PATH`, `HOME`, `USER`, `SHELL`, `TERM`, `LANG`, `TMPDIR`, `CARGO_HOME`, `RUSTUP_HOME`, `GOPATH`, `JAVA_HOME`, `SSH_AUTH_SOCK`, the proxy variables, the Windows system variables and similar; `ENV_PASS_LIST` in `shell.rs`). The command also gets `NO_COLOR=1`, `PAGER=cat` and `GIT_TERMINAL_PROMPT=0`. `false` restores the fully inherited environment without hints. |
| `shell.env_pass` | `[]` | Names let through the scrub, for example `["AWS_PROFILE", "GITHUB_TOKEN"]`. Only `~/.craftsman/config.json` (user level) is read, so a repository cannot re-enable its own access to your secrets. |

The scrub covers what the agent runs through the shell (`execute`, `!cmd`). Stdio MCP servers and language servers are started by Craftsman from your own configuration and keep the inherited environment: an MCP server usually needs the credentials its `env` names, and a language server needs the toolchain environment. Their containment is a later phase (P7a); note that a language server such as rust-analyzer runs the repository's build scripts.

### Local voice dictation

The default build includes local-only continuous voice input on macOS and Linux.
Press `Ctrl+G`, press Space in an empty composer, or submit `/voice` to start.
Ordinary pauses finalize the current speech segment into editable composer text,
but the microphone stays open for the next sentence. Press `Ctrl+G` to finalize
and close the session, or Enter to finalize, close, and submit. `Esc` discards the
current tentative segment; pressing Esc again while no segment is active closes
listening. Voice input is hidden over SSH and is not compiled on Windows.

The first activation downloads a SHA-256-verified Whisper model into
`~/.craftsman/models/`; interrupted downloads resume and later use is offline. The
default `large-v3-turbo-q5_0` model is about 574 MB. Smaller supported presets
are `small-q5_1` and `base-q5_1`:

```json
{
  "voice": {
    "enabled": true,
    "engine": "whisper",
    "model": "large-v3-turbo-q5_0",
    "language": "auto",
    "auto_submit": false,
    "min_words_to_submit": 3,
    "partial_transcripts": true,
    "vad_threshold": 0.35,
    "segment_silence_ms": 800,
    "idle_timeout_ms": 30000,
    "max_session_minutes": 10
  }
}
```

`vad_threshold` controls neural speech detection and is clamped to `0.05..=0.95`;
the default also uses a conservative sustained-energy fallback for quieter
microphone paths. `segment_silence_ms` controls sentence segmentation without closing the mic;
`vad_silence_ms` remains accepted as a compatibility alias. A completely idle
session closes after `idle_timeout_ms`, and `auto_submit` is evaluated only when
the continuous session ends—not after every sentence.

Use `/voice status`, `/voice on|off`, and `/voice model` in the TUI. The model
command opens a selectable picker; `/voice model NAME` remains available for
direct selection. On macOS, install CMake and grant microphone permission to the
terminal application under **System Settings → Privacy & Security → Microphone**.
Linux source builds need CMake, `pkg-config`, Clang, and ALSA development headers
(for Debian/Ubuntu:
`sudo apt-get install cmake pkg-config libasound2-dev clang`). Constrained or
unsupported source builds can omit all native voice dependencies with:

```bash
cargo build --release --no-default-features -p craftsman-rs
```

Delegated-agent parallelism defaults to five and can be changed in the same JSON:

```json
{
  "max_parallel_agents": 5
}
```

Values are clamped to `1..=32`. The TUI shows one live progress row per running
delegated agent above the composer and removes the row as soon as the task
finishes, fails, is suspended, or is cancelled.

Delegation runs the built-in `general-purpose` agent (configurable per
harness profile) and any agents declared by [gadgets](#gadgets) (or legacy
[plugins](#plugins) during their deprecation release).
Platform-added delegated agents were removed with the login flow (see
`docs/platform-token-refactor.md`).

Workspace file checkpointing runs once around each completed agent turn. Use
`/checkpoints` to list snapshots and `/rewind [checkpoint-id]` to restore files.
Rewind compares every current file with the checkpoint's recorded post-turn
state and skips anything changed afterward; `/rewind ID --force` is the explicit
override. It never runs `git reset`, changes refs, or touches the real Git index.
Snapshots include tracked and non-ignored untracked regular files, skip symlinks
and the `.git`, `target`, and `node_modules` directories during non-Git fallback
scans, and report files that exceed the bounded snapshot
budget as unrestorable instead of claiming they are protected.

Language servers are started lazily and are never installed automatically. The
built-in mappings are `rust-analyzer` (`.rs`), `typescript-language-server
--stdio` (`.ts/.tsx/.js/.jsx`), `pyright-langserver --stdio` (`.py`), and `gopls`
(`.go`). Override or disable them in the shared config:

```json
{
  "lsp_servers": {
    "rust": {
      "command": "/custom/bin/rust-analyzer",
      "args": [],
      "extensions": ["rs"],
      "language_id": "rust",
      "enabled": true
    },
    "typescript": {
      "command": "typescript-language-server",
      "args": ["--stdio"],
      "extensions": ["ts", "tsx", "js", "jsx"],
      "enabled": true
    }
  }
}
```

`/lsp` shows availability and live state; `/lsp restart NAME` stops a server so
it starts cleanly on the next request. Agent tools provide diagnostics,
definition, references, hover, document symbols, and workspace symbol search.
Successful `write_file`/`edit_file` calls also request bounded post-edit
diagnostics when a matching server is available.

External clients use `craftsman acp-serve` over newline-delimited JSON-RPC on
stdin/stdout, or `craftsman acp-bridge` to carry the same ACP messages over
WebSocket. The bridge retains `/chat`, `/healthz` and authenticated `/stats` with
its existing proxy-secret and identity-header behavior.

The owner retired `serve` and `desktop-serve` on November 11. Clients using their
custom v2/v4 messages must migrate to ACP; changing the port alone is insufficient.
This removal adds no ACP methods or features. Local setup, editing, task controls
and microphone voice input remain available through the terminal; server-uploaded
PCM transcription and desktop-v4 operations are no longer exposed.

## Features

- Chat with streaming over an OpenAI-compatible route (`base_url` +
  `api_key`); the platform's `llm` app is one such route
- Mid-turn input: the composer remains editable while the agent works. Enter
  steers the active turn at the next model-round boundary; Tab queues a FIFO
  follow-up that starts automatically when the current turn completes. The
  composer soft-wraps by terminal display width; Ctrl+J inserts a newline.
- Ctrl-C requests turn cancellation from approval, question and
  other dialogs. Pending approval/question waiters are refused after the real
  agent receives the stop; late prompts are refused while the turn is stopping.
  Models, compaction and foreground helpers share the stop; native commands
  retain their kill/reap/drain owner. Inline shell commands remain responsive.
  Composer/editor drafts survive. Signals and quit close the session and restore
  the terminal. See [terminal lifecycle](docs/terminal-lifecycle.md) for bounds,
  background behavior and remaining containment/recovery limits.
- Terminal sessions save user intent and each completed tool result. A failed
  checkpoint stops further agent work; reopening warns about unfinished calls
  and retains their unknown completion without automatic replay. See
  [terminal recovery](docs/terminal-recovery.md) for storage and effect limits.
  Manual `/compact` also saves before summary sampling and before confirming
  success; reopening never repeats the summary or tools. See
  [terminal compaction](docs/terminal-compaction.md).
- `/model` selects provider or configured catalog models while keeping the chat
  and checkpoint sequence. `/effort` or `/model --reasoning=high` selects the
  current model's reasoning effort. See [terminal models](docs/terminal-models.md)
  for config, compatibility and cancellation behavior.
- Tool-calling agent loop on the `craftsman-harness` middleware framework, with
  the host tools `execute`, `read_file`, `write_file`, `edit_file`, `ls`,
  `glob` and `grep` (`tools.rs`, adapted to the framework's `Backend` trait by
  `RealFs` in `backend_fs.rs`)
- `write_todos` planning (`TodoListMiddleware`) and malformed tool-call repair
  (`PatchToolCallsMiddleware`), both in `craftsman-harness::builtin`
- Reviewable HITL approval dialogs for shell, file, and network actions,
  including command context and file previews (`hitl.rs`,
  `hitl_middleware.rs`). Permission modes are `plan`,
  `accept-edits`, `manual`, `bypass-permissions`, and `auto`; plan is
  fail-closed, while auto only pre-approves clearly read-only shell commands.
  Shift-Tab cycles them and `--auto-approve` remains a bypass-permissions compatibility alias.
  `CRAFTSMAN_CLI_SHELL_ALLOW_LIST` still supports hardened first-token approval.
- Subagents / `task` tool + background execution (`SubAgentMiddleware`): the
  built-in `general-purpose` agent plus any
  agents declared by [gadgets](#gadgets) or legacy [plugins](#plugins), all
  compiled with their allowed
  host/web/MCP tools behind the same HITL boundary. `/tasks` lists foreground/background
  status + progress, and background completions/messages/needs-approval are
  broadcast to both the UI and the parent agent context at model-round boundaries
- MCP tools from `mcp_servers` config entries exposed to the model and
  dispatched without an approval prompt (`mcp.rs`, a minimal client for the
  modern stateless protocol): streamable-HTTP
  servers (including the platform's `/mcp`, with `${ENV_VAR}` header
  expansion) and stdio child-process servers
  (`{"transport": "stdio", "command": …, "args": […], "env": {…}}` with
  `${ENV_VAR}` env expansion). See `examples/config/mcp-servers.json`.
- Guided `/setup`, `/mode`, `/status`, `/doctor`, `/diff`,
  `/checkpoints`, `/rewind`, `/lsp`, `/editor`, `/queue`, `/tasks`, `/session`,
  `/skills`, `/compact`, `/tokens`,
  `/clear`, and the core connection/model/harness commands. `!cmd` remains the
  explicit shell escape.
- A Ratatui + Crossterm TUI. Tool calls and background-task completions
  render as collapsible transcript
  blocks. Click a block title to expand/collapse it, or press Ctrl-O to toggle
  the latest block; failures open automatically with full bounded details.
- Conflict-safe per-turn workspace snapshots stored beside conversation state;
  `/rewind` restores only files that still match the agent's post-turn version
  unless an explicit `--force` is supplied.
- Shared lazy stdio LSP clients and read-only semantic tools:
  `lsp_diagnostics`, `lsp_definition`, `lsp_references`, `lsp_hover`,
  `lsp_document_symbols`, and `lsp_workspace_symbols`, including UTF-16 position
  conversion and automatic bounded diagnostics after file edits.
- Headless token configuration: LLM `base_url` + `api_key` (with
  `${ENV_VAR}` expansion), platform REST via `platform.api_token`
  (or `$CRAFTSMAN_PLATFORM_TOKEN`), with `craftsman` as the primary command and
  `craftsman-rs` retained as a compatibility launcher
- Skills discovery (the agent directory, `~/.craftsman/skills`,
  `~/.agents/skills`, `./.craftsman/skills` and `./.agents/skills`) listed in the
  system prompt; AGENTS.md (global + project) appended to the system prompt
- [Gadgets](#gadgets): live, capability-scoped WASM functions plus declarative
  slash commands, subagents and prompt instructions. SDK/authoring commands are
  embedded; model visibility is opt-in. Legacy [plugins](#plugins) remain for
  one deprecation release and migrate without deleting originals.
- Legacy one-shot `-m` with fallible main-only stdout and non-TTY deny-without-read;
  private headless permission/confinement foundations are documented separately
  in [docs/headless-mode.md](docs/headless-mode.md), not advertised as a command
- Config round-trip that preserves unknown keys and writes
  the file with 0600 permissions
- Long-term memory: the virtual `/memories/` directory routes to a durable
  store (`store_files` table in the same libsql DB) via a CompositeBackend;
  `/memories` lists it. Compaction uses a token estimate at 85% of the
  configured context window (`model_profiles[model].context_window`) and
  keeps up to 20k tokens of recent user
  messages; models without a configured context window retain the
  120-message/20-message
  fallback. Manual `/compact` remains independent of the automatic trigger.
  System-reminder nudges, deterministic result budgets, and
  built-in `web_fetch`/`web_search` (network egress HITL-gated under
  `auto_approve_web`; search provider configurable, honest unconfigured
  message); delegated subagents receive the same non-interactive host, web,
  and MCP capabilities as the main agent.
- Sessions: every conversation checkpoints to `~/.craftsman/threads.db`
  (libsql — one local file, also holding `/memories`; there is no remote
  sync).
  `/session` lists, searches, resumes, renames, deletes, and exports threads;
  resuming rebuilds the visible role-aware transcript. Persisted pending
  approvals remain durable. Project contexts are stored with the thread in
  `thread_project_contexts`, primary first. A JSONL transcript journal is written to
  `~/.craftsman/sessions-rs/` for
  observability. Note: the checkpoint restores the *conversation*; files
  the agent wrote live on disk and are not part of thread state. Existing
  `threads-rs.db` data is moved automatically; a `threads.db` in the older,
  incompatible checkpoint schema is archived as
  `threads-python-legacy-<timestamp>.db` before migration.

**Not currently exposed or completed:**
- `craftsman exec`: accepted contract and private foundations only; bounded instruction
  input/run-loop adapters, guest-host enforcement, shared usage, redaction,
  schema-backed output and trace production remain open. Full wall deadlines,
  signals, OS sandbox and verification belong to later planned phases.
- `@file` attachment/inlining and image input
- Langfuse tracing and lifecycle hooks
- Admin/addon-development commands
- Full syntax highlighting for every Markdown language and terminal-native
  clickable/collapsible widgets

## Harness and provider profiles

The two profile types solve different problems:

- **Harness profile** changes agent behavior after a model is selected: system
  prompt tuning, tool descriptions/visibility, middleware, and the default
  general-purpose subagent.
- **Provider profile** supplies low-level defaults for a direct model transport,
  such as NVIDIA attribution headers. It does **not** choose the endpoint,
  API key, model, or platform route.

Run `/harness` to choose one of the built-in examples:

| Selection | Behavior |
|-----------|----------|
| `auto` | Recommended. Detect OpenAI/Claude/Nemotron from the model; unknown models use General. |
| `general` | Original Craftsman behavior with no model-specific tuning. |
| `openai` | Autonomous execution, parallel tool use, and TODO reconciliation. |
| `claude` | Grounded investigation, reflection, and parallel tool use. |
| `nemotron` | Tool grounding, bounded retries, and complete final answers. |

Useful commands:

```text
/harness                 # interactive selector
/harness status          # selected preset, route, effective layers, provider profile
/harness auto
/harness general
/harness openai
/harness claude
/harness nemotron
```

The selection is stored in the shared config as, for example:

```json
{
  "model": "claude-sonnet-4-6",
  "harness_profile": "claude"
}
```

### Route behavior

Named `providers` connections support OpenAI Responses, Claude Messages, Gemini
GenerateContent and OpenAI-compatible Chat Completions. Models live under each
provider's `models` map, keyed by the actual API ID; `/model provider/model-id`
selects the connection and its saved settings while retaining the conversation.
See [provider models](docs/provider-models.md) and the [complete placeholder config](examples/config/multi-provider.json).
`/setup` and `craftsman setup` edit one configured connection through the
[provider setup flow](docs/provider-setup.md), retaining its catalog, other
connections and the conversation. Flat configs keep their platform/direct setup.

Without named providers, the legacy chat route is the endpoint named by `base_url`
and `api_key` (`https://api.openai.com/v1` when `base_url` is empty). The harness
profile applies on top of whichever model is selected. The former platform
connection example has been removed because that service is no longer available.

For the legacy route, `provider_profile` supports:

- `"auto"` or omitted: infer from a `provider:model` model spec or the standard
  OpenAI base URL.
- `"none"`: disable provider defaults, recommended when LiteLLM or another
  proxy owns the real provider behind it.
- `"nvidia"`: apply the built-in NVIDIA attribution header profile.

`model_profiles[model]` remains the caller override layer and wins over provider
profile defaults. Common request options such as `temperature` and `max_tokens`
are sent to the selected route.

Ready-to-copy examples are in [`examples/config/`](examples/config/):

- `multi-provider.json` — named connections with actual model IDs and per-model settings
- `direct-openai.json` — direct OpenAI-compatible endpoint
- `direct-nvidia.json` — direct NVIDIA/Nemotron endpoint
- `litellm-proxy.json` — proxy route with provider defaults disabled

The example files contain placeholders and no real credentials. Set the endpoint
and API key supplied by your model provider; `${ENV_VAR}` credential references
are also supported. No platform account or platform connection is needed.

## Gadgets

Gadgets live in `~/.craftsman/gadgets/<id>/` (honoring `CRAFTSMAN_CONFIG_DIR`) or
`./.craftsman/gadgets/<id>/`. `gadget.toml` can declare slash commands, agents and
prompt instructions on its own. Add a typed Rust WASM component for callable
functions. The built-in SDK and `build-gadget` skill support authoring without
a private registry:

```sh
craftsman gadgets doctor
craftsman gadgets new word-count
craftsman gadgets build word-count
craftsman gadgets migrate --dry-run
craftsman gadgets migrate              # user plugins; originals preserved
craftsman gadgets migrate --project    # explicit project scope
```

Enable `{"gadgets":{"enabled":true}}` in user config to opt a terminal session
into gadgets. The harness discovers newly installed/rebuilt callable functions
before each main/delegated model round and sends change-only catalog metadata:
**no `gadget_list` polling, restart, or tool-definition churn**. First-load
capability approval still applies, separately from native Cargo build approval;
unapproved components never execute, including `describe`.

Commands refresh on submission; declarative agents/prompts load at session
build as legacy plugins did. Both command expansion and delegated tools retain
the ordinary permission boundary. See [docs/gadgets.md](docs/gadgets.md) for the
manifest, host capability rules, authoring, bounds and migration lifecycle, and
`examples/gadgets/` for compute, workspace-read and data-only examples. All
builds and scaffolds use the exact Rust toolchain pin; it is **1.99.1** as of
October 20, 2026. The model-facing default stays off pending live evaluations.

## Plugins

**Deprecated:** migrate with `craftsman gadgets migrate` (add `--project` for project
plugins), then opt in to `gadgets.enabled`. Originals and the legacy loader are
retained for one released deprecation cycle; removal is a later breaking
change. The JSON contract below remains supported during that interval.

A plugin is a data-only JSON manifest — no code runs. It can add slash
commands, subagents, and system-prompt instructions. Put one plugin per
`*.json` file in `~/.craftsman/plugins/` (all projects) or `./.craftsman/plugins/`
(this project; safe to commit). See `examples/plugins/review-flow.json`.

```json
{
  "version": 1,
  "name": "review-flow",
  "prompt": "Report concrete defects before style nits.",
  "commands": [
    { "name": "review", "description": "Review the current diff",
      "prompt": "Review the current git diff for correctness bugs. {$ARGS}" }
  ],
  "agents": [
    { "name": "reviewer", "description": "Read-only code reviewer",
      "system_prompt": "You are a meticulous code reviewer. Do not modify files.",
      "tools": ["read_file", "grep", "glob", "ls"] }
  ]
}
```

- **`commands`** — `/review focus on error handling` sends the command's
  `prompt` to the model, with the text after the command substituted for
  `{$ARGS}` (or appended after a blank line when the placeholder is absent).
  The prompt must contain text besides `{$ARGS}`. The expansion is model input
  only: it is never run as an inline `!shell` command or a built-in `/command`,
  and whatever the model then does still passes the approval gate for the
  active permission mode. Names are lowercase letters, digits, and dashes;
  built-in commands always win, so a plugin command named like one is ignored.
  Plugin commands appear in the `/` menu and in `/help`; the menu re-reads the
  plugins each time you start a new `/…` line, so edits show up without a
  restart. Typed while a turn is running, a command is queued like any other
  slash command. Commands expand in the interactive TUI; `craftsman -m "/review"`
  sends the text as typed.
- **`agents`** — extra subagents for the `task` tool beside `general-purpose`
  (which a plugin cannot replace). `tools` names are looked up in the session's
  tool registry: unknown names are skipped, `task`/`send_message` are dropped,
  and omitting `tools` gives a reasoning-only agent. Gated tools (shell, file
  writes, network) stay gated — the same approval boundary as the main agent.
- **`prompt`** — instructions appended to the system prompt under
  "Plugin instructions".

Agents and prompts are read when a session is built (start-up, or a rebuild
such as `/model`). `CRAFTSMAN_CONFIG_DIR` moves the global directory. Directories
are read global first, then project, and files in name order; when plugins
share a `name`, a command name, or an agent name, the later one wins, so
project plugins override global ones.

A manifest needs `"version": 1` and a `name`. It is skipped as a whole, and
silently, when it is not UTF-8 JSON (a leading byte-order mark is fine), or
contains an invalid name, a command whose prompt is empty or only `{$ARGS}`,
an agent without a description or system prompt, or an agent named
`general-purpose`. Unknown keys are ignored. Plugin directories can come from
an untrusted checkout, so discovery is bounded: 64 manifests per directory,
256 KiB each and 512 KiB in total. The sandboxed `service-agent` runtime never
loads plugins.

Plugin text steers the model the way `AGENTS.md` does, so review
`./.craftsman/plugins/` in a cloned repository before trusting it; a project
plugin with the same `name` as one of yours replaces it. Whatever the TUI
draws (model output, tool output, plugin text) is filtered for control
characters, which appear as visible symbols such as `␛` instead of reaching
the terminal.

## Development

Clone the canonical repository (GitHub access is required):

```bash
git clone https://github.com/kaobei-alabo/craftsman-harness.git
cd craftsman-harness
```

From the repository root:

```bash
cargo build --release --locked -p craftsman-rs
cargo test --locked
cargo fmt --all -- --check
scripts/check-lineage.sh     # lineage text not covered by scripts/lineage-allow.txt
# CI's native-dependency-free and opt-in gadget variants:
cargo test --locked --no-default-features --workspace
cargo test --locked --no-default-features --features craftsman-rs/gadgets --workspace
python3 -I docs/schemas/check_exec.py --self-test
```

Update docs in each implementation PR, even before a feature is finished. Use
[docs/runtime-status.md](docs/runtime-status.md) for current status/architecture;
[docs/headless-mode.md](docs/headless-mode.md) for accepted behavior and private
implementation progress; and [docs/craftsman-harness-plan.md](docs/craftsman-harness-plan.md)
for historical baseline evidence and remaining target phases. Source tests do not
replace live-quality or published-deprecation gates.

`scripts/check-lineage.sh` searches the tracked and non-ignored untracked files
for text that describes this project as derived from another (the patterns
are in the script's header) and prints every hit that
`scripts/lineage-allow.txt` does not cover; it exits 1 when it printed
anything. Every allow-list entry names a path, an optional substring, and a
reason.

## Layout

```
crates/craftsman-harness/src/
  agent.rs        Agent loop, checkpoints, steering, and builder
  backend.rs      Backend trait + StateFs contracts
  middleware.rs   AgentMiddleware hooks and onion composition
  model.rs        model requests/responses, usage, and TokenCounter
  state.rs        AgentState, messages, todos, and file reducer
  subagent.rs     delegated-agent registry, inboxes, and notifications
  profiles/       harness and provider profile resolution
  builtin/        todos, tool repair, HITL, subagents, memory, compaction,
                  reminders, and result budgets
  testing.rs      ScriptedModel test double
crates/craftsman-harness/tests/dep_guard.rs
                  keeps HTTP, database, UI, and other app dependencies out

crates/craftsman-rs/src/
  main.rs          clap CLI and supported top-level subcommands (no exec route yet)
  exec/            private headless implementation (no CLI route):
    permissions.rs immutable explicit selectors and invocation policy
    confined_fs.rs handle-relative Backend + typed byte IO, protected paths and bounded workers
    construction/  explicit config/discovery, captured credentials, source handles and sessions
    input/         bounded source argv + original input/native stdin and guarded-file composition; no CLI
    runtime/       shared-builder adapter, opaque guest scope and all-role typed fail latch
    redaction/     private captured seeds, frozen route, literal streaming/structured redaction
    shell/         private builder-only shell lease, strict pipe capture/formatting before model results
      observations/progress.rs cfg(test)-only original poll/select/pre-stop history; no extra poll/waker
    confined_fs/view/ scoped filesystem text/name producers over retained original IO; pre-page coverage
    web/           private scoped network, original-byte capture, bounded HTML and strict search views
    framework/     private concrete todo/task/message views and todo context; original state/delivery retained
    provider/      private actual-builder request views, schema/identity guard, exact checked client-bound bytes
    redaction/publication.rs shared private text/notice retention after literal redaction + verification
    messages/      private actual-state snapshots and whole-message suffix selection; no raw-state producer
    terminal/      consumed-Run authority, checked UTF-8/ASCII/human views and private bounded file Record
    output/        pre-run external file destination, guarded create-only native publication; no CLI/stdio
  platform_os/stdin_pipe/ selected pipe ownership/owned-mode or bounded native readiness; no reader thread
  oneshot_io.rs    fallible legacy -m sink and all-role output-closed guards
  execution_profile.rs local/workspace/service-agent capability profiles
  tui/             Ratatui application, split by responsibility:
    app.rs         App state, transcript model, and the main loop (run)
    input.rs       keys, mouse, paste, voice state, and composer layout
    render.rs      frame drawing: transcript, splash, subagents, footer
    modals.rs      modal state, keys, rendering, and pickers (setup, model, ...)
    commands.rs    slash commands, help, and submission routing
    session_bridge.rs agent events, approvals, hydration, and notifications
    terminal.rs    terminal guard, SafeBackend, and clipboard
  acp_serve.rs     standard ACP sessions, permissions, replay and completion
  bridge_support.rs shared ACP proxy authentication and host disk facts
  agent.rs         provider bridge and shared frontend/private-headless construction
  token_counter.rs local tokenizer used by token-budget compaction
  checkpoints.rs   durable conversation/checkpoint store
  workspace_checkpoint.rs per-turn file snapshots + conflict-safe rewind
  workspace_diff.rs bounded staged/unstaged/untracked Git diff rendering
  lsp.rs           lazy stdio LSP clients + explicit disabled headless manager
  shell.rs / shell_output.rs shell settings, scrub and bounded capture (headless: no spills)
  hitl.rs / hitl_middleware.rs legacy permission policy and approval boundary
  backend_fs.rs    RealFs backend over the host filesystem
  llm.rs / route.rs OpenAI-compatible streaming and route resolution
  mcp.rs           MCP client (modern stateless protocol; HTTP + stdio transports)
  platform.rs      OpenAI-compatible model listing
  platform_rest.rs token-authed platform REST (skills) + URL normalization
  prompt.rs / skills.rs AGENTS.md, memory, and SKILL.md prompt assembly
  extensions.rs    shared declarative gadget/legacy-plugin contracts and precedence
  gadgets/         manifest/catalog, capability host, grants, authoring and migration
    confined.rs    private byte-safe filesystem facet; not a source/grant/runtime constructor
    sources.rs     retained-source/approval snapshots + original filesystem/policy; no grant writes
    headless.rs    private source-approved host; redacted guest views, no tools/catalog/public producer
    headless/      strict HTTP facet and synthetic guest-host regressions
      output.rs    frozen-seed JSON + shared gated byte capture; direct WASIp2, before truncation
  plugins.rs       deprecated data-only loader (kept for the published deprecation cycle)
  config.rs        shared ~/.craftsman/config.json with unknown-field preservation
crates/craftsman-rs/tests/loop_test.rs / oneshot_contract.rs
                  synthetic-provider loop, delegation and one-shot regressions
crates/craftsman-rs/tests/gadget_authoring_test.rs
                  real CLI cold-build fixtures: explicit 240s build / 300s command budget
crates/craftsman-rs/src/acp_bridge.rs (tests)
                  native ready/echo/exit fixture; no OS-shell startup dependency
crates/craftsman-rs/src/exec/confined_fs/tests.rs / gadgets/confined/tests.rs
                  adversarial filesystem/permission, raw-byte and private guest-facet tests
crates/craftsman-rs/src/exec/construction/tests.rs / exec/runtime/tests.rs
                  scoped preparation, frozen-auth roles and shared-builder synthetic-provider tests
crates/craftsman-rs/src/exec/redaction/tests.rs
                  captured seeds, overlap/UTF-8 streams, match facts and abort-without-release
crates/craftsman-rs/src/gadgets/sources/tests.rs / gadgets/headless/tests.rs
                  source/hash/trust/provenance, original authority, strict guest imports/budgets
crates/craftsman-rs/src/gadgets/headless/output_tests.rs
                  all-boundary/cut shared captures, UTF-8, direct WASIp2 and JSON collision/bounds
crates/craftsman-rs/tests/fixtures/headless-gadget-host/
                  app-only source guest; bounded cold setup then strict post-instantiation stop checks
                  original executed inputs, redacted JSON/stdout/stderr/logs; unchanged WIT/SDK/old fixtures
crates/craftsman-rs/src/agent_shell_tests.rs / exec/shell/capture/tests.rs
                  actual builder lease, pipe/cut/UTF-8/timeout/stop, raw command/status/byte tests
crates/craftsman-rs/src/agent_fs_tests.rs / exec/redaction/masked/tests.rs
                  original multiline/page/line/query authority, name collisions, typed stop, fixed gate views
crates/craftsman-rs/src/exec/confined_fs/view/output/tests.rs
                  bounded UTF-8 framing, pre-cut literal verification and exact safe publication
crates/craftsman-rs/src/agent_web_tests.rs / exec/web/{capture,html,search}/tests.rs
                  actual request/query/auth, read/parser/cut bounds, MIME fixtures, typed stop and HTTP views
crates/craftsman-rs/src/agent_framework_tests.rs / exec/framework/views.rs
                  concrete source/gate binding, whole todo records, original tasks/inboxes, metadata and context views
crates/craftsman-rs/src/agent_provider_tests.rs / exec/provider/request.rs
                  original control/auth vs checked requests, escaping/schema/identity/bounds, client binding and retry stop
crates/craftsman-rs/src/exec/input/scopes.rs / scopes/tests.rs
                  bounded relative-scope/output-file preflight, original argv data and same guarded owners
crates/craftsman-rs/src/exec/input/tests.rs
                         explicit input, retained file/byte admission before sessions, actual-builder model data
crates/craftsman-rs/tests/headless_contract.rs / headless_contract/admission.rs
                  accepted structural vectors + proposed admission arithmetic/ledger checks, not real exec output
crates/craftsman-gadget/ / craftsman-gadget-macros/
                  independent gadget authoring SDK/WIT and macros
```

## Local WASM gadgets

The opt-in gadget runtime supports live installation and rebuilds, capability
approval, and the fixed `gadget_call`/`gadget_list` tools. The harness refreshes
the catalog deterministically before each model round, so the agent does not
need to poll `gadget_list`. See [docs/gadgets.md](docs/gadgets.md) for configuration,
authoring, permission boundaries and the current G1/G2/G3 implementation scope.


### Incomplete Windows Job/PID fixture maintenance

The independently checked post-merge main run41668514031 still failed the original
Windows2s partial-output and3s Job/PID cases. A narrow test-only PID fixture now
uses actual direct .NET child creation/PID flushing instead of PowerShell process/
output cmdlets; the ordinary2s fixture, all original assertions/deadlines and
production behavior stay unchanged. This is not a proven startup/root fix and
requires native Windows/all-four exact-head verification. See
[Windows fixture status](docs/windows-shell-fixtures.md).


## Immutable environment test fixtures

Config, MCP, route and HTTP/WS auth tests preserve their original assertions
using immutable child environments and per-router state. See
[environment-test isolation](docs/test-environment-isolation.md) for the maintained contract.
