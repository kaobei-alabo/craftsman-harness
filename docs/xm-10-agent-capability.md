# XM-10 — `craftsman-rs` autonomous task capability

**Ticket:** XM-10 (`CROSS_MODULE_REGISTER.md`) — InferenceaaS "Agent-as-a-Service".
**Question:** Can the embedded `craftsman-rs` runtime take a **single task intent**
(e.g. "build me a small website") and autonomously drive it to a concrete
deliverable via an OpenAI-compatible endpoint — multi-step plan/act with tool
use (file writes, shell, etc.) — as opposed to only interactive chat turns?

**Verdict: SUPPORTED (capability present and proven by an existing end-to-end
integration test).** The live probe against a *real* model endpoint is
**PROBE-PENDING** — no keyless/authless OpenAI-compatible endpoint is reachable
on this machine without credentials I do not hold (see §6). The autonomous
multi-step tool loop itself is already exercised end-to-end against a mock
OpenAI-compatible server in the repo's own test suite.

Date: 2026-07-29 · Analyst: automated code review (no runtime code modified).

---

## 1. Architecture — is there an autonomous plan-act loop?

Yes. `craftsman-rs` is built on the `craftsman-harness` framework, Craftsman's agent
harness. The core loop is a genuine model↔tool cycle, not a single chat turn.

- **The loop:** `Agent::run_turn` →
  `crates/craftsman-harness/src/agent.rs:199` appends the user message, runs
  `turn_start`, then calls `drive_from_round(0)`
  (`crates/craftsman-harness/src/agent.rs:229`). `drive_from_round` loops
  `for round in start..self.max_rounds` (`agent.rs:230`): each iteration
  samples the model (`agent.rs:268`), and if the assistant message carries
  `tool_calls` it dispatches **each** call (`agent.rs:337-359`), appends the
  tool results to the transcript, and loops again. It is a bounded autonomous
  loop, not a one-shot exchange.
- **Termination:** the turn ends when the model returns a plain answer with **no
  tool calls** (`agent.rs:329-336`), when a middleware returns
  `Flow::Finish` (`agent.rs:315-322`), or when the `max_rounds` guard fires
  (`agent.rs:364`, `AgentError::MaxRounds`). Default `max_rounds = 50`
  (`agent.rs:952`, builder default). No human turn is required for the loop to
  progress or to terminate.
- **Planning:** the framework contributes a `write_todos` planning tool via
  `TodoListMiddleware` (`crates/craftsman-harness/src/builtin/todos.rs:30`,
  system-prompt guidance at `todos.rs:10`) for breaking a complex objective
  into steps.
- **Delegation:** `SubAgentMiddleware` contributes a `task` tool
  (`crates/craftsman-harness/src/builtin/subagents.rs:323`) that launches ephemeral
  or background subagents with isolated context (description at
  `subagents.rs:60`), each running the same autonomous loop. Wired for the main
  agent in `crates/craftsman-rs/src/agent.rs:1004-1010`.
- **Robustness:** HITL suspend/resume (`agent.rs:342-357`, `resume` at
  `agent.rs:372`), cooperative cancel/steer (`agent.rs:628-656`), durable
  checkpoint + JSONL journal (`agent.rs:819-836`), and automatic transcript
  compaction (`CompactionMiddleware`, wired at
  `crates/craftsman-rs/src/agent.rs:984`) all exist.

## 2. What tools can the agent invoke?

The main agent is given a full host toolset (`crates/craftsman-rs/src/agent.rs`):

| Tool | Purpose | Source |
|---|---|---|
| `execute` | Run a shell command on the host | `agent.rs:413` / `HostKind::Execute` |
| `write_file` | Create a new file | `agent.rs:437` |
| `edit_file` | Exact-string edit in a file | `agent.rs:451` |
| `read_file` | Read file (paginated, `cat -n`) | `agent.rs:422` |
| `ls` / `glob` / `grep` | Navigate/search the workspace | `agent.rs:467-500` |
| `web_fetch` / `web_search` | Network egress | `crates/craftsman-rs/src/web_tools.rs`, wired `agent.rs:652-653` |
| `write_todos` | Planning | `builtin/todos.rs` |
| `task` | Delegate to a subagent | `builtin/subagents.rs` |
| `ask_user` | Mid-task clarification (declines cleanly when headless) | `crates/craftsman-rs/src/ask_user.rs`, wired `agent.rs:1060` |
| MCP tools | Platform capabilities (not host-mutating) | `agent.rs:661` `McpFrameworkTool`, LSP tools `agent.rs:885` |

