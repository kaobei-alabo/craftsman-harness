# Craftsman production readiness

Updated November 16, 2026. The maintained repository is
`kaobei-alabo/craftsman-harness`. Earlier independent runtime proof at `9c5774d`
and subsequent continuation evidence are recorded below. This working terminal/ACP agent has extensive source and
native regression coverage plus bounded live task/gadget evidence. Full
unattended execution, native OS containment and release readiness remain open.

## What the code can do

| Area | Current implementation | Remaining production work |
| --- | --- | --- |
| Terminal / ACP coding agent | Streaming file/shell/web tools, approvals, steering, owned cancellation, durable turns and model changes, task guidance, declared verification and opt-in bounded context/failure controls | Typed context/cache, broader semantic test/effectful-loop controls, live and exceptional recovery evaluation |
| Terminal UX | Gray/turquoise 匠 splash, small hammer/label, grouped model → effort menus, mutation-driven file checkpoints and saved 200-round pauses | Continued long-session usability measurement |
| Host monitoring | OSC 7501 lifecycle and bounded delegated-task records, verified in an actual Orca terminal | Native host chat registration is separate ACP client integration |
| Self-built gadgets | Embedded Rust SDK, create-only scaffolding, native builds, atomic component replacement, capability approvals and change-only discovery | Keep user opt-in and separate build/load approvals; expand representative live evaluation |
| Private headless foundations | Explicit invocation policy, retained confined filesystem, checked/redacted views, input/session preparation and request admission | Successful response/output/storage and real usage settlement; public result/events/trace and headless guest registration |
| Native lifecycle | Owned process groups/Windows Job Objects, bounded output/cleanup and signal restoration | Exceptional cleanup, broader recovery and native containment validation |
| Distribution | Public MIT source and validated local installation; binary/container publication disabled | Owner-approved publication and remaining attribution/provenance/distribution review |

The historical baseline and target architecture in
[craftsman-harness-plan.md](craftsman-harness-plan.md) are not current availability.
[runtime-status.md](runtime-status.md) describes maintained behavior and private
implementation boundaries. Public `craftsman exec` is absent; legacy `-m` uses
its existing configured permissions/storage and is not the unattended contract.

## Local use and model connections

Platform onboarding, inbox polling and remote skill browsing are removed.
Sessions, skills and memory are local. Model/MCP/search traffic uses explicit
configuration; voice-model downloads require opt-in. `/model` opens the grouped
catalog then effort/budget; only the final choice saves/applies. Model changes
preserve committed history and local state without replay. See
[connections](connections.md), [models](provider-models.md) and
[setup](provider-setup.md).

Use the exact Rust **1.99.1** toolchain. The app declares a tested **1.99** floor;
standalone framework/SDK/macros retain **1.85** declarations. Wasmtime and
wasmtime-wasi remain pinned to **48.0.8**. The default installation includes
voice and gadgets and requires CMake; Linux also needs ALSA development headers,
pkg-config and clang.

```sh
rustup toolchain install --no-self-update
./install.sh --upgrade
craftsman setup
craftsman
```

A source build without voice uses:

```sh
cargo install --path crates/craftsman-rs --locked --no-default-features --features gadgets
```

Installation does not replace user configuration, keys, stored sessions or
permissions. The `craftsman-rs` compatibility launcher executes its sibling
`craftsman` binary. The current splash and OSC changes are locally installed.

## Build and use the gadget workflow

Enable `gadgets.enabled` explicitly in the selected user configuration. Start
with manual permissions; native builds and first guest-load capabilities require
separate decisions. The embedded `build-gadget` skill teaches authoring.

```sh
craftsman --permission-mode manual
craftsman gadgets doctor
craftsman gadgets new word-count
# Edit the Rust source and declare the needed capabilities.
craftsman gadgets build word-count
```

New eligible callables appear before the next model round. WASM execution has
no inherited stdio, environment, sockets or filesystem preopens; declared host
capabilities govern reads/writes/HTTP. Native Cargo/build scripts still execute
on the host with the shell's separate approvals and limits. Failed builds keep
the previous component. See [gadgets.md](gadgets.md).

The [live starter evaluation](live-gadget-evaluation.md) completed four actual
build sessions and four fresh use sessions with GPT/Claude models. All 30
independent input/overflow cases passed. Native authoring tests and advisory
runtime benchmarks measure different properties; see
[gadget-validation.md](gadget-validation.md).

## Retained external protocol: ACP

`craftsman acp-serve` provides standard ACP over stdio; `acp-bridge` relays the
same JSON-RPC over WebSocket and serves health/stats. The former `serve` and
`desktop-serve` commands and custom v2/v4 servers are removed. Retired clients
must migrate their messages to ACP.

The [stable-v1 audit](acp-compatibility-audit.md) retains its original failures and
corrected strict-client/regression proof. Existing authentication, identity,
session/permission/cancellation behavior and retained ACP goldens are preserved.
Craftsman extensions use the declared ACP metadata/method boundaries. OSC 7501
adds terminal monitoring; native Orca chat registration remains host work. See
[ACP](acp.md) and [terminal status](terminal-status.md).

