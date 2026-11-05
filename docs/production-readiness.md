# Craftsman production readiness

This is the current starting point under `yumingchangsabodota/craftsman-harness`,
following the November 5, 2026 takeover. The handed-over source is preserved in
one new root commit. The old remote, branches, tags, reflogs and commit objects
were removed from the inherited checkout. The active workspace and personal
checkout also use the new history and remote. Old hashes/issues/CI runs in the
[handover](../handover.md) and dated docs are inherited engineering records.

## What the code can do

| Area | Current implementation | Remaining production work |
| --- | --- | --- |
| Terminal coding agent | Streaming model/tool loop, approvals, file/shell/web tools, foreground delegates, sessions and local memory | Fresh supported-platform checks and live model/task evaluation |
| Self-built gadgets | Embedded Rust SDK, create-only scaffolding, native builds, atomic WASM publication, capability checks and deterministic discovery before model rounds | Keep opt-in and separate build/load approvals; gather live quality evidence |
| Private headless foundations | Explicit permissions, retained confined filesystem, captured/redacted views, input-first preparation, checked private output and shared admission meter | Successful response/output/storage boundaries, real usage settlement and public result/event producers |
| Request-budget arguments | New private `--max-rounds`, default 50/range 1..10000, shared main/delegate/compaction cap | Public CLI integration follows the remaining headless contracts |
| Process lifecycle | Bounded shell capture, Unix groups/Windows Job Objects and typed stop gates | Complete signal routing/cancellation and native containment; earlier shell issue remains unresolved |
| Distribution | Pinned toolchain and source/release/image workflows | New GitHub credentials, branch protection, fresh hosted evidence and licensing/distribution decisions |

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
contracts, both full workspace feature modes and unchanged goldens. A failed
core suite does not hide the gadget suite, and any failure makes the final
script exit nonzero. CMake is required for the default voice build; Linux also
needs ALSA headers, pkg-config and clang. Source validation does not run a real
model, enable the user's gadgets, alter user config or publish a release.

The new CI requires all of `fmt`, `test (ubuntu-24.04)`, `test (macos-15)` and
`test (windows-2025)`; Windows is no longer informational. Default voice/gadget
compilation is checked as well as both native test modes. Release and advisory
benchmark workflows also use macOS 15. Configure the four matching required
contexts in the new repository's main-branch protection after the first push.
These are new repository rules; old macOS 14 check labels describe past evidence.

## Current validation record

| Check | Result |
| --- | --- |
| Locked feature-off source check | Passed with Rust 1.99.1 |
| New private budget tests, feature off | 30 passed / 0 failed / 1 marked helper ignored; no compiler warnings |
| Full core/default/gadget verification | Pending current-source run |
| Gadget authoring end-to-end test | Pending current-source full gadget run |
| New GitHub first push and hosted CI | Pending GitHub authentication |

The previous engineer's deferred macOS shell investigation is **not fixed** by
these changes. Its retained feature-off failures and perturbed userspace stack
sample remain historical failed evidence. No old assertions, deadlines, process
readers or native query bounds were relaxed. A fresh local pass on another
machine cannot establish the cause or erase those results.

## Next engineering gates

1. Complete source validation and the first push; verify every required hosted
   job and its steps on the new source head, including both Windows modes.
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
