# Craftsman production readiness

This is the current starting point under `kaobei-alabo/craftsman-harness`,
following the November 5, 2026 takeover and November 6 continuation. The handed-over source is preserved in
one new root commit. The old remote, branches, tags, reflogs and commit objects
were removed from the inherited checkout. The active workspace and personal
checkout also use the new history and remote. Old hashes/issues/CI runs in the
[handover](../handover.md) and dated docs are inherited engineering records.

## What the code can do

| Area | Current implementation | Remaining production work |
| --- | --- | --- |
| Terminal coding agent | Streaming model/tool loop, approvals, owned cancellation/signal cleanup, file/shell/web tools, required per-tool/manual-compaction checkpoints and interrupted-session warnings | Live task/console evaluation and remaining background/diagnostic recovery and containment work |
| Self-built gadgets | Embedded Rust SDK, create-only scaffolding, native builds, atomic WASM publication, capability checks and deterministic discovery before model rounds | Keep opt-in and separate build/load approvals; gather live quality evidence |
| Private headless foundations | Explicit permissions, retained confined filesystem, captured/redacted views, input-first preparation, checked private output and shared admission meter | Successful response/output/storage boundaries, real usage settlement and public result/event producers |
| Request-budget arguments | New private `--max-rounds`, default 50/range 1..10000, shared main/delegate/compaction cap | Public CLI integration follows the remaining headless contracts |
| Process lifecycle | Bounded shell capture and owned cancellation, scoped TUI Unix groups/Windows Job Objects, signal shutdown; native behavior regression checks | Validate native containment and exceptional cleanup |
| Distribution | MIT license for the project's original work; authenticated repository, verified implementations merged, pinned toolchain and source validation; binary/image publishing disabled | Explicit owner approval before restoring publication, plan-supported branch protection and remaining provenance/distribution review |

The roadmap in [craftsman-harness-plan.md](craftsman-harness-plan.md) includes
future events, recovery, editing, context and containment work. Its historical
baselines and target diagrams do not describe completed features. Public
`craftsman exec` is still absent. Existing `-m` is a legacy one-shot frontend;
its broad configured permissions are not an unattended isolation contract.

The owner selected **terminal reliability and self-built gadgets** as the current
production priority on November 6. The dialog-cancellation changes improve the terminal.
The former desktop WebSocket transport has been retired in favor of ACP. Future unattended headless work keeps
its separate accepted contracts and implementation gates.

The merged [terminal model controls](terminal-models.md) change addresses a model
switch that previously showed the old transcript while giving the new agent an
empty history. Model/effort selectors now retain state and checkpoint sequencing;
the organization-owned main run 42629991063 passed all four jobs/every step and
both native modes, retaining original cases and the 19 added entries per mode.
The earlier billing-blocked runs remain failed records. The next
[named provider/catalog implementation](provider-models.md) adds four explicit
API protocols, nested model settings and durable provider-owned continuation
data. Its source validation is separate from live model/gadget quality and
provider compatibility, which remain production gates.

The provider change is merged as PR #8 at `45aa2d1`; independent main run
42776674728 passed all four jobs, every step and both native modes with the
reviewed inventory and warning signatures intact. The
[durable manual-compaction change](terminal-compaction.md) closes a gap where
`/compact` reported success before saving its replacement checkpoint. Fresh
validation of this change is recorded separately from provider evidence.

The same continuation adds [provider-aware setup](provider-setup.md): named
connections are edited and selected as a complete validated candidate, keeping
other providers/model settings and the existing conversation. Failed saves
retain the current session. This fixes setup writing ignored flat connection
fields and an invalid unqualified model selection.

