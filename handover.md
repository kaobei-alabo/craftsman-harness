# Craftsman Harness — engineer handover

**Snapshot: November 5, 2026 (Asia/Taipei).**
Current repository: `yumingchangsabodota/craftsman-harness`.
The owner authorized a fresh source-only Git history on November 5, 2026.
The following baseline hashes and issue/CI references describe the inherited
`trend-mgai/craftsman-harness` snapshot; they are not commits in the new history.
The latest remote `main` was fetched and matched local `main` at
`b18c00b2da1a48385911e4364db722a84d8eaa7b` before this documentation commit.

This is a practical starting point for someone new to the project. It describes
that source baseline and clearly labels later, local investigation evidence.
Read [AGENTS.md](AGENTS.md) before editing; it contains the detailed constraints.
The inherited handover was committed to local `main` without a push. See
[production readiness](docs/production-readiness.md) for the new repository's
implementation and publication state.

The takeover implementation has now merged as [PR #1](https://github.com/yumingchangsabodota/craftsman-harness/pull/1).
Its final PR head passed every required Linux/macOS/Windows job and both full
native test modes. Independent merged-main validation is recorded separately.
The next terminal reliability change makes Ctrl-C reach approval/question
waiters without granting tools; see [terminal cancellation](docs/terminal-cancellation.md).
The legacy v2/v4 WebSocket commands were later retired at the owner's request;
retained external clients use ACP stdio or its WebSocket bridge.
The owner selected terminal
reliability and self-built gadgets as the current production priority on November 6.

## 1. Read this first

- The existing terminal agent works. The new headless agent has substantial
  **private implementation**, but **`craftsman exec` is not available yet**.
- The Craftsman rebrand is complete in the source. Use the current CLI, crate,
  configuration and environment names; do not reintroduce the former identity.
- Gadget discovery is already deterministic. Do not add repeated model-driven
  gadget-list calls to solve discovery.
- Temporary shell-investigation instrumentation has been retired. Maintain the
  actual native behavior tests and preserve failed historical evidence (section 6).
- The proposed **private `--max-rounds` argument adapter** is now implemented
  through the existing shared request meter. See
  [request-budget arguments](docs/request-budget-arguments.md) for its contract
  and [production readiness](docs/production-readiness.md) for validation.
- Keep public headless execution, headless gadget callables and release gates off.
  Passing a private test is not proof that those features are ready to ship.

### Your first 30 minutes

1. Read this file, [runtime status](docs/runtime-status.md), and R5 in the
   [headless specification](docs/headless-mode.md).
2. Check the branch, tracked changes and worktree layout before changing files.
3. Read the request meter and input factories listed in section 5.
4. Write additive tests for the next private adapter before connecting it.
   Use synthetic providers and isolated fixtures, not real model accounts.

## 2. What this repository does

A *harness* is the machinery around an agent: constructing model requests,
running tools, applying permissions, maintaining state and deciding when to stop.
The application adds terminal UI, configuration, HTTP, subprocesses and storage.

| Location | Responsibility |
| --- | --- |
| [crates/craftsman-rs](crates/craftsman-rs) | Application: CLI/TUI, providers, permissions, filesystem, shell, gadgets and local storage |
| [crates/craftsman-harness](crates/craftsman-harness) | Small reusable agent framework: model/tool loop, middleware, tasks and state interfaces |
| [crates/craftsman-gadget](crates/craftsman-gadget) | Gadget SDK and WebAssembly interface definitions |
| [crates/craftsman-gadget-macros](crates/craftsman-gadget-macros) | Gadget authoring macros |
| [docs/craftsman-harness-plan.md](docs/craftsman-harness-plan.md) | Roadmap; its October 16 baseline and target diagrams are historical/design material, not current-runtime claims |
| [docs/runtime-status.md](docs/runtime-status.md) | Maintained map of implemented, private and planned behavior |

Simplified private headless flow:

```text
original native arguments + explicit launch defaults
  -> complete pure argument/permission/source validation
  -> read-only bootstrap and retained filesystem authority
  -> whole instruction/stdin/attachment admission
  -> session preparation, if selected
  -> Runtime -> the SAME shared application session builder
       -> one invocation gate and one shared request meter
       -> main / delegated / compaction providers
  -> original private Run and outcome
  -> checked terminal views / guarded private file preview
       (NOT public exec-result, usage, event or trace producers)
```

Do not make a second builder that silently restores legacy defaults. The private
builder intentionally leaves memory, MCP, LSP, background services, raw journal/
checkpoint writers and model-callable headless gadgets disconnected.

## 3. What works, and what does not

### Existing supported source paths

- `craftsman` TUI, legacy `-m` one-shot mode and existing transport adapters.
- A separate restricted `service-agent`, without local shell/filesystem/database
  access. It must not inherit the terminal agent's capabilities.
- Gadget runtime, authoring, declarative extensions and create-only migration.
  Gadget model visibility remains user opt-in through `gadgets.enabled`.
- Local-only thread/checkpoint/memory storage; there is no remote replica sync.

### Gadget discovery: the earlier requirement is implemented

`GadgetCatalogMiddleware` refreshes before eligible main and delegated model
rounds and after compaction. Newly installed, eligible gadgets are picked up at
those boundaries by deterministic code. Changes produce change-only catalog
metadata, not a replaced tool definition/system prefix. This is **not** a promise
of an always-running filesystem watcher or permission-free instant execution.
Keep the middleware protected from profile exclusion and preserve first-load
approval. See [gadgets.md](docs/gadgets.md).

### Private foundations already implemented

Explicit permissions; retained `ConfinedFs`; input-first preparation; the shared
runtime builder; frozen credential/redaction scopes; selected guest/shell/
filesystem/web/framework/request/error views; bounded native pipe stdin; source,
control and relative-scope/file arguments; retained-message/terminal encoders;
and guarded one-file publication.

R5 also has a shared request-only cap, a separate optional token-reservation path,
and admission-correlated send/header observations. These are private foundations,
not a completed usage or billing system.

### Still missing / not shipped

- Public `craftsman exec`, its complete flags and full stdin/console/file support.
- Successful-provider-response and alternative-state publication boundaries.
- Headless gadget callable registration, catalog and compatible public provenance.
- Real usage fallback/settlement, billing completeness and a truthful public ledger.
- Public schema-backed results/events, storage redaction and trace producers.
- Complete shutdown/signal handling, native OS containment and verification.
- Live model-quality, default-on and published-release approvals.

The accepted R1–R7 specification and #97 token-admission clarification are
**already accepted**, not awaiting another semantic vote. Acceptance is different
from implementation, enforcement and release approval.

## 4. Toolchain and safe local setup

- Use [rust-toolchain.toml](rust-toolchain.toml): **Rust 1.99.1**, edition 2024,
  `rustfmt`, `clippy`, and `wasm32-wasip2`. Do not substitute a moving `stable`
  toolchain or change global defaults to make a build pass.
- The application manifest now declares MSRV 1.99, matching its tested pinned
  toolchain family. Standalone framework/gadget SDK/macros retain their separate
  1.85 declarations; use the pinned toolchain for this workspace.
- Wasmtime and wasmtime-wasi are pinned to **48.0.8**. No unrelated dependency or
  toolchain upgrade belongs in the argument-adapter task.
- The default voice build needs CMake, plus ALSA development headers on Linux.
  A feature-off source build avoids voice and the gadget host.

From the repository root, start with read-only inspection and a source check:

```sh
git branch --show-current
git status --short --untracked-files=no
git worktree list
cat rust-toolchain.toml
cargo check --locked --no-default-features --workspace
```

If the pinned toolchain is missing, use the repository's explicit toolchain setup
rather than upgrading everything. Do not run `install.sh`, enable gadgets, alter
real user config/environment/SDK files, or call a live model just to validate this
handover or the next private parser.

## 5. Next implementation: private request-budget arguments

**Status: implemented after takeover; the original planning contract follows.**
The separate `input::request_budget` route and typed invocation factories now
exist, with 30 additive tests covering syntax, actual role sharing, native stdin
and guarded publication. Public exec and usage settlement remain unimplemented.
The aim is to let an explicitly selected private argument path choose the
existing request cap. Do not expose a new public CLI command in this slice.

### Required contract

- The accepted flag is **`--max-rounds`**, not `--max-requests`.
- Default: **50**. Valid range: **1 through 10000**, inclusive.
- Despite its name, it limits shared **logical model-request admissions** across
  main, delegated and compaction providers, not just main conversational rounds.
- The future public result field is `limits.max_requests`; the separate `rounds`
  field counts main-agent requests. Do not emit a public result here.
- Internal transport retries remain inside their logical admission. Do not charge
  extra logical calls for retries, refund failed calls or reset existing history.
- Validate all arguments before configuration reads, stdin inspection, session
  IDs, store opening or provider effects.

### Read these files in order

1. [docs/request-admission.md](docs/request-admission.md): cap semantics and stop limits.
2. [exec/meter/mod.rs](crates/craftsman-rs/src/exec/meter/mod.rs):
   `RequestLimit::new`, `RequestLimit::default`, concrete roles and shared meter.
3. [exec/runtime/mod.rs](crates/craftsman-rs/src/exec/runtime/mod.rs):
   `Runtime::build_with_request_limit` already exists.
4. [exec/input/invocation.rs](crates/craftsman-rs/src/exec/input/invocation.rs):
   ordinary preparation chooses the default; the separate token-limit factory is
   private. `Run::request_counts` exposes actual private historical observations.
5. [input/arguments.rs](crates/craftsman-rs/src/exec/input/arguments.rs),
   [input/frontend.rs](crates/craftsman-rs/src/exec/input/frontend.rs) and
   [input/scopes.rs](crates/craftsman-rs/src/exec/input/scopes.rs): existing bounded
   original-`OsString` grammars and opaque routes. They still reject limit flags.
6. [runtime/request_meter_tests.rs](crates/craftsman-rs/src/exec/runtime/request_meter_tests.rs):
   examples using the actual builder, main/child/compaction and synthetic providers.

Supporting contracts: [input arguments](docs/input-arguments.md),
[control arguments](docs/control-arguments.md),
[scope/output arguments](docs/scope-output-arguments.md),
[token reservations](docs/token-reservations.md) and
[admission observations](docs/admission-receipts.md).

### Small implementation plan

1. Add a narrow, explicitly selected **private** budget-control adapter. Preserve
   old default routes and unsupported-flag rejection instead of widening every
   frontend or adding an escape hatch to `SessionExtras`.
2. Admit native argument count/token/total lengths before UTF-8 conversion or
   copies. Support split `--max-rounds 2` and joined `--max-rounds=2`, with one
   occurrence only. Return fixed safe errors for malformed values.
3. Reuse the complete old source/permission/session/scope grammar. A scan must
   preserve split flag values as data and stop interpreting options after `--`.
   Removing a flag must never turn invalid original input into a valid invocation.
4. Produce the existing opaque `RequestLimit` before input-first preparation.
   Connect it through the existing invocation factories to
   `Runtime::build_with_request_limit`; do not accept a caller-created Runtime,
   gate, meter, filesystem path authority or post-run destination switch.
5. Prove main, delegated and compaction providers share the **same actual-builder
   meter/gate**. Reuse the existing refusal/approval logic rather than inventing
   a second counter or public exit mapping.
6. Leave token-budget, public format/trace/gadget and other unimplemented flags
   rejected. Preserve optional token reservations and all existing preview paths.
7. Update README entry points, runtime status and the relevant argument/R5 docs
   in the same slice, including the fact that public exec remains unavailable.

### Acceptance tests to add

These are **planned private-parser vectors**, not runnable public CLI examples.

| Original argument vector | Expected selection |
| --- | --- |
| `["task"]` | Existing default of 50 |
| `["--max-rounds", "2", "task"]` | Cap 2 |
| `["--max-rounds=2", "task"]` | Cap 2 |
| `["--", "--max-rounds=2"]` | Literal prompt; cap stays 50 |
| `["--prompt-file=--max-rounds=2"]` | Literal file value; do not extract a budget |
| Split source flag followed by an option-looking value | Preserve the old grammar's refusal; do not promote the value into a control |
| Duplicate, missing, empty, zero, negative, noninteger, overflow or greater-than-10000 cap | Refuse before effects |
| Unsupported `--max-requests`, token/output/trace/gadget controls | Continue refusing |

Also prove: invalid native UTF-8/resource bounds; option ordering; preservation of
original source/file bytes; no config/stdin/ID/store/provider effects on syntax
failure; exact final allowed response; next-call refusal without another inner
request; role sums; real approval dominance; no later registered-tool dispatch
when a child/compaction catches a budget error; and no invented/refunded history.

## 6. Historical local shell investigation

Former issue **#138** concerned intermittent local macOS full-suite stalls:
an actual child shell existed, but tests saw no output or ready signal before
their normal deadlines. At the November 5 deferral, the retained feature-off
run was **1632 passed / 2 failed / 8 ignored**. The failures covered normal
shell output and stopping a running shell after a filesystem approval refusal.
A single owned userspace sample observed `_dyld_start + 0` without identifying
the kernel wait cause. Earlier four-failure runs and the final two-failure run
remain historical failures, rather than a current repository issue.

On November 11 the owner requested removal of the temporary test-only
recorders, native snapshots, sampling hooks and their dedicated docs. Actual
output, timeout, invalid-byte and approval-stop behavior checks remain.
Retiring instrumentation does not establish a root-cause fix. Git history and
private evidence under `~/.codex/evidence/craftsman-harness/` preserve the original
records; do not mutate sealed evidence or inspect historical PIDs.

## 7. Guardrails that must survive the next change

- **Dependency boundary:** the framework must not gain app HTTP/database/UI
  dependencies. Its [dependency guard](crates/craftsman-harness/tests/dep_guard.rs)
  enforces this.
- **Actual authority:** opaque scopes carry the real retained filesystem,
  captured credentials and unique invocation gate. Do not replace them with
  caller-supplied paths/gates/seeds, ambient reopening or legacy defaults.
- **Original input versus safe view:** execute original authorized data. Redact
  original bytes before cuts/retention; never execute a redacted alias or format
  raw retained state as a safe result. Do not expose secret-bearing Debug,
  Serialize or error text.
- **Accounting honesty:** admission is not a send, a header is not body EOF, and
  allocation is not consumption. Missing usage is not zero. Token allocations
  remain **UNSETTLED**; no fabricated billing, zero reads or completion receipts.
- **Typed stops:** real approval-required state is independent of budget refusal
  and dominates classification. Error strings and success metadata are not
  permission/stop authority. Closing a gate does not prove native work stopped.
- **Process safety:** all agent-started shells go through `platform_os::ProcessTree`.
  Keep Unix process groups default-off until signal routing exists; preserve the
  Windows Job Object rules and original spawn/read/drain/timeout behavior.
- **Compatibility:** preserve `SafeBackend` terminal sanitization. Desktop protocol
  v4 is frozen; ACP remains standard, with declared `_meta.craftsman.*` or
  `_craftsman/*` extensions. Do not casually regenerate goldens, schemas or the
  old fixture corpus.
- **Validation:** no weakened old assertions, skips, startup/deadline/native query
  bounds, warning suppression, fake bytes/PIDs/readiness, or unchanged broad
  retries to manufacture green results.
- **Scope:** use isolated config/pipes/synthetic providers. No real model calls,
  installed-gadget execution, user/global environment/config/SDK mutation,
  manual benchmarks, new agents/goals, tags or releases for this slice.
- **Separately blocked provider work:** do not inspect, execute, publish, retry or
  work around PR112, branch `p2/headless-provider-response-control`, commit
  `14c405db80b939fd3a4af682a076a17d4b99fb15`, or worktree
  `~/.codex/worktrees/538e/ethereal-wood-9c2e4ab1`. The request-argument task must
  remain independent of that blocked work.

## 8. Validation and later merge process

This handover changes Markdown only. Do not rerun an unchanged broad native suite
just for it; check the document's links, whitespace and Git diff. Historical
failed evidence remains failed, and no new hosted CI run is claimed.

For a **genuinely changed implementation**, start with focused additive tests,
then preserve complete validation in both feature modes:

```sh
cargo fmt --all -- --check
cargo check --locked --no-default-features --workspace
python -I docs/schemas/check_exec.py --self-test
python -B scripts/test-benchmark-gadgets.py
cargo test --locked --no-default-features --workspace --no-fail-fast -- --test-threads=4
cargo test --locked --no-default-features --features craftsman-rs/gadgets --workspace --no-fail-fast -- --test-threads=4
```

Record the exact source head, command, exit status, whole-run result, original
named/ignored/doctest inventory and warnings. Keep a failing row separate from
focused or later passes; stop to diagnose a failure rather than retrying an
unchanged whole suite. Let Cargo rebuild current source: **do not directly run
cached test executables**, which may contain the removed temporary probe.

Before a later implementation merge, independently verify **all four exact-head
CI jobs** and every setup/test/mandatory/post step, including both test modes:

- `fmt`
- `test (ubuntu-24.04)`
- `test (macos-15)` — the new repository's check name and actual runner
- `test (windows-2025)` — required in the new workflow

An overall green badge can hide a failed informational Windows step. Admin merge
may override only missing formal review, **never failed checks**. Independently
audit merged-main results before any data-preserving live fast-forward. The
current request authorizes **none** of those remote actions: this is a local-only
documentation commit, with no push, PR, merge, tag or release.

Later work needs separately scoped implementation of the missing producer/
usage/settlement/provenance/input/shutdown boundaries in section 3. Do not turn
on public exec merely because the private request-argument adapter passes.

## 9. Git and uncommitted work at handover

- Local `main` was brought to the verified remote tip before this file was added;
  it then receives one local documentation commit containing **only `handover.md`**.
  Remote `main` does not contain this handover unless publication is explicitly
  authorized later.
- A separate worktree remains at
  `~/.codex/worktrees/7752/ethereal-wood-9c2e4ab1`, branch
  `fix/macos-owned-launch-stall`, based on the same source baseline.
- Its uncommitted files are `AGENTS.md`, `README.md`, `docs/headless-mode.md`,
  legacy diagnostic notes and `docs/runtime-status.md`. They held local
  investigation/deferral notes, **not a Rust implementation**. They were preserved,
  not silently included in this commit. Review them separately before carrying
  any updates into a later change.
- The main checkout also has an untracked `.claude/` directory. Preserve it as
  user data; do not traverse, stage, clean or delete it.
- Do not reset local `main` to `origin/main`, use `git clean`, stash untracked
  data, or stage everything with `git add .`. Inspect tracked/index changes and
  stage explicit paths only.
- For later work, fetch remote `main`, inspect divergence and create a dedicated
  implementation branch. Remember this local-only commit when reconciling a
  newer remote tip; do not overwrite it or accidentally publish it with the next
  push. Main is already checked out elsewhere, so do not force-switch another
  worktree onto it.

## 10. Short glossary

- **Private / unwired:** code exists and may have tests, but is not exposed as a
  supported public command or producer.
- **Logical request:** one model-provider call admitted by the shared meter;
  transport retries are attempts inside it.
- **Admission / reservation:** permission and budget capacity to attempt work,
  not proof of traffic, measured usage or payment.
- **Compaction:** a model request that summarizes context; it shares the cap.
- **Opaque scope:** an unforgeable-by-caller binding to the actual runtime's
  captured authority, rather than a bag of paths/settings.
- **Fail latch / invocation gate:** shared typed stop/permission state checked by
  every guarded role; closing it blocks later work, not necessarily already-started
  OS operations.
- **Retained filesystem authority:** validated directory handles carried into IO,
  not a path checked once and reopened through the ambient filesystem later.
- **Terminal lease:** revocable authority to publish checked private views after
  a Run is consumed; it is not a public result or proof of process quiescence.
