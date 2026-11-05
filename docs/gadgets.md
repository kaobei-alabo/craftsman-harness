# Local gadgets — runtime and authoring

A gadget is a directory containing `gadget.toml` and, for callable functions,
a WebAssembly component named `gadget.wasm`. The authoring SDK and runtime
implement `craftsman:gadget@0.1.0` in `crates/craftsman-gadget/wit/gadget.wit`.

This implements the G1 runtime, G2 declarative surfaces/migration and G3
authoring path. Legacy JSON plugins remain supported for one released
deprecation cycle; their removal is a later breaking change. The current
legacy one-shot entry point is `craftsman -m`. The P2a `craftsman exec` contract was
accepted in #95, but the command is not exposed: permission and filesystem
and private shared-builder construction do not yet provide the full headless
input/output contract or composed source-approved guest-host enforcement. See
[runtime-status.md](runtime-status.md) and `headless-mode.md` sections 11–15.

## Enable the agent surface

The `gadgets` Cargo feature is on in the default build, independently of voice.
For a source build without native voice dependencies:

```sh
cargo build --locked --no-default-features --features gadgets -p craftsman-rs
```

The agent-facing tools and catalog are opt-in. In the user config:

```json
{"gadgets": {"enabled": true, "timeout_secs": 30}}
```

`timeout_secs` must be between 1 and 300. `--no-default-features` without
`--features gadgets` leaves out the Wasmtime host entirely. The SDK itself
builds and tests natively.

## Discovery is deterministic, not a model polling task

Roots are `$CRAFTSMAN_CONFIG_DIR/gadgets/` (normally `~/.craftsman/gadgets/`) and
`./.craftsman/gadgets/`. Project definitions override matching user ids, but
**never inherit the user directory's grant**. Bad bundles are reported rather
than failing the session. A root with more than 64 visible entries is rejected
as a whole instead of picking an arbitrary subset.

Before **every main and delegated model round**, harness middleware refreshes
the catalog using file metadata and content hashes. It detects installation,
rebuilds, manifest/capability changes, approval changes and removal. It appends
one bounded catalog notice only when the fingerprint changes. Unchanged
rounds add no notice. If compaction removed the current notice, the middleware
restores it. Notices are runtime data, not user instructions; the TUI labels
them as runtime metadata instead of showing large JSON as a user message.

The agent gets callable function names, descriptions and JSON schemas
without invoking `gadget_list`. It can use `gadget_call` immediately in the
next round. The **tool definitions and system prefix do not change**. The
catalog is bounded at 64 KiB; unusually large schemas may be omitted, with an
explicit count. `gadget_list` remains available for manual inspection or those
omitted details, not for routine polling.

Interactive discovery asks for first-load approval once per new source/hash/
capability identity. A declined, unchanged gadget does not reopen a modal each
round. Headless discovery never prompts or executes an unapproved component,
including its `describe` export. Successful descriptions and broken-component
errors are cached by hash, so unchanged discovery does not repeatedly compile
or execute a broken gadget. The persistent native-code cache is used only in
private user-owned Unix directories, never project/shared/symlink cache roots.
Windows uses the in-memory cache until a DACL ownership check is added. Calls get a fresh store and instance; an in-flight
call keeps its component if the on-disk bundle changes.

## Declarative commands, agents and prompts

A data-only gadget needs only `gadget.toml`, not a component or compiler:

```toml
id = "review-flow"
version = "0.1.0"
prompt = "Report concrete defects before style nits."

[[commands]]
name = "review"
description = "Review the current diff"
prompt = "Review the diff for correctness bugs. {$ARGS}"

[[agents]]
name = "reviewer"
description = "Read-only code reviewer"
system_prompt = "Inspect code and report defects; do not modify files."
tools = ["read_file", "grep", "glob", "ls"]
```

These surfaces use the same validation and routing contracts as legacy
plugins. Built-in slash commands always win. `{$ARGS}` is substituted once;
without it, trailing arguments are appended after a blank line. An expansion
is **model input only**, even if it starts with `!` or `/`. Agent tool names
resolve against the shared registry; unknown tools and recursive
`task`/`send_message` are filtered out. Gated tools remain behind
`SubagentGate`; declaring an agent never grants file, shell or network access.

Commands refresh deterministically when starting a slash-menu line and at
submission. Prompts and agent definitions retain the legacy session-build
lifecycle (startup or a rebuild such as `/model`); callable WASM functions are
picked up before the next model round without a restart. All gadget surfaces
are behind `gadgets.enabled` in terminal sessions. Frozen/deferred frontends
and the service-agent runtime do not load them. Manifest-only discovery never
compiles, instantiates or calls `describe`, even when a component is present;
its executable functions still require the separate first-load grant.

Discovery bounds are 64 visible bundles and 1024 total directory entries per
root, 256 KiB per manifest, and a 512 KiB budget for effective declarative
surfaces across the user/project roots. Legacy and gadget manifest reads each
have a bounded 512 KiB budget; retained migration copies are coalesced before
the effective-context budget so they do not spend it twice. Large schemas
still use the separate bounded runtime catalog. Treat repository-controlled
gadget prompts like `AGENTS.md`: review them before trusting the checkout.

## Migrate legacy plugins

```sh
craftsman gadgets migrate --dry-run       # preview user-level plugins
craftsman gadgets migrate                 # ~/.craftsman/plugins/ (CRAFTSMAN_CONFIG_DIR)
craftsman gadgets migrate --project       # explicit ./.craftsman/plugins/ scope
```

Migration creates `gadgets/<id>/gadget.toml` for the loader's valid, effective
JSON manifests. It **never deletes originals, overwrites existing gadgets,
changes config, materializes the SDK or manufactures grants**. Identical
existing migrations are reported as `already-migrated`; differing or symlinked
destinations produce structured conflicts and a nonzero exit, without changing
the existing files. `--dry-run` creates no files. Invalid, over-budget and
shadowed legacy files are explicitly reported as skipped. Normal compatible
names become the same gadget id; single-character, digit-leading or oversized
legacy names get a deterministic bounded id, reported in the result.
`[migration]` records the original merge name and file order, preserving
collision precedence even when plugin filenames and names sort differently.

Source precedence is user plugin < user gadget < project plugin < project
gadget. A migrated gadget replaces its retained original instead of duplicating
its prompt, commands or agents. Native gadgets follow migrated gadgets within
a root; migrated gadgets retain the old effective file order. Enable
`gadgets.enabled` to use the migrated surface; with it off, originals keep
working through the deprecated loader. Effective legacy manifests cause one
fixed deprecation warning per session. Removal must wait until a release has
shipped that warning; it is not part of this implementation. No real user
configuration is automatically changed.

See `examples/gadgets/review-flow/gadget.toml`, which is the migrated form of
`examples/plugins/review-flow.json`.

## Author one manually