That continuation is merged as PR #9 at `5e4f271`; corrected PR run 42834187156
and independent main run
[42841802167](https://github.com/kaobei-alabo/craftsman-harness/actions/runs/42841802167)
pass all four jobs, every applicable step and both full modes. Fresh local
verification passes all nine checks (core 2005/0/17; gadgets 2161/0/20).

Temporary test-only shell phase recorders, native snapshots and manual sampling
hooks have been retired at the owner's request. Their dedicated investigation
docs are removed; actual output, exit, timeout, invalid-byte and approval-stop
checks remain in the maintained suites. Historical failures remain preserved in
Git and private evidence. Retirement is cleanup, without a root-cause fix claim.

Fresh local verification of the retirement passes all nine checks, including
core 1927/0/17 and gadgets 2083/0/20 without warnings. The inventory audit accounts
for exactly 81 retired diagnostic entries and three converted behavior cases in
each macOS mode, retaining every unrelated named/ignored/doctest entry. Hosted
platform and independent merged-main verification remain required for delivery.

The first retirement run 42856977134 passed every job and native mode, but its
warning audit rejected one newly unused Windows test-only PID accessor warning
per mode. The accessor is restricted to its remaining Unix test callers; this
does not change production process handling or native bounds. Fresh corrected
source verification remains distinct from that first run.

## Retained external protocol: ACP

The [stable-v1 compatibility audit](acp-compatibility-audit.md) preserves the
original failures. The corrected adapter passes the same 39 local strict-client
checks; seven additional native regressions cover effects, isolation,
permissions, MCP, replay, EOF and model transfer. [ACP behavior](acp.md) describes
the interface and limits. Two ACP golden recordings have explicit reviewed
identity/cancellation diffs; bridge/model/schema/SDK/native limits are unchanged.
Full local native modes, all four hosted jobs and independent-main proof remain
separate delivery gates. The first hosted Ubuntu job failed APT repository DNS and
skipped mandatory default/core steps; its gadget pass does not clear that
failure. CI now selects the HTTPS Ubuntu package endpoint hosted by kernel.org without changing
package requirements, suites/signing keys, runners or tests; the audit retains
the original failure. No paid calls or publication are enabled.

The owner retired the `serve` and `desktop-serve` commands and their custom
WebSocket v2/v4 implementations on November 11. External clients use `acp-serve`
(stdio ACP) or `acp-bridge` (WebSocket relay of the same ACP JSON-RPC plus the
existing HTTP health/stats surface). Existing authentication, identity headers,
permission/cancellation/session behavior and ACP frame goldens are preserved.

Retired clients must migrate their message contract to ACP. Terminal setup,
editing, task controls and microphone voice remain; uploaded browser PCM and
custom desktop workspace/configuration/voice operations are removed, without
adding replacement ACP extensions. User config, threads, memories, checkpoints,
SQL layout and native shell limits remain intact. Database compatibility helpers
still used by fixture tests are retained under cfg(test); unused desktop catalog
and A2UI-sink APIs are removed from runtime code. Validation must inventory the
retired server tests separately and retain all ACP and unrelated native coverage.

Cleaned `scripts/verify.sh` passes all nine checks without warnings: core
1882/0/17 and gadgets 2038/0/20. Exact differences are 51 retired/moved old entries
and six moved/new entries per local mode; all unrelated named/ignored/doctest
entries and all retained ACP golden bytes remain. The default build's 17 retained
voice tests also pass without starting microphone capture or loading a model.
The first compile/helper errors and the first full run's unused-helper warnings
remain separate from these corrected results. Fresh platform and independent
merged-main verification remain required.

## Build and use the gadget workflow

The user-authorized [live starter evaluation](live-gadget-evaluation.md) on
November 13 completed four actual agent build sessions and four fresh use
sessions, split between `gpt-6.1-sol` and Claude Fable 5.2 on Bedrock. All 30
independent cases passed, including invalid-input/overflow refusals. The reviewed
pure gadgets were installed create-only with ephemeral invocation grants; global
manual permissions and live shell limits were preserved. This is specific live
task evidence, distinct from scripted tests and general production/containment
claims. The former platform example is retired because its service is unavailable.

Use the exact **Rust 1.99.1** toolchain in `rust-toolchain.toml`. The application
now declares the tested 1.99 compiler floor instead of the inherited 1.85 claim;
the standalone framework/SDK/macros retain their separate 1.85 declarations.
Gadget engine pins remain Wasmtime/wasmtime-wasi **48.0.8**.

```sh
rustup toolchain install --no-self-update
cargo install --path crates/craftsman-rs --locked --no-default-features --features gadgets
craftsman setup
```

The install provides `craftsman` on the Cargo bin path, so the agent can invoke
its own authoring CLI. Configure an explicit model and OpenAI-compatible route
through setup. To opt in, add `"gadgets": {"enabled": true}` to the selected
user config. Start with manual permissions:

```sh
craftsman --permission-mode manual
```

An example task is: “Build a reusable gadget that counts words and lines,
compile it, then use it on this text.” The built-in `build-gadget` skill teaches
the SDK and authoring sequence. Native Cargo builds retain shell approval and
environment scrubbing; capability approval occurs before even guest `describe`.
WASM calls inherit no stdio, environment, sockets or filesystem preopens. Reads,
writes and HTTP go through declared host capabilities. Newly eligible callables
appear before the next model round without routine `gadget_list` polling.

```sh
craftsman gadgets doctor
craftsman gadgets new word-count
# Edit the generated Rust source and declare only required capabilities.
craftsman gadgets build word-count
```

Builds are native host operations; the WASM sandbox applies to the compiled
gadget runtime. Failed builds preserve the previous component. The existing
`gadget_authoring_test` exercises agent scaffolding/editing/build/discovery/call
in one session with a synthetic provider and isolated configuration. It is
behavior coverage, not evidence of live model quality or unrestricted execution.
See [gadgets.md](gadgets.md) for the host contracts and project scopes.

## Verification entry point and CI

The README is a concise product introduction. The former detailed configuration,
implementation notes and layout are preserved in the
[November 13 README archive](archive/readme-2026-11-13.md); its dated/retired claims
remain historical. Current architecture and limits are in
[runtime-status.md](runtime-status.md) and the feature docs.

```sh
./scripts/verify.sh
```

The script selects an installed Python 3.11+ interpreter, checks formatting,
locked core/default builds, accepted exec conformance, benchmark-driver
contracts, documented source provenance, both full workspace feature modes and
unchanged goldens. A failed
core suite does not hide the gadget suite, and any failure makes the final
script exit nonzero. CMake is required for the default voice build; Linux also
needs ALSA headers, pkg-config and clang. Source validation does not run a real
model, enable the user's gadgets, alter user config or publish a release.

The new CI requires all of `fmt`, `test (ubuntu-24.04)`, `test (macos-15)` and
`test (windows-2025)`; Windows is no longer informational. Default voice/gadget
compilation is checked as well as both native test modes. The advisory benchmark
workflow also uses macOS 15. The four matching required contexts should
be configured in main-branch protection. GitHub returned HTTP 403 for the new
private repository because its current account plan requires GitHub Pro for
branch protection. No privacy or billing setting was changed. CI failures fail
the run, but server-side required-check enforcement is not configured.
These are new repository rules; old macOS 14 check labels describe past evidence.

The owner approved lighter draft-PR checks and Cargo dependency caching on
November 12. Drafts run formatting/cache/workflow contracts; promotion to ready
starts the full matrix, as do ready-PR updates, main pushes and manual runs.
The existing final-head and independent-main requirements remain. Compiled
dependency caches are capped at 2 GiB per platform, exclude workspace binaries
and guest fixtures, and are saved only by successful main platform jobs. The cache
helper passed an actual offline Cargo reuse/rebuild probe and contract tests;
hosted cache behavior/timing still require live runner verification. See
[CI validation and cost controls](ci.md). Account billing still blocks hosted
startup; no billing/privacy setting or publication policy was changed.

At the owner's November 6 request, `.github/workflows/release.yml`,
`service-agent-image.yml` and `publish-workspace-images.yml` were disabled in
GitHub and removed from source. This holds tag releases, manual binary artifacts
and container publication. No releases or workflow artifacts existed when the
hold was applied. Normal CI still compiles and tests the agent, and the optional
benchmark uploads only `target/gadget-benchmark.json`. Restore publication only
through a reviewed change after explicit owner approval; no variable or manual
workflow input bypasses the hold.

## Current validation record

| Check | Result |
| --- | --- |
| Locked feature-off source check | Passed with Rust 1.99.1 |
| New private budget tests, feature off | 30 passed / 0 failed / 1 marked helper ignored; no compiler warnings |
| Full core/default/gadget verification | Passed on `d9d87aa`: core 1878/0/10 and gadgets 2033/0/12 (passed/failed/ignored); exit 0; no compiler warnings |
| Gadget authoring end-to-end test | Passed: actual scaffold/edit/build/catalog/call in one synthetic-provider session |
| Release-mode coding/gadget source build | Passed with gadgets and without voice; optimized CLI help/version passed without configuration effects; no release/tag published |
| New GitHub first push | Passed: private repository main began with parentless snapshot `d20e622`, verified against the remote SHA |
| Request-budget hosted CI | Passed on PR #1 head `d3a9b9a`: all four jobs/every expected step and both full modes; details below |
| Request-budget merge | PR #1 merged normally at `774a5d5`; independent main-run verification is separate from the PR result |
| Terminal prompt cancellation | Local corrected `0745e6e` verification passed: core 1887/0/10 and gadgets 2042/0/12, all nine new cases, all checks and no compiler warnings; hosted checks remain separate |
| Historical retired desktop reconnect correction | Corrected local `4c83934` passed all steps, core 1895/0/10 and gadgets 2050/0/12, all 17 new terminal/desktop cases and no compiler warnings; original failed hosted row remains separate |

The first complete run at `96c3a4d` retained **1877 passed / 1 failed / 10 ignored**
in core mode and **2032 passed / 1 failed / 12 ignored** with gadgets, with no
compiler warnings. Both failures were the existing CLI help contract after an
unnecessary description edit; the original help wording is restored without
changing its assertions. All agent/shell/budget unit cases and the full synthetic
agent-builds-and-calls-a-gadget case passed. These failed full-run rows are retained
separately from the corrected-source verification below. The provenance checker
also required four precise interpreter-version exceptions, each with a reason;
no general source or attribution exclusion was added.

The corrected source at `d9d87aa` passed every `scripts/verify.sh` step: formatting,
locked core/default builds, exec contract (141 records/streams, 33 rules, 22 wire
cases), nine benchmark-driver contracts, provenance, both complete native modes
and unchanged goldens. Full logs and named/ignored/doctest inventories are retained
locally under `target/verification/`; the first failed log is kept separately.
That local row is macOS source evidence; hosted results are recorded separately
below. Neither is live model-quality evidence. The ignored inventory preserves diagnostic helpers,
benchmarks and the existing macro documentation example.

[PR #1 CI run 42060279548](https://github.com/yumingchangsabodota/craftsman-harness/actions/runs/42060279548)
completed on `d3a9b9ae43423dddc353484662b3756d0dc9548e`. Every setup, mandatory and
post step passed, with only the Linux prerequisite step correctly skipped on
other platforms. Runner logs identify Ubuntu 24.04, macOS 15 ARM64 and Windows
Server 2025. Complete native results (passed/failed/ignored):

| Platform | Core | Gadgets |
| --- | --- | --- |
| macOS 15 | 1878 / 0 / 10 | 2033 / 0 / 12 |
| Ubuntu 24.04 | 1824 / 0 / 10 | 1979 / 0 / 12 |
| Windows 2025 | 1802 / 0 / 10 | 1946 / 0 / 12 |

All 30 request-budget cases passed in both modes on all platforms; the actual
agent scaffold/edit/build/discover/call test passed in gadget mode everywhere.
All prior Windows named/ignored/doctest inventory is retained, with exactly 30
new cases and one marked helper. Linux/macOS have no compiler warnings. Windows
retains identical baseline warning messages/source-line multiplicities: seven
occurrences in core tests and ten in gadget tests; the new default-build check
repeats five existing warnings. Raw logs and the step/inventory/warning audit are
retained under `target/verification/`. This does not claim warning-free Windows
or substitute the PR run for independent merged-main verification.

The initial snapshot's [main run 42059194736](https://github.com/yumingchangsabodota/craftsman-harness/actions/runs/42059194736)
is retained separately. Its inherited informational Windows job failed the
original `acp_bridge_http_and_relay` case during the bounded real 60-second cold
`/stats` setup: core **1771 / 1 / 9**, later gadgets **1916 / 0 / 11**, interpreter
post step skipped. Overall success did not mean all steps passed. The PR's pass
does not establish a cause or fix for that baseline failure; no unchanged run,
deadline relaxation, warmed setup or assertion change was used to replace it.

The previous engineer's deferred macOS shell investigation is **not fixed** by
these changes. Its retained feature-off failures and perturbed userspace stack
sample remain historical failed evidence. No old assertions, deadlines, process
readers or native query bounds were relaxed. A fresh local pass on another
machine cannot establish the cause or erase those results.

## Next engineering gates

Terminal lifecycle [PR #5](https://github.com/yumingchangsabodota/craftsman-harness/pull/5)
and independent [main run 42170055307](https://github.com/yumingchangsabodota/craftsman-harness/actions/runs/42170055307)
passed every required job/step and both modes with original inventories retained:
macOS core 1927/0/12 and gadgets 2083/0/15, Ubuntu 1873/0/12 and 2029/0/15,
Windows 1847/0/11 and 1992/0/14. Linux/macOS had no compiler warnings; the existing
Windows five/eight occurrences matched the reviewed PR. Isolated scripted probes
also exercised an actual terminal file write, SIGTERM during native work and
terminal restoration without network or host credentials. This is runtime/source
verification, not live model-quality evidence.

The next [terminal recovery](terminal-recovery.md) change saves completed tool
results immediately, stops after failed saves, atomically commits local metadata/
state/pruning and warns about unknown effects. Stable local verification passes
all nine checks: core 1947/0/13 and gadgets 2103/0/16, no compiler warnings, with
every original case and 20 new cases/one helper retained. Hosted and independent
main checks remain separate from the lifecycle results above.

1. Verify shared terminal cancellation through tools/builds, compaction and child
   agents, plus SIGTERM/SIGHUP shutdown and terminal restoration. Evaluate remaining
   exceptional/native containment and durable recovery limits. Verify each
   implementation and independent merged-main run, including
   every required job/step and both Windows modes. The fresh history, login,
   initial push and first implementation's PR verification are complete.
2. Finish successful-provider-response and safe public result/storage/trace
   boundaries, then truthful shared usage settlement. Preserve original execution
   authority separately from redacted views and do not report unknown usage as zero.
3. Finish signal/deadline shutdown and contained evaluation before exposing
   unattended `exec`. Headless gadget registration must use its existing retained
   source/approval host and publish compatible provenance.
4. Run an explicitly configured, budgeted live task/gadget evaluation in isolated
   environments. Resolve the recorded licensing/distribution questions before a
   release; keep existing `NOTICE` attribution and the legacy plugin deprecation
   cycle. See [gadget-validation.md](gadget-validation.md) and
   [oss-review.md](oss-review.md).

This checkpoint improves the working terminal/gadget foundation and advances R5.
It does not assert complete unattended execution, OS containment or release
readiness.

## Is the terminal agent production ready?

It is a working, extensively source-tested agent suitable for supervised trials
in a disposable checkout with explicit model configuration and manual approvals.
It is not yet production ready. Shared cancellation now reaches native builds,
compaction and foreground helpers, and signals enter owned shutdown. The
recovery continuation has isolated checkpoint/crash/reopen verification;
real console-event/task evaluation and remaining background/diagnostic recovery
still need validation. Live
model/task/gadget quality has not been measured. The host shell and
filesystem are not sandboxed by the gadget WASM engine. Keep backups and review
the agent's changes; do not rely on it for unattended production changes yet.
Binary/container publication stays on hold by owner request.