File and shell tools run against a real filesystem backend (`RealFs`, rooted at
`cwd`) — `crates/craftsman-rs/src/agent.rs:807`. So the agent can produce concrete
on-disk deliverables (write a website's files, run a build, etc.).

Host-mutating tools (`execute`/`write_file`/`edit_file`) are gated by
`ApprovalMiddleware` (`crates/craftsman-rs/src/hitl_middleware.rs`) unless the
permission mode pre-approves them (see §4).

## 3. How is the model endpoint configured?

The chat endpoint is resolved by `route::resolve_chat_endpoint`
(`crates/craftsman-rs/src/route.rs:71`) into a `{base_url, api_key, source}`:

- **Direct / "litellm" route** (`route.rs:40`): `cfg.base_url` + `cfg.api_key`,
  defaulting to `https://api.openai.com/v1` when `base_url` is empty. This is
  the "point it at any OpenAI-compatible endpoint" path. Config fields live in
  `~/.craftsman/config.json` (`crates/craftsman-rs/src/config.rs:82-86`: `base_url`,
  `api_key`, `model`).
- **Platform "llm app" route** (`route.rs:88`): when logged in to a platform,
  the OpenAI-compatible base URL is taken from the `llm` app's SDK manifest
  (`App::openai_base_url`) and authed with the JWT. This is how a federation
  **lease endpoint** is consumed.

The wire layer is OpenAI-compatible throughout: `StreamingLlmProvider::complete`
(`crates/craftsman-rs/src/agent.rs:211`) converts framework messages to the OpenAI
chat shape (`to_llm_messages` `agent.rs:251`, `to_openai_tools` `agent.rs:299`),
streams SSE deltas, and parses `tool_calls` back
(`to_fw_message` `agent.rs:318`). Per-model / provider profiles (temperature,
max_tokens, headers) are applied from config (`build_session_ext`
`agent.rs:756-780`).

## 4. How is a task submitted? (CLI vs. protocol vs. chat-only)

There are **three** front-ends onto the *same* autonomous loop; it is **not**
chat-only:

1. **Headless one-shot — the task-intent path.** `craftsman -m "<intent>"`
   (`crates/craftsman-rs/src/main.rs:57` flag, dispatch `main.rs:212-213`) calls
   `run_once` (`main.rs:289`), which builds a session with the
   `NONINTERACTIVE_INSTRUCTIONS` system layer — *"You are running
   non-interactively. Make reasonable assumptions and proceed autonomously
   rather than ask clarifying questions"* (`crates/craftsman-rs/src/prompt.rs:9`) —
   and runs `session.agent.run_turn(message)` (`main.rs:326`). The loop drives
   to completion, streams the answer to stdout, and the process exits
   (`main.rs:336-352`). This is exactly *single task intent → autonomous
   multi-step deliverable*.
2. **Interactive REPL** (the `tui` module) and **`craftsman serve` / `craftsman
   desktop-serve`** (WebSocket protocol, `main.rs:210-211`) — these drive the
   same `Agent` with transcript/HITL/steer surfaced to a UI. The desktop
   `/chat` hook is one of these.

**Permission modes** (`crates/craftsman-rs/src/config.rs:14`,
`main.rs:64-66`) govern whether mutating tools run unattended:

- `--auto-approve` / `--permission-mode bypass-permissions` / `auto`: mutating
  tools run without prompts → **fully autonomous** file+shell deliverable.
- `manual` (default): host-mutating tools are gated. In headless one-shot with
  no approval UI and closed stdin, the gate reads EOF and **denies**, forwarding
  the rejection to the model (proven by
  `tests/loop_test.rs:117` `one_shot_without_auto_approve_denies_on_eof_and_reports`).
  So an unattended deliverable run **requires** an auto-approving mode — this is
  a config choice, not a missing capability.

## 5. End-to-end evidence (existing test — no new code)

`crates/craftsman-rs/tests/loop_test.rs:77`
`one_shot_runs_tool_loop_against_mock_server` drives the **real compiled
binary** in one-shot mode against a mock OpenAI-compatible SSE server:

- Config injected via a temp `~/.craftsman/config.json`:
  `{"base_url": <mock>, "api_key": "test-key", "model": "mock-model"}`
  (`loop_test.rs:88-97`) — i.e. the direct OpenAI-compatible route.
- Command: `craftsman -m "run the echo" --auto-approve` (`loop_test.rs:101`).
- Round 1: the model streams a `tool_calls` request for `execute`
  (`echo loop-ok`); the binary runs the tool. Round 2: the tool result is fed
  back verbatim in the follow-up request (asserted `loop_test.rs:49-59`), and
  the model streams a final plain answer and stops (`finish_reason:"stop"`).
- Assertions: process exits 0, `done: loop-ok` reaches stdout, activity lines
  reach stderr (`loop_test.rs:107-114`).

This is a passing, in-repo demonstration of *task intent → autonomous
multi-round tool use → deliverable → clean termination*, headless, with **no
human turns** — against an OpenAI-compatible endpoint. Two further tests cover
the manual-deny path (`loop_test.rs:117`) and harness/model-profile application
(`loop_test.rs:222`).

## 6. Live probe against a real model — PENDING (reproducible commands)

I did **not** fabricate a live run. Environment scan on 2026-07-29:

- The malaysia-nace federation stack **is up** (`docker ps`): four home-node
  fronts on `127.0.0.1:8100/8200/8300/8400` (gov / partner-a / partner-b /
  craftsman), `mock-gpu-core`, postgres, etc. Their HTTP surface returns
  OpenAI-compatible **error envelopes** (`{"error":{...,"type":
  "invalid_request_error","code":"not_found"}}`), but `/v1/models`,
  `/v1/chat/completions`, and `/api/llm/v1/models` all return *Route not found*
  without platform routing + a JWT. Consuming a lease endpoint therefore
  requires `craftsman login` to a home node (the SDK resolves the `llm` app base URL
  from the manifest and auths with the JWT — `route.rs:58`).
- No keyless local OpenAI-compatible server is reachable
  (`ollama :11434`, `lmstudio :1234` both closed).
- No `~/.craftsman/config.json`, no built `craftsman` binary, `craftsman` not on `PATH`.
- Toolchain present: `cargo 1.97.1` / `rustc 1.97.1` (satisfies edition 2024 /
  rust ≥ 1.85). A build additionally fetches the private
  `craftsman-sdk` git dependency (tag `rust-v0.1.0`, credentialed) and voice deps
  (`whisper-rs`, `cpal`).

### Exact commands to complete the live probe

Option A — direct OpenAI-compatible endpoint (any provider or local server):

```sh
cd ~/Repo/v3/craftsman-harness
cargo build --release --bin craftsman            # fetches craftsman-sdk (needs git creds)

mkdir -p ~/.craftsman
cat > ~/.craftsman/config.json <<'JSON'
{ "base_url": "https://api.openai.com/v1", "api_key": "sk-...", "model": "gpt-4o-mini" }
JSON
# (or a local server, e.g. base_url "http://127.0.0.1:11434/v1" for ollama)

mkdir -p /tmp/xm10-probe && cd /tmp/xm10-probe
~/Repo/v3/craftsman-harness/target/release/craftsman \
  -m "Create a single-file HTML page with an <h1> heading that says 'XM-10 probe' and save it to /tmp/xm10-probe/index.html. Then stop." \
  --permission-mode bypass-permissions
# Expect: multi-round activity on stderr (write_file), /tmp/xm10-probe/index.html
# exists, process exits 0 — with no human input.
```

Option B — federation lease endpoint (platform route):

```sh
cd ~/Repo/v3/craftsman-harness
cargo build --release --bin craftsman
target/release/craftsman login --url http://127.0.0.1:8100 <email>   # gov home node
# resolves the llm-app OpenAI base + JWT; set a model the llm app serves:
target/release/craftsman --permission-mode bypass-permissions \
  -m "Create /tmp/xm10-probe/index.html with an <h1> 'XM-10 probe' heading, then stop."
```

Reproducible fixture (no live model needed) — the existing e2e proof:

```sh
cd ~/Repo/v3/craftsman-harness
cargo test -p craftsman-rs --test loop_test one_shot_runs_tool_loop_against_mock_server
```

## 7. Missing pieces / scope note

**Capability is not partial** — the autonomous task loop, tool set (incl. file
writes + shell), planning, delegation, OpenAI-compatible endpoint config, and
headless one-shot task submission all exist and are tested. The only outstanding
item is an **operational live run against a real model**, which is gated on
credentials (a federation JWT via `craftsman login`, or a direct provider/local
key) and a build of the binary — none available in this analysis environment.

**Demo scope decision (per the plan's exit criterion):** the runtime supports
the "Agent-as-a-Service" reframe — desktop can reframe `/chat` into a
task-workspace that submits a single intent and renders the autonomous
transcript/deliverable, rather than annotating a limitation. Recommended
before the demo: run the §6 live probe once against a lease endpoint to capture
a real transcript, and drive unattended runs in an auto-approving permission
mode (`bypass-permissions` / `auto`).

No runtime code was modified for this verification (docs-only). If a live probe
surfaces a gap, that becomes a separately-scoped runtime step (plan mutation),
not silent work under this ticket.