`craftsman gadgets new <id>` now scaffolds an independent Cargo package with a
lockfile, the pinned stable toolchain and a dependency on the embedded SDK.
`craftsman gadgets build <id>` compiles and atomically publishes it. The layout
below also documents how to create one manually:

```toml
# ~/.craftsman/gadgets/word-count/Cargo.toml
[package]
name = "word-count"
version = "0.1.0"
edition = "2024"

[lib]
crate-type = ["cdylib"]

[workspace]

[dependencies]
craftsman-gadget = { path = "/path/to/craftsman-harness/crates/craftsman-gadget" }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
schemars = "1"
```

```rust
// src/lib.rs
use craftsman_gadget::{gadget_call, GadgetError};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Input { text: String }

#[derive(Serialize)]
struct Count { words: usize }

/// Count whitespace-separated words.
#[gadget_call]
fn count(args: Input) -> Result<Count, GadgetError> {
    Ok(Count { words: args.text.split_whitespace().count() })
}
```

The SDK emits the component exports itself; authors do **not** need a second
`export!()` macro or cargo-component. A function is a synchronous, safe,
non-generic Rust function with zero arguments or one owned
`Deserialize + JsonSchema` argument, returning `Result<T, GadgetError>` where
`T: Serialize`. Doc comments supply its description; `name = "..."` and
`description = "..."` can override it. Duplicate names are rejected.

```toml
# gadget.toml
id = "word-count"
version = "0.1.0"
description = "Small text statistics."
```

```sh
rustup target add wasm32-wasip2
cargo build --locked --release --target wasm32-wasip2
cp target/wasm32-wasip2/release/word_count.wasm gadget.wasm
```

Generate this standalone package's lockfile before its first `--locked` build.
Publish the final component by atomic replacement when rebuilding. Gadget ids
have 2–64 lowercase ASCII letters/digits/hyphens, start with a letter, end with
a letter or digit, and match the directory name.

## Embedded authoring workflow

```sh
craftsman gadgets doctor
craftsman gadgets new word-count
# Edit the generated src/lib.rs and declare only needed capabilities.
craftsman gadgets build word-count
```

`new` is create-only. `--project` explicitly selects `./.craftsman/gadgets/` for
`new` and `build`; a native Cargo build never silently chooses a project
shadow. SDK copies live under the user gadget directory's `.sdk/`, keyed by
CLI version plus embedded-source/toolchain hash. Materialization is resumable,
rejects symlinks and refuses to overwrite modified copies. There is no private
SDK registry requirement. Scaffolds include a lockfile and use the exact stable
Rust pin from this binary.

`build` uses the existing process-tree and scrubbed-environment shell runner.
Cargo dependencies/procedural macros/build scripts are native host operations,
not WASM-sandboxed work. The agent invokes these commands through `execute`,
so shell approval still applies. Static component/WIT validation does **not**
instantiate the guest or call `describe`; first-load capability approval stays
separate. Failed builds leave the previous component unchanged, and successful
publication uses an atomic rename. The harness announces the new callable
functions on the next model round, without list-tool polling.

`doctor` reports a missing pinned toolchain/WASI target and the rustup fix; it
does not install tools itself. When `gadgets.enabled` is on in a terminal
session, the built-in `build-gadget` skill is materialized next to the SDK and
listed for main and delegated agents. Feature-off requests and deferred
frontends do not load it. The skill teaches typed entry points, the host API,
least-capability declarations and the two separate approval boundaries.
See `examples/gadgets/` for pure and workspace-read implementations.

## Capabilities and approval

No filesystem or HTTP access is granted by default. Optional declarations:

```toml
[capabilities]
read = ["workspace"]
write = ["workspace"]
http = ["api.example.com"]
```

Use the SDK's `host::read_file`, `list_dir`, `write_file`, `http_request` and
`log` wrappers. Native host wrappers do not access the machine; native tests
exercise pure functions, registration, schemas and JSON dispatch.

- Paths are scoped to the workspace through capability-relative operations.
  Writes reject symlink parents, protected `.git`, `.craftsman`, `.github`, CI and
  `.env*` paths, Windows device paths and ambiguous path components.
- `write_file` is create-only, matching the existing host tool, and **each write
  still consults the normal permission/approval gate**. A gadget grant is not a
  blanket file-write approval.
- HTTP is HTTPS on port 443 to exact declared lowercase hosts, with no wildcard,
  redirects, environment proxies or routing-header overrides. Requests and
  responses are bounded; attempted requests are counted in harness metadata.
- WASI has no preopens, sockets, DNS, inherited environment, arguments or stdio.
  Stdout/stderr/log output is captured into a shared 64 KiB buffer, never written
  directly to the terminal. CLI diagnostic output escapes control characters.
- Epoch interruption and an outer async deadline stop CPU loops and WASI waits;
  only a human write-approval wait pauses the deadline. A fresh instance has a
  256 MiB memory limit, one memory and bounded tables. JSON input/results are
  limited to 1 MiB; manifests to 256 KiB; components to 32 MiB.

User-only grants live in `config_dir()/gadgets/.grants.json`, atomically updated
under an inter-process file lock, with mode 0600 on Unix. They record source,
id, hash and capabilities. Rebuilds at that same source inherit a grant only
when their capabilities are equal or narrower. Wider capabilities ask again.
First-load "always" approval never becomes global permission bypass.

