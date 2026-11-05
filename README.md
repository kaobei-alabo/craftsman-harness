# craftsman-harness

A Rust terminal coding agent that edits files, runs tools, delegates work and
can build reusable **WASM gadgets** for itself. The workspace contains the
**`craftsman-harness`** agent framework, **`craftsman-rs`** application and the
gadget SDK/macros. Model routes are OpenAI-compatible; platform skills and MCP
are optional integrations. See `docs/platform-token-refactor.md` for token
configuration.

The canonical repository is **[yumingchangsabodota/craftsman-harness](https://github.com/yumingchangsabodota/craftsman-harness)**.
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
A subsequent test-only scoped-shell observation adapter helps investigate the
unresolved local macOS full-suite discrepancy (#138). It records actual launch/
read timing and fixed metadata only on a failed fixture; old assertions/deadlines
and all runnable agent behavior stay unchanged. It is not production tracing or
a root-cause fix. The next macOS-only failure snapshot records bounded native
task/thread-state facts and exact executable/CWD classifications against the
retained invocation workspace. A bounded test-only pre-kill snapshot now preserves
original stop/pipe/native facts without later PID lookup; none are raw paths,
CPU-spin/exec/containment/progress/measured-usage proof. Old assertions and
production behavior remain unchanged. See
[docs/shell-test-observations.md](docs/shell-test-observations.md).
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
craftsman desktop-serve --host 127.0.0.1 --port 8081
craftsman service-agent --request request.json  # restricted shared-runtime turn
craftsman acp-serve              # Agent Client Protocol server over stdio
craftsman acp-bridge --port 8080 -- <harness>  # WebSocket to stdio ACP bridge
craftsman-rs --version           # legacy compatibility command
```

Platform access is headless and token-configured (no login command): the
platform's LLM app is a stock OpenAI-compatible endpoint (`base_url` +
`api_key` in `~/.craftsman/config.json`, with `${ENV_VAR}` expansion), the
platform's `/mcp` is an ordinary `mcp_servers` entry, and
`platform.api_token` (or `$CRAFTSMAN_PLATFORM_TOKEN`) backs skills REST and the
workspace inbox. See `docs/platform-token-refactor.md`.

### Execution profiles

Craftsman CLI currently exposes three security/capability profiles. The private
headless construction profile is test-only, not another runnable CLI surface.
Filesystem authority does not imply OS containment:

| Profile | Commands | Filesystem | Shell | Session storage |
|---|---|---|---|---|
| `local` | `craftsman`, `craftsman -m`, `desktop-serve`, `acp-serve` | Real user filesystem | HITL-controlled native shell; no Craftsman OS sandbox | Local `~/.craftsman` |
| `workspace` | `craftsman serve` | Deployment-selected workspace/PVC | Native shell; isolation depends on the external deployment policy | Workspace HOME/PVC |
| `service-agent` | `craftsman service-agent` | Scoped virtual files plus selected public-web tools | Unavailable | Supplied/owned by the external runtime |

`acp-bridge` is a transport front door, not a separate filesystem profile. Desktop
protocol v4 is frozen; ACP uses standard frames and declared Craftsman extensions.
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

`desktop-serve` is the local desktop bridge. Unlike the pod-oriented `serve`
mode, it preserves the normal HITL boundary and exposes token, activity,
permission, `ask_user`, A2UI, state snapshots, structured tool activity, setup, conversations,
workspace checkpoints, tasks, and diagnostics over protocol v4. A launcher can set
`CRAFTSMAN_DESKTOP_TOKEN` and connect with `?token=...` for loopback authentication.

Conversations have global identities independent of their execution directory.
Each persisted thread may carry an ordered `project_contexts` list with zero,
one, or many canonical project directories. `set_thread_contexts` updates that
metadata without replacing the thread or transcript; the first path is the
primary project for relative tools, LSP, diffs, and file checkpoints, while
additional projects are available by absolute path and are included in the
agent's system context. `switch_thread` restores both history and project context
without restarting the process, port, token, or WebSocket. Heavy provider/LSP
state remains lazy until the next real turn.

The older additive `workspace_switch` operation remains accepted for compatible
clients and is translated to a single project context, but current desktop
clients do not use project paths as conversation keys.

`desktop-serve` binds its loopback listener before refreshing credentials or
building any configured agent session. Headless initialization performs no
provider call and creates no project runtime. Its first snapshot can therefore
report `initializing: true` while the local bridge is already connected, then
become ready without touching `config.json`. Provider discovery and the
project-aware agent runtime is created only when a real chat request needs
them. Automatically selected models remain in memory; only explicit user
configuration writes model/provider settings. A provider error is reported only
when the real turn fails.

Desktop conversation catalogs are paginated at the SQL layer. `list_threads`
and `list_all_threads` accept `offset`/`limit` and return `next_offset`/
`has_more`; desktop fetches small pages as the sidebar needs them. Every row is
an ordinary conversation and includes its ordered project-context metadata.
Catalog and history reads remain local and available without a provider session.
Changing project context preserves message checkpoints and discards only
filesystem rewind snapshots whose paths belonged to the previous context.

## Features

- Chat with streaming over an OpenAI-compatible route (`base_url` +
  `api_key`); the platform's `llm` app is one such route
- Mid-turn input: the composer remains editable while the agent works. Enter
  steers the active turn at the next model-round boundary; Tab queues a FIFO
  follow-up that starts automatically when the current turn completes. The
  composer soft-wraps by terminal display width; Ctrl+J inserts a newline.
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

There is one chat route: the OpenAI-compatible endpoint named by `base_url`
and `api_key` (`https://api.openai.com/v1` when `base_url` is empty). The
platform's LLM app is used the same way, with `base_url` pointing at
`{platform}/apps/llm/api/v1` (see `examples/config/platform.json`). The harness
profile applies on top of whichever model is selected.

`provider_profile` supports:

- `"auto"` or omitted: infer from a `provider:model` model spec or the standard
  OpenAI base URL.
- `"none"`: disable provider defaults, recommended when LiteLLM or another
  proxy owns the real provider behind it.
- `"nvidia"`: apply the built-in NVIDIA attribution header profile.

`model_profiles[model]` remains the caller override layer and wins over provider
profile defaults. Common request options such as `temperature` and `max_tokens`
are sent to the selected route.

Ready-to-copy examples are in [`examples/config/`](examples/config/):

- `platform.json` — the platform as a stock OpenAI-compatible endpoint plus
  its `/mcp` as a plain MCP entry (token via `${CRAFTSMAN_PLATFORM_TOKEN}`)
- `direct-openai.json` — direct OpenAI-compatible endpoint
- `direct-nvidia.json` — direct NVIDIA/Nemotron endpoint
- `litellm-proxy.json` — proxy route with provider defaults disabled

The example files contain placeholders and no real credentials. Mint a
personal API key on the platform (auth → API keys), export it as
`CRAFTSMAN_PLATFORM_TOKEN` (or paste it into `platform.api_token` /
`api_key`), and the LLM route, MCP entry, and skills REST all use it.

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
git clone https://github.com/yumingchangsabodota/craftsman-harness.git
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
  desktop_serve.rs desktop protocol-v4 bridge with HITL and conversations
  serve.rs         pod-oriented WebSocket protocol-v2 server
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

### Desktop voice protocol

`craftsman desktop-serve` exposes the same local voice controller to desktop
protocol-v4 clients through `get_voice_status`, `voice_start`, `voice_stop`,
`voice_cancel_segment`, `voice_cancel`, `set_voice_model`, and
`set_voice_enabled`. Progress, microphone level, partial/final transcript, and
error events are emitted as typed `voice_event` frames bound to the WebSocket
generation that started capture. Disconnect cancels active microphone capture.

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


### Private native scheduling diagnostics

Test-only macOS shell diagnostics now project exported flag bits and original
nice/task/thread priority/policy fields from already-returned native buffers,
with no additional queries. Old deadlines/behavior and4096-byte whole safe-report
cap stay intact; unknown bits and signed native values are not a startup/wait-
cause/usage proof. Local #138 failures remain unresolved and preserved. See
[scoped shell test observations](docs/shell-test-observations.md).


### Test-only original shell poll/selection diagnostics — November 4

The unresolved local #138 failures now have a bounded `cfg(test)` follow-up:
actual original reader-poll counts/last results and existing reader/wait selection
phases, with pre-stop history kept separate from later drain. No waker wrapper,
additional native lookup, new poll, changed assertion/deadline or production
process behavior is added. See [the diagnostic slice](docs/shell-poll-progress.md)
for validation and limits; public exec/usage/settlement/callables stay unwired.


November 4 changed-source full validation remains mixed: off FAILED1600/4/7
(including the NEW normal-output diagnostic), gadgets1747/0/9 passed. All old
inventories plus13 new cases are present; no compiler warnings/unchanged retry.
ONE original Pending poll per pipe/no wait-selection entry in the off failure
reports is parent-side phase evidence, NOT a root fix or lost-wakeup explanation.
The gadget pass never replaces the off/prior failures; #138 stays OPEN and all
public/usage/settlement/callable/P2g/live/release gates remain separate/off. See
[the diagnostic validation](docs/shell-poll-progress.md).


### Corrective test-environment isolation — November 4, 2026

The next correction removes unsafe process-global environment mutation from
parallel config/MCP/route/protocol tests. Original assertions run with exact
synthetic values injected BEFORE a marked child starts, and the concurrent serve
auth case uses a cfg(test)-only per-router source with the same actual gate.
Runtime auth/environment/shell behavior, SDK/framework/protocols and deadlines
are unchanged. See [test-environment-isolation.md](docs/test-environment-isolation.md)
for the partial correction, native validation and remaining #138 root-cause gate.


Corrective full validation (November 4 UTC): all five original environment/
protocol cases and eight new substantive cases pass both modes, warning-free.
Full off still FAILED1605/4/8 with the same four shell/readiness failures;
gadgets1752/0/10 passes. Unsafe environment mutation is removed, but it is NOT
proved #138's sole cause/fix; no old assertion/deadline/process/reader/SDK or
provider workaround. All prior failed evidence is retained and #138 OPEN;
all-four/every-step/both-mode hosted verification remains required.


Windows PR147 first-head41829354220 is preserved FAILED in BOTH modes only for
the NEW offline declaration guard's LF-only source match (native main1552/0/8
and1684/0/10 and every old/env assertion pass; interpreter post step skipped).
Correction normalizes ONLY CRLF pairs to LF in that NEW guard; original exact
cfg/declaration checks remain, with an additive LF/CRLF-versus-lone-CR/wrong-cfg/
wrong-name/tab negative case. No production/old assertion/deadline/pin/SDK/
workflow/warning suppression or unchanged CI retry. Require ALL FOUR fresh
corrected-head jobs/every step/both modes/native/inventory/warning/source/tree
proof before review-only merge; #138 and old local failures remain unresolved.


## PR147 corrected-head Windows failure and test-only phases — November 4 UTC

Corrected head3919f640/CI41835883954 attempt1 passes ALL four new CRLF/environment
source guards and ALL five original environment/protocol cases BOTH modes on
Ubuntu24.04/actual macOS15/Windows2025. It is still NOT all-four green: Windows
feature-off Test fails the ORIGINAL2s partial-output stdout assertion with empty
wording (1551/1/8); later gadgets1684/0/10 does not replace it and interpreter post is
skipped. All other jobs/steps/old+new inventories and Windows9 warning signatures/
17 occurrences remain verified. PR147 remains DRAFT/unmerged; #138 stays OPEN.
No startup/reader/wake/capture cause follows from the missing output.

The next [test-only legacy phase observer](docs/legacy-shell-observations.md) records original spawn return,
exact delegated read polls and existing reader/wait/kill/drain selections, owned
by the actual ordinary test ShellRunner. At most8 calls/WHOLE4096 ASCII-LF report
or fixed refusal; original clock/counters and pre-stop/current histories remain
separate. No extra poll/waker/native lookup/raw bytes/error/env/clock reset/global
hook/launcher/warmup/retry. Only original assertion-panic teardown prints it;
headless retains its SAME existing recorder/native bounds. Shared executor
extraction and cfgtest-only seams keep original pump/process/capture/defaults/
old commands/assertions/Windows2s+3s/30s outer/2s drain/private15s+20s+10s+6s and
env20s+5s limits intact. Eight additive cases plus a CRLF-safe source guard are
not a root fix/Windows proof. Docs with incomplete work; all FOUR fresh exact-head
jobs/EVERY step/BOTH modes/native/source/tree/inventory/warning/review proof still
gate missing-review-only merge. No unchanged broad retry/admin failure bypass,
real model/user config/SDK/global-env mutation, agents/goals/blocked-provider work
or public exec/usage/settlement/callables/P2g/live/default-on/release enablement.


One genuinely changed-source full command per mode at c1cac545 (November4 UTC)
completed warning-free with every old native name/ignored/doctest plus the eight
new main cases/new guard/CRLF guard preserved. All original environment/auth and
all18 new substantive cases relative to PR146 pass BOTH modes. Off remains
FAILED1613/4/8, exit101,998.48s whole command: the SAME four scoped-shell cases
still fail. Gadgets1760/0/10 passes, exit0,73.85s; never substitute that pass.
Temporary lack of startup output from an offline test binary later cleared; its
original source assertions pass. No root-cause/PID/startup-latency inference,
clock/assertion/launcher workaround, unbounded wait or unchanged broad retry.
Full evidence stays retained; #138 OPEN and PR147 draft pending ALL FOUR fresh
exact-head jobs/EVERY step/BOTH native modes/source/tree/inventory/warning/review
proof. Public/usage/settlement/callable/P2g/live/default-on/release remain off.


## Native current-child pipe snapshots — November 5 Asia/Taipei / November 4 UTC

PR147/main86704e4 are independently ALL FOUR/every-step/both-mode/source/tree/
inventory/old Windows9-signature17-occurrence verified; live main/p0-integration
branches were FF-only/data-preserving. The transient null Windows main-job
metadata was strictly refused, then SAME-run/job metadata converged before
verification/FF; no CI retry/faked completion. Local off1613/4/8 remains FAILED,
gadgets1760/0/10 passes, #138 OPEN and all older failed evidence sealed/read-only.

The next [read-only pipe facet](docs/native-pipe-snapshots.md) queries ONLY the existing live actual
native snapshot ticket's CURRENT child fd1/2 metadata; no read/dup/poll/waker/
launcher/environment/clock/signal/sampler/public output change. Complete exact
public ABI/write-pipe/FIFO/signed-count checks and SAME final PID/start/parent/
group validation gate all facts. Buffer counts are not original pipeline
identity/read/emission/EOF/usage or cause proof; child fd rebinding is explicit.
SAME13-query/8-thread/call/view/4096-byte whole-report bounds:5 fixed + whole-list
reserved threads; two pipe queries only if<=6 fit,7/8 preserves old threads and
refuses new facet, no refund/retry/clip. Old phase/ticket/clock/limits/assertions/
production/SDK/WIT/deps/lock/pin/frontends/frames/schemas/goldens/trace stay exact.
Additive ABI/pure/real-unread-pipe/actual-builder-zero-timeout tests and docs are
not a root fix. Require fresh all FOUR exact-head jobs/EVERY step/both modes/
native/source/tree/inventory/warning/review proof before review-only merge and
separate main verification before safe live FF. No warmed/unchanged broad retry,
real model/user config/SDK/env mutation, agents/goals/blocked-provider work or
public exec/usage/settlement/callables/P2g/live/default-on/release enablement.


Changed-source full validation completed November5 Asia/Taipei/November4 UTC:
off remains **FAILED1622/4/8**, exit101,427.17s whole command; gadgets1769/0/10
passes, exit0,77.20s. All nine additive cases and original environment/auth cases
pass BOTH modes; every old named/ignored/doctest inventory is retained, no warnings.
The SAME four failures each have zero buffered bytes in CURRENT child fd1/2 at
their original native query, with one original Pending poll/no wait-selection
return. That is not original pipeline/prior emission/startup/wait-cause proof or
a root fix. The offline guard's temporary absent startup output later cleared
and its original assertion passes. No unchanged broad retry; #138 stays OPEN.
See [the complete pipe diagnostic validation](docs/native-pipe-snapshots.md).


## Explicit owned-child native stack experiment — November 5, 2026

The user approved ONE bounded native stack capture of a newly owned failing
child, expanding no-sampling only for an explicitly marked cfgtest/macOS
experiment. PR148/main151d4bf are all-four/every-step/both-mode verified; local
off1622/4/8 remains FAILED/#138 OPEN. The new private seam borrows the SAME actual
pre-kill ProcessTree/Call, selects only a fixed libtest diagnostic case plus
explicit owned directory marker, never old/reused/caller PID or runtime role
authority. Normal CI/unmarked/production paths do not sample. One consumed budget,
fixed1s/1000ms tool request,5s execution/2s owned cleanup/8KiB pipes/1MiB trace
and sampler-only file limit; actual return/signal/identity/hash facts, no retry.
Sampling SUSPENDS/RESUMES threads: perturbation is explicit, not passive wait/exec/
EOF/usage/quiescence/cause proof. Original native13-query/8-thread/call/view/4096
report/ticket/clock and old assertions/deadlines/production/pump/reader/waker/
ProcessTree/environment/framework/SDK/WIT/deps/pins/workflows/frontends/frames/
schemas/goldens/trace remain intact. Guards/docs in progress; no inferred root
fix, public exec/usage/settlement/callables/P2g/live/default-on/release, model/
installed gadget/user config/env/SDK mutation, agents/goals or blocked-provider
work. See [the owned-stack scope and validation](docs/owned-child-stack-capture.md).


Owned-stack experiment completed: ONE newly owned pre-kill child trace succeeded
(exit0/no signal,1.127321s,859 bytes) with a single `_dyld_start + 0` frame and
no binary-image description; symbol-lookup warning is retained. It is a perturbed
loader-entry userspace observation, not kernel wait cause/exec/EOF/usage proof or
a root fix. Original off1630/4/8 remains FAILED, gadgets1777/0/10 passes; all8 new
guards and original environment/auth cases pass, warnings0. The initial free
marker filtered two rustdoc cases: a targeted unmarked doc-only companion restores
exact combined inventory, NOT a claim of first-command full doctest coverage.
Future authorized markers use a nonmatching --skip VALUE; metadata-only unit/doc
lists prove no actual case exclusion. No unchanged broad retry/second sampler/old
PID lookup/assertion/deadline/launcher/permission workaround; #138 OPEN. See
[the complete owned-stack evidence](docs/owned-child-stack-capture.md).