## Verification entry point and CI

```sh
./scripts/verify.sh
```

The script selects an installed Python 3.11+ interpreter and runs all 14 checks:
formatting, locked core/default builds, accepted exec conformance, benchmark
contracts, CI-cache/workflow contracts, coding-budget/scope/preflight/options contracts,
provenance, both full workspace feature modes and unchanged goldens. A failed
core mode does not hide the gadget mode. Default builds include voice/gadgets.

Required hosted contexts are `fmt`, `test (ubuntu-24.04)`, `test (macos-15)` and
`test (windows-2025)`. Every platform must complete every mandatory setup/build/
test step and both native modes at the reviewed head; skipped jobs and cache hits
are not validation. Source/tree, inventory and warning baselines are audited.
Merged main has independent full validation before local fast-forward/install.
Windows is required.

The repository is public. Standard public GitHub-hosted runner compute is free;
earlier account-billing startup blocks remain historical failures. Main runs
have exercised dependency-cache restore/save. Draft PRs use light checks, and
ready PRs/main/manual runs retain the full matrix. Dependency caches are capped
at 2 GiB per platform, exclude workspace binaries and guest fixtures, and are
saved only by successful main jobs. See [CI controls](ci.md).

On November 16 the active `Protect` ruleset was updated to require all four
matching GitHub Actions contexts and current-branch checks. Its existing
deletion/non-fast-forward protections, target conditions and bypass actors were
preserved. This replaces the obsolete private-repository plan/403 explanation;
no billing or repository visibility setting was changed. The source/tree and
every-step/native-mode audits remain mandatory in addition to status enforcement.

Binary/container publication remains disabled by owner request. Release binary
artifacts and service/workspace image workflows are absent; validation CI and
the JSON-only gadget benchmark remain. Publication needs a reviewed change and
explicit owner approval. MIT licensing does not settle all inherited-text
attribution/provenance questions; see [NOTICE](../NOTICE) and
[OSS review](oss-review.md).

## Current validation record

Earlier installed implementation: [PR #24](https://github.com/kaobei-alabo/craftsman-harness/pull/24),
PR run [43797314382](https://github.com/kaobei-alabo/craftsman-harness/actions/runs/43797314382),
independent main run [43799188076](https://github.com/kaobei-alabo/craftsman-harness/actions/runs/43799188076)
at `9c5774d`. All four jobs, every mandatory step and both modes passed with
matching source/tree and retained test/warning inventories.

| Platform | Core passed | Gadget mode passed | Warnings |
| --- | ---: | ---: | --- |
| macOS 15 | 1,962 | 2,120 | None |
| Ubuntu 24.04 | 1,961 | 2,119 | None |
| Windows 2025 | 1,925 | 2,072 | Unchanged five / eight occurrences |

Local verification passed all 12 checks with 1,962 core / 2,120 gadget-mode tests.
The installed splash passed five native captures at 24/40/80/120 columns with
GPT/Claude fixture configs, clean exit, restored modes and no model requests.
OSC approval/question waits, real edit and saved answer were also verified in
an actual Orca terminal. Config and keys were preserved.

The bounded context/failure continuation passed all 14 local checks at `12f16ae`,
with 2,079 core / 2,237 gadget-mode tests and no warnings. All earlier tests and
warnings were inventoried; 117 ordinary cases and two explicitly marked ignored
helpers were added, with no retired cases or changed goldens. The first original
reserved-command sorting failure and the new completed-recovery expectation
failure are retained separately from the corrected proof. Its
[selected live comparison](live-context-evaluation.md) records original and
clarified inputs without changing the historical 30-task grades. Final hosted
and independent-main validation still follow the same required gates.

The [coding baseline](live-coding-evaluation.md) records 17/30 successful completed
turns and 21/30 behavior-passing workspaces. Selected memory/workflow comparisons
and one [declared-verifier comparison](live-verification-evaluation.md) provide
limited live composition evidence. There is no full 30-task score for the latest
runtime or broad comparison with other coding products. The original allowance accounts for
$24.285934/$25, retaining all 40 unknown-usage reservations; these are conservative
admission amounts, not an invoice.

Failed early fixture/build/CI attempts remain failed evidence in their feature
docs, Git and local records. Current passing runs do not diagnose or erase those
historical failures. Old repository hashes and platform labels are inherited
records, not checks performed against today's source.

## Next engineering gates

1. Extend typed context/cache and broader semantic test/effectful-loop controls
   only with their own source authority and behavior/quality evidence.
2. Analyze retained failures offline, then run a fresh full/repeated evaluation
   across larger projects, long sessions and additional models within an explicit
   budget. Keep historical grades and the same retained accounting intact.
3. Complete public headless response/output/storage/usage/trace, shutdown and guest
   registration boundaries before exposing `exec` or unattended headless gadgets.
4. Validate remaining exceptional recovery/native containment and complete
   attribution/distribution work before any owner-approved publication.

[harness-improvements.md](harness-improvements.md) is the concise completion map.
This working agent remains suitable for supervised use with reviewed changes;
full unattended production/containment and release claims are not established.