Wasmtime is pinned to **48.0.8**, not the plan's historical48.0.5/48.0.6 pins.
The reviewed48.x patches include the AArch64/br_table sandbox-escape fix
(GHSA-mghf-wjfp-p5xm, fixed from48.0.7), later component/WASI fixes in48.0.8,
and the earlier host panics during component startup and async-call
cancellation ([GHSA-26c4-8g52-rhj9](https://github.com/bytecodealliance/wasmtime/security/advisories/GHSA-26c4-8g52-rhj9),
[GHSA-3r89-v5mq-j5xf](https://github.com/bytecodealliance/wasmtime/security/advisories/GHSA-3r89-v5mq-j5xf)).
The gadget feature requires at least Wasmtime's Rust 1.95 floor; use the
repository's pinned toolchain. This is an in-process WASM capability boundary,
not an OS sandbox for other processes on the user's machine.

## Inspect or call without a model

CLI subcommands work with `gadgets.enabled` off:

```sh
craftsman gadgets list
craftsman gadgets list --allow-gadget word-count
craftsman gadgets call word-count count --allow-gadget word-count \
  --args '{"text":"one two three"}'
```

`--logs` includes safely escaped captured diagnostics. `--allow-gadget ID` is
an invocation-scoped explicit grant; it is never persisted. A project gadget
also needs `--trust-workspace` in headless mode. `--auto-approve`/bypass mode
alone never approves a gadget's first load.

For the agent, enable `gadgets.enabled` and name headless grants explicitly:

```sh
craftsman -m 'Use the text-statistics gadget' --allow-gadget word-count
```

The TUI's `/gadgets` view shows source, version, status, capabilities and last
error. Frozen/deferred server, desktop, ACP and service-agent frontends do not
load this new runtime.

## Private headless filesystem is not a gadget-host default change

The private `ConfinedFs` backend enforces stricter headless read/write/config
rules, but this runtime still owns its capability filesystem adapter. A future
headless construction path must explicitly apply the same immutable invocation
policy and hard config/credential denies to guest operations; merely compiling
`ConfinedFs` does not do so. Existing TUI gadget behavior, fixed tool definitions,
first-load approval, declarative lifecycles and model opt-in are unchanged. The
headless authoring guide must stay unadvertised when its source-write workflow
cannot satisfy the accepted confinement contract.

Private explicit preparation can now read bounded declarative manifests from its
selected/trusted roots and coalesce migration origins without executing a guest.
The private runtime consumes that data snapshot but deliberately omits callable
gadget tools/host/catalog construction until source-bound grants and strict host
filesystem/invocation policy are integrated. The next private `ConfinedGuestFs`
facet now supplies byte-safe read/create/list over preparation's retained authority,
with declared scopes **and** invocation permission, 10 MiB reads, 1 MiB writes,
1024 total directory candidates and typed fixed-text failures. This is not a
source-bound grant or a `GadgetRuntime`/`HostState` constructor.

A separate private `sources::SourceAuthority` now retains preparation's selected
config/workspace handles and returns owned raw manifest/component bytes with full
current hashes and explicit-versus-user-grant origin. Explicit ids bind to one
source/capability ceiling; stored version-1 grants remain read-only, source-bound
and equal/narrower-only. Root rebinding cannot trigger an ambient reopen. User
opt-in, project trust and invocation denies remain independent; no grant file,
cache or SDK is written, and no compile/instantiate/describe is performed. Existing
stored grants have no manifest hash; the internal record distinguishes that
original approval from the current full manifest hash instead of inventing it.

A separate private `headless::GuestHost` now consumes approved bytes plus their
original filesystem/policy through an opaque lease of the actual runtime's fail
latch. It selects strict byte FS/HTTP/log imports, empty-capability describe,
typed caught-import failure observation, WASI/memory/operation bounds and fixed
diagnostics. It never substitutes legacy RuntimeOptions/ApprovalState, reopens a
source label or writes grants/cache/SDK. JSON schemas cannot retrieve external
HTTP/file/config resources. An app-only source fixture tests malicious describe,
caught errors followed by reads/CPU/WASI sleep and raw byte/resource boundaries;
SDK/WIT and old fixtures remain unchanged.

The normal private builder still does not auto-create/register callable tools/
catalog. The host now selects #104's frozen credential seeds through the SAME
opaque runtime scope for guest JSON and shared stdout/stderr/import logs. Original
bytes enter redaction and delayed literal verification before 64 KiB retention;
UTF-8 is never lossy-replaced. Direct WASIp2 writes are synchronous, independently
permitted and gated again after approval/expiry; flush/handle shutdown does not
release pending credential suffixes. JSON keys/values are redacted before escaping,
then literal scalar/syntax/replacement collisions fail instead of corrupting types.
Execution still consumes original arguments/path/payload; typed failure authority,
counts and original approval/description metadata remain private. This is not a
ToolOutcome/public-schema producer or compatible source-origin publication. Other
provider/tool/storage/trace/output adapters still gate registration and exec.
The scoped shell path redacts original pipes before cuts (§20 / #106). Scoped
filesystem/local-search views now redact complete original source before pagination
and keep original line/query/path authority (§21); fixed gate denials also select seeds.
Scoped web fetch/search now check original bytes before body cuts and parsed fields
before snippet selection (§22 / #108). Concrete built-in todo/task/message results
and todo context now select checked views while original state/delivery stay separate
(§23 / #109). All-role provider request copies now get pre-encoding text/argument/
annotation checks and fixed identity/schema guards, with original control/state/auth
untouched (§24 / #110). Private HTTP/error diagnostics now select frozen UTF-8 views
before retention/400-scalar cuts with actual send/receipt/request witnesses, original
observations separate and fixed unsafe failures (§25). Successful SSE/control, transport,
alternative-state/storage/output adapters remain open. See `headless-mode.md` sections 15–25. These foundations
do not create first-load grants or materialize the SDK; they do not change the
legacy terminal host/grants or enable model visibility. The independent private
input-first adapter (`headless-input.md`) now admits instruction/attachment files
through the same retained policy before session construction; source data/labels
are not gadget grants, stdin is still unsupported, and callable/catalog integration
remains disconnected. The actual runtime's retained-message view
(`retained-message-views.md`) selects only bounded main messages, not raw gadget
origin/approval/custom/child state or a new public ToolOutcome/producer. Its separate
private suffix selector (`message-selection.md`) reduces only admitted messages,
not source approval/provenance or the original outcome, and opens no public/callable
registration gate. The separate private terminal authority
(`terminal-publication.md`) consumes only an actual returned Run and does not grant
or reopen guest/tool execution, publish origin/approval/child state, or prove
all-role/process quiescence. Callable/catalog/public writer gates remain closed. The private
terminal selector (`terminal-message-selection.md`) retains actual output/facts and
omits only admitted whole main messages, not source approvals/provenance/child state;
no guest/tool execution or public writer is reopened. The separate private
ASCII adapter (`terminal-ascii.md`) preserves those source/lease gates and omits only
admitted whole messages after full projection checks; it publishes no gadget origin,
approval, registry/child state or new callable/IO surface. Its separate private
human renderer (`terminal-human.md`) selects only the actual immutable final answer
and fixed private-fact diagnostics, not gadget origin/approvals/child state, and
reopens no callable/catalog/public-writer path. Complete redaction/control/runner
checks retain the SAME lease/gate; real TTY/OS/IO/provenance stay separate.
The independent private file-invocation route (`guarded-output.md`) captures one
outside-workspace/config sink before sessions and publishes only its actual checked
terminal record with bounded create-only native IO. It does not expose guest
origins/approval/child state, grant directory access, enable callables/catalog or
provide public schema/stdio/trace; legacy host/SDK/grants/catalog remain unchanged.
New Windows file fixtures verify real native rename refusal/original directory
identity without changing retained source authority or the existing gadget host.
Private content passes now count seek/payload/EOF inside their unchanged call cap;
fixture-only read observers do not supply gadget usage/provenance or public receipts.
Failed cleanup/early file results recheck the SAME actual publication lease/epoch;
no guest/gadget scope, public counter or native rollback claim is added.
A separate private explicit byte-pipe stdin route (`named-stdin.md`) now precedes
session creation with native mode/deadline/UTF-8 ownership. It never grants guest
stdin/environment/preopens, runs describe or enables callables/catalog; old terminal
gadget discovery, first-load approvals, SDK/WIT and deterministic refresh stay unchanged.
Terminal change-only catalog
refresh before main/delegated rounds and after compaction is unchanged: newly
installed approved gadgets still do not require model-driven list polling.

## Runtime measurement and remaining gates

See [gadget-validation.md](gadget-validation.md) for the release-mode dispatcher/
catalog benchmark, its advisory report, the headless/isolation/live-evaluation
prerequisites before enabling the default, and the published deprecation cycle
required before removing the legacy loader. Runtime timings are not live
model-quality evidence.

## Verification

Cold authoring integration fixtures use an explicit isolated-config 240 s native
build budget, matching the existing model-driven fixture, within a 300 s outer
command bound. They still test first-load/build permission separation, failed-build
component preservation and automatic discovery without list polling. This is not
a cold-compiler performance gate and does not change the production 120 s shell
default, Wasmtime limits or live-quality/runtime measurement gates.

```sh
cargo fmt --all -- --check
cargo check --locked --no-default-features --features gadgets -p craftsman-rs
cargo test --locked --no-default-features --workspace --no-fail-fast
cargo test --locked --no-default-features --features craftsman-rs/gadgets \
  --workspace --no-fail-fast -- --test-threads=4
```

Fixtures are built from Rust source for `wasm32-wasip2`; no binary is checked in.
Private source/facet tests cover retained-root authority, raw-byte hashing,
source-bound provenance, non-widening grants and unsafe/concurrent replacement
without guest execution. Private composed-host tests additionally execute an
approved app-only source-built guest to prove empty-capability describe, same-latch
caught-import failure, strict byte/HTTP/WASI/resource bounds and fixed diagnostics;
these are not model-quality/default-on or redacted publication gates.
Prompt-stop regressions separate bounded cold source/Wasmtime setup from the
strict post-instantiation stop check; production limits are not relaxed to cover
compiler startup, and a failed CI check is fixed/reverified rather than bypassed.
The #110 Windows gadget-suite startup follow-up likewise changes only private
fixture ready/sleep calls: flushed original stdout plus direct .NET file/sleep calls.
Real readiness, original byte counts, six-second partial calls and strict post-readiness
stop assertions stay mandatory; it does not relax host/process limits or bypass a
failed platform job (headless section 24).
Tests cover migration idempotence/conflicts, preserved originals and precedence,
model-only command routing, delegated approval, declarative context bounds,
live installation/rebuild without model list calls, change-only
catalog notices, compaction recovery, fixed request tool definitions,
source-bound approval, capability escalation, workspace/protected/symlink
escapes, ambient environment/files/listening sockets, resource limits and
subsequent successful calls. Feature-off requests are compared both to the
existing goldens and to explicit `gadgets.enabled = false`. CI exercises both
feature configurations without changing its required job names.

### MSRV audit

The pinned-toolchain suites exercise the app. A separate Rust 1.85 probe of
the unchanged P2b baseline already fails on existing `darling`, `home`, ICU,
`idna_adapter` and `instability` versions in its lockfile. The plan's claim that
the entire feature-free app currently builds on 1.85 therefore needs a baseline
dependency audit; it is not a new Wasmtime dependency leak. The SDK and proc
macro have their own 1.85 checks, independently of the application dependency
set. Do not claim that the app's older-toolchain check is green until that audit
is completed.

The first hosted native-stdin revision failed both Windows builds (12 failures
each); its earlier local passes are not cross-platform completion. The separate
native correction preserves real-stdin/deadline/source assertions, distinguishes
Pending from genuine EOF and verifies read/type/synchronous ownership without a
blocking-reader fallback. This document's existing boundary is unchanged; see
[named-stdin.md](named-stdin.md) for the preserved failures and fresh exact-head
Windows/other-platform merge gates. No public frontend/callable/meter is enabled.

A second Windows attempt still failed the five real-stdin cases in both builds.
Current native polling keeps original grants: owned NOWAIT where supported, or
unchanged-mode availability before read-only data/actual EOF probes. No blind
idle/generic/background reader, access upgrade or public OS/exclusive-input
claim is added; all earlier42 stdin assertions and this boundary stay intact.
The additive read-only cases and final-head gates are in `named-stdin.md`.

The separate private [stdin-file-output.md](stdin-file-output.md) composition now
captures original guarded output before stdin, rechecks it after complete admitted
input before sessions/providers, and uses the old paired actual file controller.
This boundary's bodies/codec/authority/lease/source caps remain unchanged: no
arbitrary Run/target pairing, new reader/writer or public/callable/meter gate is
added. Shared-reader/native-IO/OS and final-head all-platform gates remain explicit.


Private instruction-argv follow-up (`input-arguments.md`): source/attachment flags
now route to the same four headless factories, not a gadget/SDK/authoring surface.
`--allow-gadget` and all unimplemented full frontend/output/usage/deadline flags
are REFUSED by this subset before effects, never silently accepted as grants.
Private runtime callables/catalog remain disconnected; original terminal opt-in/
deterministic main-and-delegated catalog refresh and all SDK/WIT/grant/provenance/
first-load/write/live/release rules remain unchanged. Old native readers/writers/
assertions and all four final-head/every-step merge requirements remain intact.


## Guest fixture load follow-up — October25 (Asia/Taipei)

PR #130's first CI40165049917 at ed0c2c9 stayed unmerged: fmt/macOS/Windows
passed but Linux gadget main1441/2failed/6ignored. All58 new argv/old48 native/
old42 composition cases passed BOTH modes/EACH OS; the two unchanged guest tests
observed loading Timeout instead of reaching their denial/post-instantiation
assertions. The exact failure logs/tested tree are preserved, not waived/retried.

The new focused SOURCE input retains actual dynamic binary read/write, original
spin/WASIp2 sleep/entered logs and all malicious describe probes. Only those TWO
old tests choose it; their complete assertions and original30s load/default,
1s post-instantiation and5s outer budgets stay exact. Broad19-function source,
SDK/WIT, production host/compiler/cache/limits and native reader/publisher bodies
remain unchanged. Coordinated source builds happen before guest budgets; actual
Wasmtime load/approval/instantiate/IO still use the SAME existing operation path.
New fixture-only size/LTO settings are not a runtime optimization/approval/cache.
See [guest-fixture-loads.md](guest-fixture-loads.md) for first attempts, advisory
local141598-vs340104 byte/137-vs360ms load observations and truthful limitations.
New exact export/describe-denial/dynamic256-byte roundtrip checks do not invent
status/bytes/zero counts or skip failure; only the NEW sorted-export expectation
was corrected after its first failure. Fresh bounded verification/ALL FOUR exact-
final-head jobs/EVERY mandatory step still gate merge, Windows included. Public
exec/wholeflags/stdin/stdio/R2/results/events/storage/trace/provenance/gadgets/
usage/#97/P2g/live/release remain separate/off.


## Private control/source argv follow-up — October26 (Asia/Taipei)

The new [control-arguments.md](control-arguments.md) factories now combine ONLY
supported absolute workspace/config, trust, manual/plan/deny/fail, explicit
allow/deny/env-pass and mutually exclusive fresh/ephemeral/resume/continue controls
with the old original source/attachment argv grammar. Whole native bounds/strict
UTF8/syntax/old provisional selector and session-ID/scope admission precede IO.
Unsupported relative/ambiguous scope, output/trace/request/token/wall/gadget/full
flags refuse, never accept-and-ignore or guess ambient directories.

Explicit caller Defaults are raw absolute inputs, not physical authority. Old
Bootstrap remains authoritative for config/credential-source env denies, retained
ConfinedFs, selected sessions and actual all-role policy/gate; the preliminary
syntax policy is dropped. Original Options/source arguments are captured into old
ArgumentInvocation/ArgumentFileInvocation, returning existing Run/FileRun without
new caller Runtime/Run/target/data/seeds/gates/record/IO constructors or post-run
format/path switching. EVERY old body/assertion/reader/writer/codec/lease/limit/
fixture/SDK/WIT/framework/dependency/lock/pin/frontend/frame/schema/golden/trace
stays exact after narrow declarations/grammar-bound aliases. The initial58 normal
cases plus one argv-only ignored helper and locked production check passed on
source-local builds; no user config/SDK/global env mutation or raw history reset.
No public exec/full flags/stdin/stdio/R2/results/events/storage/trace/provenance/
headless gadgets/usage/#97/P2g/live/release enablement. Fresh all-four exact-head/
every-mandatory-step/both-mode/Windows evidence is a separate merge requirement.


## Private relative-scope/output-file argv follow-up — October29 (Asia/Taipei)

The separate [scope-output-arguments.md](scope-output-arguments.md) adapter resolves
ONLY bounded normal-relative cwd/config scopes against ORIGINAL explicit launch
Defaults, independent of option order. Task/output paths use the SELECTED workspace;
no ambient cwd/HOME/env/chdir, canonicalization or dot/parent/drive/UNC/device repair.
The old absolute control adapter remains unchanged and still refuses relative input.

One original `--output-file` becomes a typed pre-run Destination through SAME old
ControlFileInvocation. Physical workspace/config output still refuses, including
relative output; use an existing absolute external directory for this private
publisher. FileOptions selects only old private ASCII/human previews, NOT public
text/JSON/JSONL. Public format/trace/usage/gadget/wall/legacy flags refuse before
effects, never accept-and-ignore or rename preview data into public schema fields.
Source/attachment values and after-`--` tokens remain original task data.

Whole native/default bounds/UTF8 and complete new+old syntax precede effects.
Provisional old policy/destination selections are DROPPED; actual Bootstrap alone
revalidates configured credential sources and captures original auth/ConfinedFs/
source roots/session selection. Existing Run/FileRun, native rights/read clocks,
record/lease/epoch/create-only single-attempt publisher and OS caveats stay exact.
Only two narrow old input-module/preflight declarations are appended; ALL older
Rust bodies/tests/fixtures/hosts/limits and framework/SDK/WIT/deps/lock/pin/frontend/
SafeBackend/frames/schemas/goldens/trace/corpus remain unchanged.

The first locked production check passed warning-free. The first NEW fixture
compile recorded an accessor typo/unused NEW import, corrected only in new tests
with actual source-before-store coverage; its log is preserved, not called green.
Isolated grammar/runtime/native fixtures retain original input/request history,
without real user config/SDK/global env mutation. Unmarked/unknown helper scenarios
return before fixture/config/stdin work. Current-source bounded checks/links and
ALL FOUR fresh exact-head jobs/EVERY mandatory step/BOTH modes/Windows independently
gate merge; no old assertions/deadlines/process workaround or unchanged full-CI retry.
Public exec/full parent/ambiguous paths/stdin/stdio/R2/results/events/storage/trace/
provenance/headless gadgets/shared usage/#97/P2g/live/default-on/release stay open/off.


Collected current-source evidence: all **34 bounded local rows** passed warning-free,
including new60 scope/output cases and unchanged58 controls/58 source argv/48 native/
42 composition cases in BOTH modes, old37 guest cases locally (35 compiled on Windows),
every selected older suite, both locked production checks, actual marked/unmarked
existing compile probes, fixture/root fmt/reference126/driver9/whitespace/lineage.
The exact17-path source audit keeps EVERY older Rust body/assertion unchanged after
TWO narrow append-only module/preflight declarations; all91 local Markdown links and
fences pass. The first NEW accessor/import compile failure remains preserved, not
passing evidence; corrected60 and final34 source rows are independent. This is not
fresh all-platform CI, public exec/full path/format/R2/usage/live/OS/release approval.
ALL FOUR fresh exact-final-head jobs/EVERY mandatory step/BOTH modes/Windows remain
independent prerequisites before a missing-review-only admin merge.


## Shared-meter contract prerequisite — accepted November1 (Asia/Taipei)

[token-admission.md](token-admission.md) records explicit user acceptance of #97's
optional metadata/strict refusal/v1 compatibility judgment: no invented main/child/
compaction usage or calls. Older semantic validators must update and optional-name
collision risk remains acknowledged. The contract decision is resolved, not inferred
from CI; shared meter/runtime/public producer implementation remains unwired.
This is not a gadget grant/host/catalog/provenance change. Existing guest fixtures/
SDK/WIT/hosts/limits, deterministic per-round catalog refresh, opt-in and all public/
live/default-on/release gates remain unchanged.


## Private shared logical-request admission — November1 (Asia/Taipei)

[request-admission.md](request-admission.md) records the next bounded R5 slice after
#133's accepted-contract merge/#97 decision closure. One actual-builder source and
opaque ORIGINAL-provider/concrete-role bindings enforce1..10000/default50 calls
across main/subagent/compaction before supplied-inner entry. Same-gate cap refusal
closes execution deny-only, never fake NeedsApproval; true approval dominates the
private runtime before/after classification. Active/actual inner-return/drop facts
never refund admitted calls, and foreign/equal sources and poisoned state refuse.

Forty additive tests (27 owned-builder synthetic +13 actual input/loopback/runtime)
cover all roles, concurrent cap/last-response boundaries, failures/drop/refusal,
source identity, real approval and later same-response tool denial. Original request
histories and partial state/registry stay private, not reset or promoted into usage.
This is NOT token allowance/clamping/usage classification/HTTP receipt/unknown billing,
public flags/results/events/trace/storage/provenance/callables/catalog/P2g/live/release.
An admitted wrapper can fail before traffic; public token admission must combine
both reservations BEFORE the charge point, not refund/hide a refused call later.

Docs/source/tests accompany the incomplete implementation. Old tests/assertions/
readers/writers/native/guest/host limits, SDK/WIT/framework/deps/lock/pin/frontends/
SafeBackend/schemas/frames/goldens/trace/corpus remain unchanged. Initial new adapter
warning/scope/privacy/input-constructor setup failures are retained; fixes do not
suppress warnings, broaden production visibility or weaken old checks/deadlines.
ALL FOUR fresh exact-head jobs/EVERY mandatory step/BOTH modes/Windows and source/
warning/review proof gate merge, not the older #133 contract CI. No independently
blocked provider correction is inspected, executed, changed, retried or worked around.


## Private atomic token reservations — November2 (Asia/Taipei)

[token-reservations.md](token-reservations.md) records a private R5 opt-in foundation.
The SAME builder binds concrete scoped clients/roles to one meter; bounded whole
checked-input estimates and final reply-body checks precede ONE request/token
allocation decision. Default request-only paths remain unchanged; only a separate
typed private factory opts in, with no public flags/CLI. Allocations stay active or
retained unsettled, never inferred measured usage, refunded calls or #97 public
observed/remaining facts. Fifty additive tests cover27 actual-builder/source/client/
concurrency cases,12 real input/runtime/child/compaction cases and11 arithmetic/
estimator cases. Real histories stay intact; initial NEW `--lib` driver failure is
preserved, corrected only by selecting the binary test target.

Receipt/partial-output witnesses, usage-presence/fallback, concurrent settlement/
public ledger and all other producer/P2g/gadget/live/release gates remain separate.
All old assertions, source codecs/response control/native limits/framework/SDK/WIT/
deps/lock/pin/frontends/protocols/schemas/goldens/trace/history remain protected;
additive tests/docs and ALL FOUR fresh exact-head CI jobs/every mandatory step/both
modes/Windows plus source/warning/review proof precede any missing-review-only merge.


### CI image correction — November2 (Asia/Taipei)

The first #135 head's macOS job was cancelled in four seconds BEFORE runner
assignment/checkout/any test. GitHub's official runner-images#13518 and check
annotation identify the macOS14 retirement brownout (November1 14:00–November2
00:00 UTC); full retirement is November2, 2026. That cancellation is preserved,
not a source-test failure or green evidence, and is DISTINCT from the unchanged
local partial-shell readiness failure above.

`ci.yml` now separates stable check ids from actual images: required
`test (macos-14)` retains its exact context name but runs on explicit `macos-15`.
Ubuntu24.04/Windows2025 names/images and EVERY action/test/build/limit/condition
remain unchanged; no ruleset/branch protection mutation, macos-latest alias,
old assertion relaxation or unchanged full-CI retry. Only real image selection
and current docs change; every Rust byte from the50-case implementation/local
rows is identical. Fresh corrected-head ALL FOUR jobs/every mandatory/setup step/
both modes/Windows plus ACTUAL macOS15 labels/raw logs/source/warning/review proof
are required. Do not call the legacy name macOS14 test coverage or reuse the
cancelled original head's status as coverage of the corrected workflow.


## Private admission-correlated send/header observations — November2 (Asia/Taipei)

[admission-receipts.md](admission-receipts.md) records the next private R5 slice after
#135. Only actual token-admitted body/permit/checked-client identity mints the scope;
SAME builder/meter/gate and concrete main/subagent/compaction keys bind bounded
<=10000 calls/two slots. Client-only witnesses record ACTUAL first send-future poll,
awaited send error or returned HTTP headers; preparation/no-poll, abandonment and
late stop retain truthful distinct facts. No socket/wire/billing/body completion/
usage/settlement inference, extra retry request charge, fake status/bytes or refund.
Default request-only/frontends/flags remain unchanged; only actual Run selects
non-Debug/non-Serialize historical source-bound observations. Forty additive tests
(10 builder/identity,20 actual runtime/loopback,10 coherence/bounds) and docs accompany
the incomplete adapter. Initial5 NEW unused test interfaces are preserved/corrected
only by completing new actual-owner coverage; no suppression/old assertions changed.

Original successful SSE/control/usage/retry/diagnostics, source codecs, native/guest/
input/writer/host limits, framework/SDK/WIT/deps/lock/pin/frontends/SafeBackend/v4/ACP/
schemas/vectors/goldens/trace/history stay unchanged. No independently blocked provider
inspection/execution/workaround. Actual bounded successful response/usage presence/
partial-output, billing/settlement/overrun/public producers/provenance/P2g remain
separate; exec/callable headless gadgets/catalog/live/default-on/release stay OFF.
ALL FOUR fresh exact-head CI jobs/every mandatory/setup step/both modes/Windows plus
actual macOS15 label/source-warning-tree-review proof precede missing-review-only merge.


### Required engine security patch — November2 (Asia/Taipei)

During this slice GitHub surfaced two critical manifest/lock alerts for the SAME
Wasmtime AArch64/large-br_table advisory GHSA-mghf-wjfp-p5xm. The advisory/48.0.7 fix
were published October19, 2026; our repository alerts appeared November1 at20:06UTC.
This is newly surfaced evidence, not a new November2 advisory or proof that earlier
48.0.6 was safe. Vendor48.0.8 (October29) additionally fixes component/WASI issues;
both direct host pins now use exact48.0.8. Rust stays1.99.1, SDK/WIT/guest-host policy/
capabilities/production limits/defaults/assertions and CI/check names remain unchanged.

A separate necessary dependency commit updates ONLY the two host pins and30 existing
Wasmtime/Pulley/Cranelift version/checksum entries; no dependency edges/features/
registry sources, unrelated libraries, standalone gadget SDK sources/lockfiles or
Rust toolchain changes are bundled.
Three additive offline guards pin the reviewed matching runtime/WASI versions/family
and original feature/model-opt-in shape; no exploit or unpatched guest is executed
by these guards. The local machine is nativeARM64. Fresh patched-head locked checks,
all43 NEW receipt/security tests, protected native/guest/SDK/WIT suites and ALL FOUR
CI jobs/every mandatory/setup step/both modes/Windows/actual macOS15 evidence gate merge.

All15 earlier bounded local rows passed warning-free BEFORE this prerequisite; they
are retained as pre-security evidence, NOT relabeled patched-head coverage. No whole
unchanged retry/empty commit/assertion/deadline weakening; only the real dependency
security update justifies fresh source validation. Other open advisories, public
exec/usage settlement, full containment/P2g/live/default-on/release remain separate.


### November 3: existing Windows shell partial-output fixture correction

[windows-shell-fixtures.md](windows-shell-fixtures.md) preserves main
run41398538418's failed Windows feature-off partial-output case separately from
the green exact-head PR and later passing gadget-host/authoring/discovery suites.
Only the Windows test command uses direct flushed Console stdout/stderr plus a
long blocking sleep; old 2s assertions and production shell/job/capture behavior
stay unchanged. Three additive native Windows cases protect two-format actual
33-byte Timeout observations and deliberate pre-write zero-byte outcomes, never
fake partial output or a retry/warmup. All FOUR fresh jobs/every mandatory step/
both modes, not the informational badge, remain merge evidence.

The private6s readiness fixture, SDK/WIT, grants, guest host/limits and deterministic
change-only catalog remain unchanged. Approved installed/rebuilt gadgets are still
announced before the next main/delegated round without model list polling; this
does not enable headless callable registration, live/default-on or release gates.


### Test-only native shell observations for #138 — November 3

[shell-test-observations.md](shell-test-observations.md) records fixed bounded
launch/read metadata from actual scoped Shell owners only in cfg(test). Original
fixture failures can reveal startup versus read timing without dumping raw
commands/paths/auth/output or changing20s/15s/10s assertions. This is not a root
cause or production/gadget trace. Preserve failed whole-run evidence separately
from focused/instrumented passes. SDK/WIT/host grants/guest limits, production
shell/process sources and deterministic change-only catalog refresh are unchanged;
no callable headless gadget, live/default-on or release gate is enabled.


### Native failure snapshot refinement for #138

The first instrumented full runs still failed:1554/2/7 off and1697/2/9 gadgets.
Actual launch return/first read polls were under3ms with zero received bytes;
this is not a root-cause fix. The live children were gone when those logs were
inspected, so no post-hoc lookup of possibly reused PIDs is performed.
A cfg(test)/macOS-only panic-teardown query now records fixed BSD image class,
state and flags ONLY for an active actual PID after exact struct/PID/current
parent/group validation. It retains no raw process name/path/argv/environment,
starts no sampling thread and sends no signal. The bounded native-call count is
not an absolute OS-latency/PID-reuse/exec/quiescence guarantee. New source evidence
and all4-job/every-step/both-mode gates remain separate from old failed runs;
all original assertions/deadlines and production behavior are unchanged.


### Native task/thread failure refinement for #138 (November 3)

PR139 merged only the diagnostic slice after exact-head all-four-job/every-step/
both-mode/source/warning/tree audits; #138 is still OPEN. Its refined local full
off run still failed1554/2/7, independently of hosted green evidence. No root
cause or exec/usage/settlement/public-output/P2g/live/release approval follows.
The next cfg(test)/macOS-only failure snapshot adds original kernel task CPU/
thread/running/context-switch/syscall/fault counters and bounded per-thread
run-state/native-time facts. It uses one complete TASKALLINFO snapshot, at most8
native thread entries and a final original PID/start/current-parent/group
revalidation; incomplete, overbound, duplicate or changed subqueries are fixed
unavailable/refusal diagnostics, never a clipped complete set. At most11 native
calls per active entry, with no retry/sampler/signal/sleep/native IO under the
recorder lock or raw name/pthread-address/path/argv/environment rendering.
Native counters are not wall-time/rate/CPU-spin/wait-channel/exec/progress/usage
proof; calls are bounded, not absolutely interruptible OS-latency guarantees.
All old runnable code/reader/process semantics/assertions/deadlines stay exact.
New native metadata and actual-builder cases are separate from full evidence;
full validation of genuinely changed source is not an unchanged broad retry.
See [shell-test-observations.md](shell-test-observations.md) for exact limits and
remaining diagnosis; every partial slice keeps current documentation in sync.


### Native executable/CWD observation refinement for #138 (November 3)

PR140's independently audited merged-main run41631110649 passed all FOUR jobs/
every setup/mandatory/post step/BOTH modes; its failed full local1562/1/7 off
and1704/2/9 gadgets remain failed and #138 is OPEN. The next cfg(test)/macOS-only
snapshot distinguishes a process display-name class from the actual native
executable path class and compares native CWD device/inode to the SAME retained
ConfinedFs workspace captured from the actual ShellScope. Only exact literal
image classes and fixed match/different/unavailable relations are retained or
rendered, never paths, names, argv/environment or device/inode values. A native
path has one initialized4096-byte buffer/exact strlen+NUL validation; CWD uses a
complete exact native vnode struct, ignores embedded paths and checks directory
mode/nonzero identity. No alias normalization, path reopen, metadata as authority
or root rebinding to manufacture a match. Final ORIGINAL PID/start/parent/group
revalidation gates ALL new execution facts and existing thread facts; reads are
not atomic/exec/containment/wait-cause/usage proofs. Maximum13 native queries per
active entry, plus one clone/metadata/close of the retained directory at TEST-ONLY
Shell construction; no sampler/signal/sleep/retry/clock/launcher/default change.
Seven macOS additions include real same-workspace and OWNED namespace-rebind
cases; every older body/assertion/deadline/production source stays exact. Earlier
failed full evidence is never replaced by focused/hosted passes or an unchanged
broad retry. See [shell-test-observations.md](shell-test-observations.md); docs and
all-four exact-head job/step/mode/source/warning/tree gates accompany this slice.


### Bounded pre-kill native snapshot refinement for #138 (November 3 UTC)

PR141 merged-main41647609134 independently passed all FOUR jobs/every setup/
mandatory/post step/BOTH modes with exact source/tree/runner/case/warning proof.
Its FAILED local1566/4/7 off and1709/4/9 gadget results, INCLUDING new integration
readiness failures, remain failed and #138 OPEN. A new cfgtest/macOS-only hook
records the actual original stop decision/current owned tree ID/observed pipe
facts BEFORE ORIGINAL kill/reap, so later invalid-byte Timeout assertions can
report saved fixed native data without querying old/reused PIDs. One non-clone
query ticket per retained call is shared with active failure views; reserve under
lock, query outside, cap4096 safe ASCII bytes/13 native calls, never clip/echo raw
text or refund/rebind an evicted ticket. Prior active data stays labelled active-
view, not fresh before-stop; clocks explicitly distinguish active-query vs call.
No stop/pid mismatch/released call authorizes a saved-PID query. Existing report/
failure_report remain path-free and hide released native facts; ONLY original
fixture-panic teardown/new explicit TEST view includes the bounded saved history.
No sampler/signal/sleep/retry/timer/launcher/production/default/old assertion/
deadline/reader/process change. Twelve macOS additions (eight pure/four actual
builder) test one-shot identity/caps/eviction/before-drain facts and real timeout/
approval/prior-view/normal-return behavior, NOT root cause/exec/quiescence/usage
or replacement full coverage. See [shell-test-observations.md](shell-test-observations.md);
docs plus all-four exact-head job/step/mode/source/warning/tree gates accompany
this incomplete slice. Public/blocked-provider/usage/live/release gates stay off.


## Windows Job/PID fixture follow-up — November4,2026 Asia/Taipei

PR142's separately audited merged-main41668514031 is NOT all-four green:
Windows feature-off1532/2/7 failed the ORIGINAL2s partial-output and3s Job/PID
cases; gadgets1666/0/9 passed, but the interpreter post step was skipped.
All other jobs/inventories and original Windows9 warning signatures/17 occurrences
remain verified. Preserve that failure separately from green PR142 CI, never
substitute the later gadget pass or retry unchanged CI to manufacture green.

Only the old Windows Job/PID fixture input now removes Start-Process/Write-Output:
construct one actual System.Diagnostics.Process, keep UseShellExecute false and
child streams redirected to that Process, Start without implicit Boolean output,
flush its ORIGINAL positive PID through Console, then actual WaitForExit. The
SAME existing PowerShell/ProcessTree/Job Object path and original3s timeout/30s
outer/2s drain/PID/grandchild assertions stay exact. Child creation/output is not
mocked, no startup readiness/bytes/PID/completion is fabricated. Two additive
native Windows cases check exact input order/no cmdlets and actual PID/raw bytes/
typed Timeout/no exit/real duration/grandchild kill under the SAME3s bound.

The ordinary2s PRINT_THEN_HANG/direct Console fixture and ALL its older/new test
bodies remain byte-exact. Its missing-output cause is NOT established or fixed
by this narrower PID-input implementation; no universal cold-start latency is
promised. Production runner/capture/process/defaults/clocks/timers, framework/
SDK/WIT/provider/input/guest/permissions/publishers/frontends/deps/lock/pin/
workflow/frames/schemas/goldens/trace/corpus stay exact. Current implementation
is incomplete pending native Windows verification; local macOS checks with zero
selected Windows cases are NOT Windows execution evidence. Documentation must
accompany every partial slice; historical plan/Appendix A/R1–R7/#97 stay intact.
ALL FOUR fresh exact-head jobs/every setup/mandatory/post step/both modes/actual
macOS15+Windows/old+2-new-inventory/warning/source/tree proof precede review-only
merge. No warmup/unchanged retry/deadline/assertion/launcher workaround, real model
call/user config/env/SDK mutation or blocked-provider work. #138 stays OPEN;
public exec/usage/settlement/callables/P2g/live/default-on/release remain off.


## Read-only native scheduling facts — November4,2026 UTC

PR143/main41683501397 is independently all-four/every-step/both-mode/source/
old+new-inventory/warning/tree verified. Earlier Windows main41668514031 remains
FAILED; prior local macOS1578/4/7 off and1721/4/9 gadgets remain FAILED/#138 OPEN.
Retained pre-kill flags20987920 are not, alone, a wait-cause or startup witness.
A conservative PUBLIC flag projection and original nice/task/thread priority/
policy fields are now copied from ALREADY complete native query buffers.

This is cfgtest/macOS-only, ZERO additional native queries/handles/root reads:
unchanged13-query/8-thread/call/view bounds and original final PID/start/current-
parent/group revalidation gate ALL new process/thread scheduler details. Process
facts are labelled initial-task-info, not atomic last state. Resource POLICY is
one masked enum (0x600=terminate, never two policies); unknown ACTION combinations
and all non-public/unclassified bits remain numeric, never guessed private flag
names or rewritten as none. EXEC/TRACED/PPWAIT/etc are exported bit predicates,
not current executable/script entry/tracer/whole-process wait/permission/usage
proof. Native signed priorities/policies are preserved, not normalized into
high/low/starved/suppressed or treated as a portable scheduling explanation.

The SAME one non-clone active/pre-kill ticket, original phase/clock/pipe facts,
released-native hiding and cached teardown semantics remain intact. Whole fixed
ASCII/LF reports still fit4096 bytes or WHOLE fixed refusal, never clipping/raising
caps to rescue extra fields. No sampler/signal/sleep/poll/clock/timer/launcher/
production/capture/process/default/permission/config/storage/frame change. Eight
additive pure cases plus one real actual-builder1s zero-byte timeout (NOT startup/
partial proof) protect projection, masked enum/unknown bits/native signs/no raw
name/address, final revalidation and saved history without another PID lookup.
All old15s invalid-byte/20s readiness/10s stop/6s partial/Windows2s+3s/30s outer/
2s drain/source/test/assertion bodies remain exact. Docs with incomplete work;
historical plan/Appendix A/R1–R7/#97 and SDK/WIT/framework/deps/lock/pin/workflow/
frontends/schemas/goldens/trace/corpus stay exact. Native and ALL FOUR exact-head
job/every-step/both-mode/actual macOS15+Windows/old+9-new-inventory/warning/source/
tree verification remains required before missing-formal-review-only merge.
No unchanged broad retry/warmup, fake PID/bytes/readiness/usage, stale PID lookup,
real model/gadget/user config/env/SDK mutation, agents/goals/blocked-provider work
or public exec/usage/settlement/callables/P2g/live/default-on/release enablement.


## Current test-only shell poll/selection follow-up — November 4, 2026 UTC

After PR145's independently verified rebrand, unresolved #138 receives a
cfg(test)-only original-reader-poll/selection-phase follow-up. It preserves exact
inner wakers/buffers/results, all old assertions/deadlines/production behavior,
the native13-query/8-thread/ticket/phase/clock/cap bounds and every failed full
result. New progress uses one whole4096-byte ASCII/LF section or whole refusal,
with before-stop history separate from later drain; no new poll/native lookup or
readiness/wait-cause/usage claim. See [shell-poll-progress.md](shell-poll-progress.md)
for the partial implementation and required all-four/every-step/both-mode native
verification. Framework/SDK/WIT/deps/lock/pin/frontends/goldens/schemas/trace/history
remain exact; public exec/usage/settlement/headless callables/catalog/P2g/live/
default-on/release and blocked-provider work stay separate/off.


November 4 changed-source full validation remains mixed: off FAILED1600/4/7
(including the NEW normal-output diagnostic), gadgets1747/0/9 passed. All old
inventories plus13 new cases are present; no compiler warnings/unchanged retry.
ONE original Pending poll per pipe/no wait-selection entry in the off failure
reports is parent-side phase evidence, NOT a root fix or lost-wakeup explanation.
The gadget pass never replaces the off/prior failures; #138 stays OPEN and all
public/usage/settlement/callable/P2g/live/release gates remain separate/off. See
[the diagnostic validation](shell-poll-progress.md).


## Corrective environment-test isolation — November 4, 2026 UTC

After verified PR146, [test-environment-isolation.md](test-environment-isolation.md)
records a concrete correction to unsafe parallel test-global env mutation.
Original config/MCP/route assertions use immutable marked child env plans; the
same concurrent serve HTTP/WS auth assertions use a per-router cfgtest source
with the original real comparison. Production env/auth/ProcessTree/reader/
deadline/SDK/framework/permission/schema/frame/golden/trace behavior is unchanged.
This isolation bug is real but not yet the proved cause/fix of #138; every old
and new failed result remains FAILED. Fresh new-source/all-four exact-head/
every-step/both-mode/native/inventory/warning/source/tree verification remains
required. No user/parent env/SDK/model/gadget/release/default-on/P2g/usage/
settlement/public exec or independently blocked provider work is enabled.


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

The next [test-only legacy phase observer](legacy-shell-observations.md) records original spawn return,
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

The next [read-only pipe facet](native-pipe-snapshots.md) queries ONLY the existing live actual
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
See [the complete pipe diagnostic validation](native-pipe-snapshots.md).


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
work. See [the owned-stack scope and validation](owned-child-stack-capture.md).


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
[the complete owned-stack evidence](owned-child-stack-capture.md).
