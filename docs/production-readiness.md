# Craftsman production readiness

This is the current starting point under `yumingchangsabodota/craftsman-harness`,
following the November 5, 2026 takeover and November 6 continuation. The handed-over source is preserved in
one new root commit. The old remote, branches, tags, reflogs and commit objects
were removed from the inherited checkout. The active workspace and personal
checkout also use the new history and remote. Old hashes/issues/CI runs in the
[handover](../handover.md) and dated docs are inherited engineering records.

## What the code can do

| Area | Current implementation | Remaining production work |
| --- | --- | --- |
| Terminal coding agent | Streaming model/tool loop, approvals, cooperative cancellation from dialogs, file/shell/web tools, foreground delegates, sessions and local memory | Complete interrupt/signal lifecycle and live model/task evaluation |
| Self-built gadgets | Embedded Rust SDK, create-only scaffolding, native builds, atomic WASM publication, capability checks and deterministic discovery before model rounds | Keep opt-in and separate build/load approvals; gather live quality evidence |
| Private headless foundations | Explicit permissions, retained confined filesystem, captured/redacted views, input-first preparation, checked private output and shared admission meter | Successful response/output/storage boundaries, real usage settlement and public result/event producers |
| Request-budget arguments | New private `--max-rounds`, default 50/range 1..10000, shared main/delegate/compaction cap | Public CLI integration follows the remaining headless contracts |
| Process lifecycle | Bounded shell capture, Unix groups/Windows Job Objects and typed stop gates | Complete signal routing/cancellation and native containment; earlier shell issue remains unresolved |
| Distribution | Authenticated new repository, initial snapshot pushed, first verified implementation merged, pinned toolchain and source/release/image workflows | Plan-supported branch protection, independent main validation and licensing/distribution decisions |

The roadmap in [craftsman-harness-plan.md](craftsman-harness-plan.md) includes
future events, recovery, editing, context and containment work. Its historical
baselines and target diagrams do not describe completed features. Public
`craftsman exec` is still absent. Existing `-m` is a legacy one-shot frontend;
its broad configured permissions are not an unattended isolation contract.

## Build and use the gadget workflow

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
compilation is checked as well as both native test modes. Release and advisory
benchmark workflows also use macOS 15. The four matching required contexts should
be configured in main-branch protection. GitHub returned HTTP 403 for the new
private repository because its current account plan requires GitHub Pro for
branch protection. No privacy or billing setting was changed. CI failures fail
the run, but server-side required-check enforcement is not configured.
These are new repository rules; old macOS 14 check labels describe past evidence.

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
| Desktop reconnect correction | Original Ubuntu gadget reconnect assertion failed on PR #2; owned reader/cleanup/writer handoff and eight additive tests are under corrected validation; see [desktop connections](desktop-connections.md) |

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

1. Verify each new implementation and independent merged-main run, including
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
