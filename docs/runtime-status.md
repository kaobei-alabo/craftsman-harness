# Craftsman runtime — current implementation and remaining work

> **Updated:** November 5, 2026 (Asia/Taipei). This page describes the maintained checkout,
> including private implementation foundations. It is not a release announcement.
> **Design/evidence:** `craftsman-harness-plan.md` retains the October 16 baseline,
> target architecture and historical measurements. Its baseline “today” boxes
> are not this page's current runtime. `headless-mode.md` is the accepted v1
> specification and records partial implementation separately from CLI availability.
> **Current acceptance:** #97's token-admission representation/v1 compatibility was
> explicitly approved November1 (`token-admission.md`), not inferred from CI. Earlier
> dated slice evidence describing that then-open gate is historical. A private
> shared request-only cap is selected by default (`request-admission.md`); separate
> private bounded token reservations are implemented (`token-reservations.md`).
> Private admission-correlated send/header observations now exist (`admission-receipts.md`);
> usage/settlement/public producers remain unwired.

> **New ownership and implementation:** `yumingchangsabodota/craftsman-harness`
> starts with a fresh source-only Git history. The new private
> [request-budget arguments](request-budget-arguments.md) select `--max-rounds`
> before preparation and use the shared main/delegate/compaction meter. Default
> 50, range 1..10000; existing routes and public exec availability are unchanged.
> The app declares Rust 1.99 with the existing 1.99.1 pin. The new CI requires all
> three platforms, uses actual macOS 15 labels and checks the default build.
> [Production readiness](production-readiness.md) records current validation and
> the working self-built gadget path; older check/issue records below are historical.

> **Terminal prompt cancellation (November 6):** Ctrl-C now requests a stop from
> approval/question dialogs and other modals. The actual agent receives the
> cooperative stop before synchronous waiters receive denial/absence; queued and
> late prompts cannot keep that stopping turn waiting for input. Unrelated drafts
> survive and next-turn prompts work normally. This does not abort a streaming
> provider, running native tool or foreground child, or complete signal shutdown.
> See [terminal cancellation](terminal-cancellation.md) for implementation and tests.

> **Desktop reconnect correction (November 6):** the required Ubuntu gadget suite
> exposed the original idle reconnect assertion on PR #2. An actual connection
> owner now stays held through old-reader cancellation/detach cleanup, with one
> bounded handoff wait and an explicit Close/writer-stop path. A live peer remains
> exclusive and original turn context persists until real completion. This changes
> lifecycle ordering, with no protocol-v4/schema/golden/native-limit change.
> [Desktop connections](desktop-connections.md) keeps the failing and corrected
> validation records separate; all four corrected-head checks still gate merge.

> **Current shell maintenance:** the November2 post-merge main run `41398538418`
> failed one feature-off Windows partial-output test, despite an informational
> overall success and a later green gadget-host step. A narrow Windows fixture
> correction now uses direct flushed Console writes before blocking, with three
> additive native cases and unchanged old 2s assertions/limits. All four jobs AND
> every mandatory step/both modes remain the gate; no generic badge/PR-pass
> substitution. See `windows-shell-fixtures.md` for the distinct original evidence.

## Complete identity migration — November 4, 2026

The November 4 canonical repository was `trend-mgai/craftsman-harness`. The source rebrand
also changes CLI/crate/config/environment/ACP/WIT/schema/image and file names
to Craftsman. This is a breaking identity migration, not merely a repository
folder change. Existing user data, Git history, grants, releases and sealed
evidence are not mutated; old failure outcomes remain failures. Historical
names in docs are translated labels, not new evidence or producer claims.
See [repository-name.md](repository-name.md) for the interface table, explicit
configuration handling and external source/secret/publication prerequisites.
The identity migration merged as PR145 and is independently verified on main
CI41808669282 (November 4, UTC): all four jobs/every setup, mandatory and post step,
both modes, exact old/new inventories, warning locations and merged tree. This
completes source migration validation, not the unfinished public-exec/callable-
gadget/accounting/live/release gates below or external publishing prerequisites.

The first renamed local full invocations are retained as failures (1585/6/7
feature-off;1728/6/9 gadgets), including five native shell cases and one
renamed-token collation expectation. Production capture was unchanged; only
that exact two-name expected order was corrected and passes both focused
modes. No unchanged broad retry or root-cause claim. Corrected-head CI41803187695
and independent merged-main CI41808669282 are fully audited, with actual macOS15
behind the retained context and unchanged Windows9 warning signatures/17
occurrences. The migration note records the completed validation checkpoint.

## 1. What users can run now

| Surface | Current behavior | Important limit |
|---|---|---|
| `craftsman` TUI | Streaming conversation, steering/queue, cooperative cancellation from dialogs, HITL, host/web tools, local sessions/memory, lazy LSP, skills, opt-in gadgets | Host filesystem and native shell are not a Craftsman OS sandbox; in-flight work is not immediately interrupted |
| `craftsman -m PROMPT` | Legacy argument-only one-shot; main content streams to stdout; activity goes to stderr | Still inherits legacy user-config permissions/storage; not the v1 unattended contract |
| `craftsman gadgets …` | Inspect/call, create/build/doctor, create-only legacy migration | Build/native-code permission is separate from first guest-load permission |
| `serve`, `desktop-serve`, `acp-serve`, `acp-bridge` | Existing transports/adapters remain supported | Desktop v4 is frozen; ACP stays standard with declared Craftsman extensions; no new headless/gadget opt-in is inferred |
| `service-agent` | Request-scoped virtual resources and explicitly selected public-web tools | No real filesystem, native shell, local DB/memory/LSP/project discovery |
| `craftsman exec` | **Not exposed** | Accepted contract plus partial private implementation, not a runnable command |

Source implementation is not publication: gadget model visibility remains user
opt-in, and merging migration code does not complete a published plugin-deprecation
cycle. `gadget-validation.md` records the independent runtime,
live-evaluation and release gates.

## 2. Current architecture

```text
CLI / TUI / existing transport adapters
  |
  +-- local paths: agent::build_session_ext
  |     route + model/provider profiles + prompts + extensions
  |     main, delegated and compaction model providers
  |     middleware + host/web/MCP/LSP tools
  |       |
  |       +-- craftsman-harness::Agent
  |       |     six middleware stages, model/tool loop, task registry,
  |       |     notifications, state and persistence interfaces
  |       |     no app HTTP/database/UI dependencies
  |       |
  |       +-- app resources
  |             RealFs + optional /memories StoreFs
  |             ShellRunner -> platform_os::ProcessTree
  |             local libsql threads/checkpoints + JSONL journal
  |             optional gadget host + deterministic catalog middleware
  |
  +-- service-agent: separate restricted construction, virtual backend
  |
  +-- private headless foundations (not a CLI route)
        exec::permissions: immutable explicit invocation policy
        exec::confined_fs: bounded handle-relative ORIGINAL IO authority
          -> actual builder FilesystemScope -> TextFs backend + bound filesystem host tools
             complete original-byte coverage before page/hit selection; original lines/queries/mutations
             safe name labels (never authority aliases), typed FsError observation, no raw spill
        accepted schemas/reference + native conformance tests
          + accepted #97 detail; private request cap + optional bounded token reservations
             checked-source clamped body before atomic charge; allocation remains unsettled, not usage
             actual-admitted two-slot first-poll/send-error/header facts; no body/usage/billing inference
        exec::input: argument/file admission; those old routes still refuse stdin
          -> construction::PreparedInput: read-only bootstrap + SAME retained ConfinedFs
             whole UTF-8 instruction/files/framing BEFORE session ID/store assignment
          -> input::Invocation: SAME Runtime builder, one original model-data turn
             private source facts + original partial state/outcome; not grants/public receipts
        exec::input::scopes: explicit launch-base normal-relative scope + output-file admission
             untouched source/values; same old complete syntax BEFORE effects, no public formats
             selected-workspace task/output paths; SAME old physical/publisher owners, no new grant
        exec::input::frontend: supported absolute scope/permission/env/session controls
             whole syntax/source + old grammar preflight BEFORE IO; explicit Defaults
             authoritative old Bootstrap credential/root/session checks -> old argv factories
        exec::input::arguments: bounded native argv; source/attachment flags ONLY
             whole grammar BEFORE config/stdin -> SAME four old invocation factories
             old Run/FileRun returned unchanged; no new runtime/terminal/writer/CLI
        exec::input::output::stdin: exact '-' + original guarded Destination; capture BEFORE stdin
             complete native/attachment admission -> retained-target recheck BEFORE IDs/store/providers
             same old actual FileInvocation/FileRun/FileTerminal/Record; no new reader/writer/CLI
        exec::input::stdin: separate exact '-' native pipe/owned-mode or original-rights availability polling
          -> SAME Bootstrap; original1MiB /10s-first-byte/30s-total / genuine EOF / UTF-8
             restore/close source -> old attachment/frame admission BEFORE sessions/store
             actual input Run, no public stdin/console/file/outcome/usage producer
        exec::construction: explicit config/discovery + session preparation
          + captured known credentials/frozen provider route -> private redactor seed set
          -> exec::runtime -> the SAME agent session builder (no credential re-expansion)
             retained ConfinedFs; immutable tool gate + all-role fail latch
             scoped context; no MCP/LSP/memory/background/guest callables
             silent sinks; raw checkpoint/journal writers disconnected
        gadgets::confined: private byte-safe filesystem facet over the SAME authority
        gadgets::sources: retained config/workspace + owned source/approval/original-policy snapshot
          -> opaque exec::runtime::GuestScope -> private gadgets::headless::GuestHost
             SAME original filesystem and all-role fail latch; typed import observation
             empty-capability describe; bounded WASI/HTTP/logs; no inherited ambient resources
             frozen-seed guest JSON + shared stdout/stderr/log capture BEFORE retention cuts
             direct synchronous WASIp2 writes; SAME-gate checks, no queued post-finalization writes
             NOT registered as model tools/catalog; original metadata/provenance remain private
        exec::redaction: bounded literal matching + abort and rendered-match verification
                         selected by guest/shell/filesystem/web/concrete-framework views and fixed gate denials
        exec::shell: actual builder ShellScope, ORIGINAL command and captured scrub policy
                     redaction/verification before head-tail cuts; raw byte/status facts retained
                     no raw spill, strict UTF-8, unsupported post-capture clean_output rejected
        exec::web: actual builder WebScope, frozen literal search auth, ORIGINAL URLs/queries
                   no ambient proxy/auth/cookie defaults; manual rechecked fetch redirects
                   original-byte checks before body cuts, bounded HTML, complete structured search views
        exec::framework: actual builder FrameworkScope + SAME-gate opaque concrete subagent source
                         known todo/task/message results before framework cuts; original typed state/delivery
                         checked whole todo JSON + todo context; NOT registry/inbox/all-profile/state publication
        exec::provider: actual builder ProviderScope; all-role copied request views before argument/HTTP encoding
                        original state/control/auth; exact checked bytes + source identity, per-attempt gate rechecks
                        source/schema constraints unchanged or refuse
          -> diagnostics: actual-client Sent/Received + per-checked-request lease
                          original error-body UTF-8 before 100k view / 400-scalar cuts; fixed unsafe failures
                          original attempt/status/body observations; same latch, bounded last failure only
                          success SSE/control/arguments and transport authority remain original/private
        exec::messages: only actual Runtime MessageScope; whole retained message admission
                        under SAME state lock BEFORE copies; frozen finite text/argument views
                        role/order/id/name/key/scalar/flags preserved or refuse; no aliases
                        checked snapshot bytes/count, SAME latch/binding; other state unselected
        exec::messages::selection: consume ONLY admitted View; newest whole-message suffix
                                   exact encoded budget/counts; no raw-source rescue or repair
                                   SAME gate and final literal check; original outcome unchanged
        exec::terminal: ONLY consumed actual Run; deny-only SAME execution seal
                        independent revocable lease/typed approval epoch; original return retained
                        checked original final text + whole retained messages, no raw state clone
                        not public outcome/IO/ASCII/shutdown/usage or all-role quiescence
        exec::terminal::selection: both channels admitted before combined-budget selection
                                   original checked answer/facts kept; newest whole-message suffix
                                   exact count/framing/lease/epoch checks, no unadmitted rescue
        exec::terminal::ascii: complete admitted channels -> bounded ASCII projection/verification
                               mapped whole-record offsets; post-escape budget/newest suffix
                               original output/facts and SAME lease/seal; no public schema/IO
        exec::terminal::human: actual immutable final answer + fixed private-fact diagnostics
                               whole original redaction/checks -> control/runner-safe UTF-8/LF
                               exact complete budget/lease/epoch; no state copy/public outcomes/IO
        exec::output: FileInvocation pre-session explicit external target -> actual FileRun/FileTerminal
                      only actual-terminal checked Record, fixed pre-run format/budget, one attempt
                      guarded private staging + create-only link, actual-link observation; no public/stdio/usage
        pending: guest tool/catalog/provenance integration, successful-response/alternative-state outputs,
                 full stdin/frontend ownership/console/file integration, usage/receipt settlement, storage/output/trace producers
```

`crates/craftsman-harness` is the framework. `crates/craftsman-rs` owns HTTP, config,
filesystem authority, subprocesses, libsql, transports and TUI. Gadget SDK/WIT
and proc macros are separate `craftsman-gadget` / `craftsman-gadget-macros` crates; the
harness does not gain those app dependencies.

Terminal drawing remains behind `tui/terminal.rs::SafeBackend`. Do not route
model/tool/repository-controlled cells around it. Frozen frame and model-request
goldens are compatibility guards, not files to regenerate casually. Bridge IO
regressions use a native ready/echo/exit fixture: startup is separate from the
bounded final-frame/close assertion, which fails on timeout rather than silently
passing. This test-only change does not add or alter production frames.

## 3. Implemented, private, or still planned

| Area | Current state | Evidence / next boundary |
|---|---|---|
| Framework names and TUI split | Implemented source | `craftsman-harness/src`, `craftsman-rs/src/tui`; dependency guard and frozen goldens |
| Shell hardening | Implemented mechanisms; model-visible options stay gated | `ProcessTree`, scrub, timeout/output metadata and tests; Unix groups default off pending signal routing |
| Gadgets G1 | Implemented, model-opt-in | Fixed `gadget_list`/`gadget_call`; first-load grants; change-only catalog refresh before main/delegated rounds and after compaction |
| Declarative gadgets/migration G2 | Implemented; legacy loader retained | Shared extension validation/precedence; create-only migration preserves originals; published deprecation still required |
| Embedded SDK/authoring G3 | Implemented | `gadgets new/build/doctor`, immutable SDK/skill; static ABI validation does not run `describe` |
| Runtime measurement | Implemented advisory benchmark | `gadget-validation.md`; three-OS runtime measurements are not live-model quality |
| Headless v1 specification | Accepted and merged as #95; #97 clarification explicitly accepted November1 | R1–R7 and126 baseline reference cases preserved;70 accepted admission cases/196 total are conformance, not real CLI enforcement |
| Legacy `-m` safety fixes | Merged as #96 | Non-TTY approval input denies without reading; child/compaction deltas silent; output-closed guards prevent post-failure dispatch |
| Immutable invocation policy | Private implementation, not CLI-wired | Exact selectors, manual/plan, deny precedence, credential-source env-pass restrictions |
| Confined filesystem | Private implementation merged as #98, selected by private runtime adapter only | All six operations, no-follow handles, protected paths, bounded workers; private guest host/text views consume it, not public registration; `headless-mode.md` §§12/14/17/21 |
| Explicit config/discovery/session preparation | Private implementation merged as #99, consumed by private runtime | Bounded selected roots, project trust, no-store ephemeral, random fresh IDs, read-only canonical resume/continue; `headless-mode.md` §13 |
| Shared-builder headless adapter | Private implementation merged as #100, no CLI/output producer | Consumes `Prepared`; retained filesystem authority; same immutable gate for main/delegated/contributed tools and fail latch for every provider; no interactive fallbacks; `headless-mode.md` §14 |
| Byte-safe guest filesystem facet | Private implementation merged as #101, consumed by the private guest host, not model/CLI-wired | `ConfinedFs` typed raw read/create/list; validated declaration plus immutable invocation policy; 10 MiB reads/1 MiB writes/1024 total directory entries; `headless-mode.md` §15 |
| Retained gadget sources/approval snapshots | Private implementation merged as #102, consumed by private guest host only | Selected config/ConfinedFs root handles, whole raw manifest/component hashes, explicit versus stored grant provenance, non-widening approvals and original filesystem/policy; `headless-mode.md` §§16–17 |
| Source-approved headless guest host | Private implementation merged as #103, not model/CLI-wired | Opaque actual-runtime scope; strict byte FS, empty-capability describe, typed caught-import latch, bounded HTTP/log/WASI and fixed diagnostics; `headless-mode.md` §17 |
| Captured credentials/seeded redactor | Private foundation merged as #104; selected by guest/shell/filesystem/web/concrete-framework/request views and fixed gate denials | Known configured values, frozen all-role route, bounded overlapping streams, complete source coverage and structured redaction; `headless-mode.md` §§18–24 |
| Guest JSON/stdout/stderr/log redaction | Private host adapter merged as #105, not a public/tool/storage producer | Actual opaque scope seeds; redaction and delayed literal verification before 64 KiB retention; synchronous gated WASIp2, strict UTF-8/fixed failures and original executed inputs; `headless-mode.md` §19 |
| Scoped private shell-output redaction | Private shared-builder adapter merged as #106, not a public command | Opaque actual ShellScope, same gate, captured scrub/credentials; original pipes redacted before cuts, scoped argument errors, strict UTF-8 and trustworthy raw ToolMeta; `headless-mode.md` §20 |
| Scoped private filesystem views | Private shared-builder adapter merged as #107, not a public command | Actual FilesystemScope, complete original coverage before page/hit selection, original IO/line/query facts, bounded checked names/framing and same typed fail latch; `headless-mode.md` §21 |
| Scoped private web views | Private shared-builder adapter merged as #108, not a public command | WebScope/frozen literal auth, original-byte pre-cut capture, bounded HTML, complete search JSON/field checks, actual request/byte facts and same latch; `headless-mode.md` §22 |
| Concrete private framework views | Private shared-builder adapter merged as #109, not a public command or all-state scrub | Actual FrameworkScope/source witness; known todo/task/message views before cuts, whole typed todo JSON and checked todo context; original state/delivery/metadata and remaining alternative producer gates; `headless-mode.md` §23 |
| Scoped private provider requests | Private shared-builder all-role request producer merged as #110, not a response/storage/public adapter | ProviderScope/original effective options/auth; copied text/decoded arguments/schema annotations before encoding, immutable identities/constraints, bounded exact bytes/client binding and SAME retry gate; `headless-mode.md` §24 |
| Private provider HTTP/error diagnostics | Partial client adapter, not a successful-response/state/public or usage producer | Actual send/receipt/per-request witnesses; frozen literal/strict UTF-8 before cuts, original bounded attempt facts, fixed unsafe failures and same latch; `headless-mode.md` §25 |
| Instruction and attachment-file admission | Private input-first implementation merged as #115, not CLI-wired | Explicit source/UTF-8/1 MiB instruction/1 MiB file/8 MiB total; retained FS/policy and complete framing before optional session creation; actual shared-builder model-only turn; `headless-input.md` |
| Checked main retained-message snapshots | Private actual-runtime view, not a public result/storage/all-role ledger | Actual MessageScope/state lock/frozen seeds/gate; whole source/argument admission before copies, bounded finite encoding, immutable original state/control and fixed refusal; `retained-message-views.md` |
| Bounded private retained-message selection | Private post-capture adapter, not terminal/result publication | Only checked View; exact byte-budget/newest whole suffix/counts and final literal check; unchanged source caps/state/outcome and SAME gate; `message-selection.md` |
| Private terminal-publication authority | Private actual-Run owner, not public result/IO/shutdown | Same deny-only execution seal, original outcome and independent revocable publication lease; typed epoch checks, checked final-text/whole-state-message fragment and truthful limits; `terminal-publication.md` |
| Private terminal-message selection | Independent terminal-lease adapter, not public result/IO | Complete channel admission before exact combined budget; newest whole suffix, unchanged checked answer/facts, counts/new-join checks; `terminal-message-selection.md` |
| Private terminal ASCII encoding | Actual terminal-lease encoder merged as #123, not public schema/IO/human rendering | ASCII UTF-16 escape projection; complete-channel/final literal checks, post-escape budget/whole-message suffix and unchanged data/facts; `terminal-ascii.md` |
| Private safe human display | Actual terminal-lease renderer merged as #124, not stdout/stderr or public text eligibility | Complete original answer/fixed fact diagnostics; visible control/backslash/runner defanging, exact combined LF budget and final literal/lease/epoch checks; `terminal-human.md` |
| Guarded private one-file publication | Merged as #125; explicit pre-session file route, not CLI/stdio/result/events | Actual FileRun owner and immutable outside-workspace/config target; complete checked Record before native staging, one guarded create-only link, fixed failure/actual-link observation; `guarded-output.md` |
| Explicit native byte-pipe stdin | Merged as #128; private, not CLI/console/file support | Exact '-' opaque selection; same-access native ownership/owned-mode or readiness-bounded polling,1MiB/10s/30s/real read EOF/UTF-8 plus old attachments/framing before sessions; `named-stdin.md` |
| Native stdin + guarded file composition | Merged as #129; private, not CLI/full source/output flags | Whole source/destination grammar; original output capture before stdin and recheck after admission before sessions; SAME Bootstrap/ConfinedFs and old paired controller; `stdin-file-output.md` |
| Private relative-scope/output-file argv | Merged as #132; private normal-relative/typed-preview adapter, not public exec/full formats | Explicit original launch base; old complete syntax/physical owners, selected-workspace data and guarded external file scope; `scope-output-arguments.md` |
| Private supported control/source argv | Merged as #131; private absolute-scope/permission/session adapter, not full public CLI | Old source/selector/ID grammar before IO; authoritative Bootstrap/credentials/ConfinedFs/session and same Run/FileRun; `control-arguments.md` |
| Private instruction argv parsing/routing | Source/attachment subset, not a public exec/whole-flag parser | Bounded original OsString inputs; exact source selection/flag arity before IO, four unchanged factories and old Run/FileRun; `input-arguments.md` |
| Headless frontend/stdin input, guest tool integration and persistence | Still open | Complete named-stdin exclusive ownership/console/file and flag/output-path integration; other tool/provider/storage redaction and compatible origin publication before callable registration; checkpoint/journal writers stay disconnected |
| Shared admission/usage | Private request default, separate atomic token-reservation opt-in and admission-correlated send/header observations; usage/settlement ledger still unwired | Actual-builder1..10000/default50 cap, source-bound bounded estimates/clamped reply before atomic charge; active/retained-unsettled observations, not measured usage;40 request +50 reservation +40 receipt-observation tests; #133/#97 accepted/merged, not public producer enforcement; `request-admission.md` / `token-reservations.md` / `admission-receipts.md` / `token-admission.md` |
| Real headless result/event output + redaction/trace | Still open | ASCII schema-backed production, create-only publication, seeded chunk/error/tool/storage redaction, trace-v1 instrumentation |
| Native OS sandbox / headless shutdown / verification | Separately planned | P2f / P2g / P4c; no current guarantee inferred from private filesystem checks or mock tests |
| Full Session API/events/content blocks/governed policies | Separately planned | P6/P7; existing callbacks/journal are not that future typed stream |

A private builder passing tests does not clear the whole P2a phase. Current
terminal gadget-host reads/writes still use their own capability adapter; they do
not inherit `ConfinedFs` merely because the module exists. A private guest filesystem
facet now reuses that authority and returns typed errors/binary bytes, but neither
legacy `GadgetRuntime` nor terminal `HostState` chooses it. Private source authority
retains preparation's config/workspace handles and returns owned approval snapshots
with the original filesystem/policy. The separate private `GuestHost` now consumes
those bytes and selects strict imports through the actual runtime's opaque scope,
with typed import/inner-tool failure observation on the SAME all-role latch.
The bridge golden also separates a bounded, real cold stats-cache setup from its
unchanged 30 s recorded HTTP checks after an initial Windows cold-walk failure;
all auth/shape/relay assertions, seven-step golden and 120 s scenario remain.
The stop assertion begins only after bounded cold guest setup/instantiation;
Linux/Windows caught and drove that fixture correction, without changing runtime
limits or bypassing a failed check. It
never writes grants/cache/SDK and does not register tools/catalog. Guest JSON and
shared logs now use preparation's frozen redactor before cuts; original descriptions/
approval metadata remain private, not a public schema. The profile still advertises
no callable gadgets before other tool/provider/storage adapters, compatible
provenance publication and builder registration are ready. The
authoring skill cannot be advertised headlessly if its user-directory/project-config workflow violates confinement.

## 4. Legacy one-shot versus accepted headless v1

`-m` remains compatible apart from its three mandated safety fixes. Its round
cap still exits 2, its ordinary completion exits 0, it takes input from the
argument, and existing broad user-config approvals are still honored. Fixing
piped approval reads does not turn it into an explicit-only job runner, make its
file tools workspace-confined, or suppress durable local state.

The future `exec` contract instead fixes invocation-only permissions, canonical
workspace scope, input/output/resource bounds, recorded role usage, fresh or
explicitly selected sessions, seeded redaction and machine output. Deadline /
signal / final-result shutdown is P2g; filesystem checks do not OS-contain native
shell or Cargo build scripts (P2f). Exit 0 will mean end of turn, not task
correctness; verification receipts are P4c. No unsupported flag may be accepted
and silently ignored during staged delivery.

Current frontend `skills::search_dirs_at` still starts from the home directory.
The new private preparation loader instead uses only the explicitly selected
config root and trusted project roots, supplying bounded skill guidance directly.
The private runtime now consumes that bounded snapshot without repeating
ambient discovery, including for declared foreground agents. Existing frontend
construction remains unchanged. This is not the full headless input/output/run
producer or a license to advertise inaccessible user-directory skill paths.

The headless shell runner uses captured credential sources (including disabled
services), removes those sources even if their names normally pass the legacy
toolchain scrub, and uses bounded memory-only capture under either output format.
It never creates raw spill files, including formatting/timeout paths. Silent
sinks and disconnected checkpoint/journal writers prevent those legacy fallbacks;
they are not seeded redaction of model/tool/state data. Private captured-credential
seeds and literal streaming/structured redaction merged as #104. Private guest
JSON/stdout/stderr/log views, scoped shell pipe/model-result text, scoped filesystem
views, scoped web fetch/search text, concrete built-in todo/task/message results,
todo context, copied provider request views and fixed gate denials now select them.
Original dispatch/state/registry/inbox remain private; their outgoing request copies
are selected, not persistence. Private HTTP/error diagnostics now select bounded original
body UTF-8 before view/diagnostic cuts and retain source-bound original observations separately.
Successful SSE/control/arguments, transport authority and alternative-state/storage/output
adapters remain unselected. Raw shell commands,
exit codes, duration and observed bytes remain original facts. Post-capture terminal
cleanup is explicitly rejected by this private shell path, not silently applied. The all-role provider key is frozen at
preparation and shares the seed snapshot instead of later environment expansion.
Only synthetic tests use this unfinished adapter, not live-quality evaluation.

## 5. Toolchain, builds and verification

Use the exact toolchain in `rust-toolchain.toml` (**1.99.1** at this update), not
a floating `stable`. The declared app `rust-version = 1.85` is not proof that
all current dependencies build there; the standalone SDK/macros retain their
separately tested 1.85 floor. The optional Wasmtime host has a higher minimum.
A read-only check of the official stable-channel manifest on October 23, 2026
matched this exact 1.99.1 pin (manifest date October 15); no toolchain installation,
floating pin, dependency or lockfile update was needed for this slice.

```sh
cargo build --release --locked -p craftsman-rs
cargo check --locked --no-default-features -p craftsman-rs
cargo test --locked --no-default-features --workspace --no-fail-fast
cargo test --locked --no-default-features --features craftsman-rs/gadgets --workspace --no-fail-fast
cargo fmt --all -- --check
python3 -I docs/schemas/check_exec.py --self-test
scripts/check-lineage.sh
```

Default voice builds also need the native dependencies listed in the README.
No-default-features excludes voice and Wasmtime; it now retains the app's small
filesystem-authority dependencies so confinement can be tested independently.
CI tests both feature sets on Linux/macOS/Windows. Cold authoring integration
fixtures explicitly select a 240 s native-build limit with a 300 s outer command
bound; they test safety/authoring behavior, not cold-compiler speed. Production
shell defaults stay at 120 s. Windows remains formally informational until P2d;
implementation merges nevertheless verify its exact-head
result instead of hiding failures behind that setting.

The initial #110 Windows gadget-enabled suite failed three real shell/FS startup or
partial-output fixtures (878 passed / 3 failed / 3 ignored), not a green platform result.
Those private Windows commands now flush ORIGINAL stdout, write their actual ready file
through `IO.File.WriteAllText` and wait through `Threading.Thread.Sleep`, without cold
PowerShell ready/sleep cmdlet discovery. Unix commands, six-second partial-call budgets,
20-second startup / ten-second post-readiness stop bounds, original-byte assertions and
all production `ProcessTree` behavior remain unchanged. Fresh ALL-FOUR final-head CI and
every mandatory step, including Windows, are required; see `headless-mode.md` §24.
The post-correction local request22/dedicated filesystem-stop1 checks passed, while the
focused shell module had 12 passed / 1 failed (three zero-byte Unix partial-startup attempts).
That local failure is not relabeled green or Windows fixture proof. #110's corrected
final head nevertheless passed all FOUR jobs and mandatory steps independently: main-unit
counts Linux/macOS770 feature-off /911 gadgets, Windows751 /881, all with zero failures.
The three formerly failing Windows fixtures passed both builds; review was the sole admin
override and tested/merged tree identity was verified. Section 24 retains the failed history.

Section 25 adds private HTTP/error diagnostic views only. An over-budget/UTF-8/rendering/
marker conflict discards ALL detail; genuine EOF alone finishes holdback. Original observed
counts/status/attempts are not inferred from views. One bounded last failure per client is
retained after fixed framework error conversion, not a role ledger, durable event or billing
receipt. Source/request identity and the same typed latch govern access and retries. No
successful SSE admission, transport sandbox, public output, raw storage or meter follows.
Its implemented-source diagnostics25 (both feature sets), runtime42, request22 and legacy
LLM10 suites passed; both production checks/reference126/driver9 and protected-source/
document audits passed. The one full local run was NOT green (main795/3/2, then600 s
alias/native stall abort124). The final test-only read-readiness adjustment's compile240 s
and test-typecheck120 s attempts also stalled/aborted; final-source hosted CI is required,
not assumed from earlier focused passes. No native/legacy deadline or assertion is relaxed.

The independent `exec::input` / `PreparedInput` adapter now validates instruction
and attachment-file sources before durable session selection or runtime construction.
It uses the same retained confined filesystem and immutable denies, preserves original
UTF-8 data/source bytes and passes model-only input through the existing builder.
A consumed invocation retains the original outcome/partial state, not a public safe
result. Those old routes still refuse stdin; the separate private byte-pipe route
now adds native reader/deadline/ownership before the same admission. Complete
console/file/exclusive frontend input/flag/output-path integration still gates `exec`. `headless-input.md` records private
resource ceilings and the stricter, unchanged later provider budgets. This branch does
not include or imply a merge/check override of pending provider response PR #112.
Its input30 suite passed with gadgets off/on; existing filesystem38/runtime42/
provider47/LLM10 and both warning-free production checks passed. Reference126,
driver9, formatting/lineage/document and protected-source audits passed; no full
local workspace/native retry or live-model/default-on/release proof is inferred.
Initial exhaustive error-map compilation failures and two incorrect fixture
assumptions were corrected without changing older error kinds, deny rules, source
bounds or assertions; `headless-input.md` retains that evidence. ALL FOUR exact-head
CI jobs/every mandatory step, Windows included, still gate merge.

#115 subsequently passed all FOUR exact-head jobs and mandatory steps (main units
Linux/macOS828 feature-off /969 gadgets, Windows805 /935; zero failures). Missing
review alone was overridden after fresh rules/comment/thread/base/head audit;
CI-tested and merged tree identity was verified. Pending #112 remains separate.

The actual runtime now selects a finite `exec::messages` snapshot before any raw
state clone: original message/argument admission, frozen data views and bounded
encoding share the state lock, while ids/names/keys/scalars/flags remain original or
refuse. `input::Run` selects it too. Late typed stops hide views; whole-message
omission/terminal/public output and other state fields remain unselected. See
`retained-message-views.md` for scope, private bounds and remaining producer gates.
Local bounded verification after adopting main's security lockfile update passed
28 retained-message cases in each feature mode, existing runtime42/input30/provider47/
LLM10 suites, both production checks, offline contract126/driver9 and source/docs
checks. The first 26/2 run's new adjacent-marker fixture corrections are retained
in that feature doc; no full local workspace/strict-Clippy/live rollout is claimed.
All FOUR exact-final-head CI jobs and every mandatory step remain merge gates.

#119 subsequently passed all FOUR exact-head jobs/mandatory steps: Linux/macOS
main856/997, Windows833/963, zero failed; all28 snapshot cases passed in both builds
on all three OSes. Missing review alone was overridden after a fresh audit, and
squash6a43802's tree/parent matched the actual four-job checkout/base evidence.

A separate private selector now consumes only those admitted snapshots, keeping the
largest newest whole-message suffix under an exact private byte budget, with truthful
omission counts and final framing verification. Original source admission/caps remain;
unsafe old messages cannot be omitted around those checks. Original state/outcome,
terminal publication and public R2/ASCII/result/storage gates remain separate. See
`message-selection.md`; documentation and all-four exact-head CI remain required.
Local bounded selection39 cases passed with gadgets off/on; unchanged snapshot28,
runtime42/input30/provider47/LLM10 suites and both warning-free production checks
passed. Offline contract126/driver9, formatting/whitespace/lineage and protected-source/
Markdown audits passed. The first38 run was green before the added boundary/outcome
checks; no failed attempt, full local workspace or strict-Clippy claim is hidden.

#120 subsequently passed all FOUR exact-head jobs/mandatory steps: Linux/macOS
main895/1036, Windows872/1002; zero failed, all39 selection cases in both builds
on all three OSes. Missing review alone was overridden after a fresh audit; squash
f99ef83 matched the four-job checkout tree and audited base/parent.

A separate private terminal authority now consumes the actual returned input Run,
closes its SAME execution gate without inventing/clearing approval, and retains
original runtime/outcome under a separate revocable publication lease. A typed late
approval epoch supersedes old fragments; fresh capture retains real return/stop
observations. Final text comes from the original outcome, not the retained last
message. Whole-message/raw-output source/copy/framing limits remain private; there
is no public status/IO/ASCII/meter/storage or all-role/process-quiescence claim.
`terminal-publication.md` records source/binding/bounds and the new-fixture compiler
correction; docs and all FOUR exact-head jobs/mandatory steps remain merge gates.
Local bounded terminal43 cases passed with gadgets off/on; unchanged view28/selection39,
runtime42/input30/provider47/LLM10 and both warning-free production checks passed.
Offline contract126/driver9 and source/format/lineage/Markdown checks passed. Only a
new StateFs fixture-constructor compile error was corrected; no older assertion,
native deadline, full local workspace/strict-Clippy or live rollout claim is hidden.

#121 subsequently passed all FOUR exact-head jobs/mandatory steps: Linux/macOS
main938/1079, Windows915/1045, zero failed; all43 terminal-authority cases passed
in both builds on all OSes. Missing review alone was overridden after fresh rules/
head/base/review/thread audit; squash7d48ddd has the identical four-job checked tree
and audited base as sole parent.

A separate `Terminal::selection` now preserves original checked answer and private
return/approval facts, admitting BOTH channels before reducing only whole newest
messages to an exact combined budget. Unadmitted old messages/outputs still refuse;
whole Snapshot, live selector/layout and terminal lease/seal semantics are unchanged.
`terminal-message-selection.md` records private bounds/limitations and new-warning
history; documentation and all-four exact-head CI remain mandatory.
Local bounded terminal-selector43 cases passed with gadgets off/on; unchanged
terminal43/live-selector39/view28/runtime42/input30/provider47/LLM10 suites and both
warning-free production checks passed. Offline contract126/driver9, format/lineage
and source/Markdown checks passed. The initial42 run had only a new unused-export
warning, corrected by API placement; no full local workspace/strict-Clippy/live
rollout or older assertion/native-limit change is claimed.

#122 subsequently passed all FOUR exact-head jobs/mandatory steps: Linux/macOS
main981/1122, Windows958/1088, zero failed; all43 terminal-selector cases in both
builds on all OSes. Only missing review was overridden after a fresh audit; squash
825cf09 matched the actual four-job checkout tree/audited sole parent.

A separate private terminal ASCII encoder now preserves checked output/facts and
projects complete admitted channels before post-escape whole-message selection.
DEL/non-ASCII UTF-16 escapes, strict scalar/offset checks, bounded6MiB/channel staging,
unchanged KMP verification and exact <=1MiB final framing remain under the SAME
publication lease/typed epoch and deny-only seal. Original source caps/codecs/live
views/UTF-8 selector stay unchanged; this encoder supplies no human rendering,
OS redirection or public schema/IO/outcomes/R2/#97/P2g/provenance completion.
`terminal-ascii.md` documents
scope/limitations/evidence; docs and all-four exact-head CI remain required.
Local bounded ASCII47 cases passed with gadgets off/on; unchanged UTF-8 terminal
selector43/authority43/live-selector39/view28/runtime42/input30/provider47/LLM10 and
both warning-free production checks passed. Offline contract126/driver9 and source/
format/lineage/Markdown audits passed. No first-run failure/warning fix, full local
workspace/strict-Clippy/human/OS/public rollout is inferred.

#123 subsequently passed all FOUR exact-head jobs/every mandatory step: Linux/macOS
main1028 feature-off /1169 gadgets, Windows1005 /1135, zero failed. All47 ASCII
fixtures passed in both builds on all OSes. Fresh rules/head/base/review/comment/
thread audit permitted only missing-review override; squashb901d6f has the identical
four-job tested tree and audited base825cf09 as sole parent (`terminal-ascii.md`).

A separate private human renderer now consumes only the immutable actual final
answer and fixed original-return/approval/presence diagnostics, not retained/raw
state or JSON decoding. Complete source redaction/checks precede visible control/
backslash/overlapping colon/hash defanging; exact combined UTF-8/LF budget precedes
rendered allocation. Whole-channel/new-join KMP checks in both fixed orders and
SAME publication lease/typed epoch guard every release. No answer truncation,
fact aliases, raw errors, stdout/stderr eligibility, writers, TTY/PowerShell/OS or
public outcome/usage claim is added. Source caps/old codecs/ASCII/live selectors/
terminal seal/SafeBackend and other frameworks/hosts/frontends/deps/contracts stay
unchanged. `terminal-human.md` records bounds, attempted/final evidence and remaining
IO/public/R2/storage/provenance/#97/P2g/live/release gates; all-four exact-head CI and
documentation remain required in this partial slice.
Local bounded human60 fixtures passed in both feature modes, with unchanged
ASCII47/UTF-8-selector43/authority43/live-selector39/view28/runtime42/input30/
provider47/LLM10 and both warning-free production checks. Offline contract126/
driver9 and source/format/lineage/Markdown audits passed. The initial54/55 fixture
run and empty provider filter are recorded, not counted as target coverage. No
full local native workspace/strict-Clippy/runner/TTY/OS/public rollout is inferred.

#124 subsequently passed all FOUR exact-final-head jobs/every mandatory step:
Linux/macOS main1088 feature-off /1229 gadgets, Windows1065 /1195, zero failed;
all60 human fixtures in both builds on all OSes. Fresh unchanged head/base/rules/
review/comment/thread audit allowed only missing-review override; squash875056f
has the identical four-job tested tree/auditedb901d6f sole parent. Both additional
full local default-feature-free workspace builds passed too, without warnings.
`terminal-human.md` records the verified evidence separately from earlier attempts.

A new private FileInvocation route now captures immutable external directory/name/
format/budget before session IDs/store creation, then consumes only its actual Run
into a one-attempt FileTerminal. Old source/codec checks and new whole LF/framing
checks precede native staging. Bounded guarded writes/flush/sync/content/identity
checks plus create-only link preserve existing entries; late stop/failure retains
only actual own-link return observation, never fake complete delivery or usage.
Private random staging/cleanup, inherited-ACL/Windows best-effort/native blocking/
external-race/CAS/P2g limits are explicit in `guarded-output.md`. Old routes/Options/
FS permissions/codecs/lease/epochs/deps/contracts/SafeBackend remain unchanged.
Public exec/stdio/inside-workspace output/R2/outcomes/results/events/storage/#97/
provenance/P2g/live/release gates stay separate and disabled. Docs/all-four exact-
head CI accompany this partial boundary, not a public runner.
Earlier bounded file64 fixtures passed in both modes, with old human60/ASCII47/
selectors/authority/runtime/input/construction/FS/provider/LLM and production checks
green. Earlier full local units1152/1293 passed with integration binaries unchanged.
Initial legacy startup failures and a stale baseline Cargo-artifact rerun are
recorded as failures/non-current evidence in `guarded-output.md`, not hidden or
fixed by relaxing old source/assertions/deadlines. All-four final-head CI remains
required before merge. Initial PR125 Windows failed staging-handle cleanup and
new literal-path/rename fixtures; those failures are recorded, with explicit
staging handle closure/retained verified final-handle reads and preserved scope
assertions. The second Windows run cleared the IO failures but still failed three
new rename fixtures (1124/3 feature-off,1254/3 gadgets): native sharing denial32
was not PermissionDenied, and retained directory handles prevented actual movement.
The test-only correction accepts only exact Windows access/sharing codes5/32,
checks unchanged original identity/no movement or preserves all successful-rebind
refusal assertions, and never loosens production directory opens. No failure is
bypassed; ALL FOUR final-head jobs and mandatory steps remain required before merge
(`guarded-output.md`). The second correction's default-parallelism local feature-off
full run had1149 passes/3 unchanged shell startup/timeout failures; its gadget full
run passed1293 plus all integration binaries. One four-test-thread feature-off full
run then passed1152 plus all integration binaries with the same actual assertions/
deadlines; no default-concurrency cold-start claim. These outcomes are recorded
separately, not conflated with earlier green runs or final-head CI.
The final content-pass audit found seek/EOF outside the loop counter: a real
boundary test reproduced over-ceiling publication, then the same4096 cap was
tightened to include seek/payload/EOF without widening source/public budgets.
Private coverage is now66 native fixtures (64 Windows); read-size/attempt observers
are test-only, not public usage/receipts. All-four fresh final-head CI is still
required for this additional correction.
The final native-result audit also found failed cleanup bypassing its post-result
check: a real foreign-identity fixture reproduced Changed instead of a late-stop
result. Cleanup now retains the native result before SAME ready check, and the
actual paired controller rechecks after all early results/Drop cleanup. Native
failure without stop stays Changed; actual approval/lease revocation overrides it,
without deleting foreign/final entries or faking absence, usage or delivery.
All66 final focused cases pass in both builds;21 bounded/source checks are green
without warnings. Final full four-thread feature-off was NOT green (1151/3 in
unchanged FS/shell real-startup/invalid-UTF8 fixtures); gadget full1295 plus all
integration binaries passed. The failure is preserved, not retried/hidden through
old source/assertion/deadline changes or a fictitious default-cold-start claim.
Fresh ALL-FOUR exact-final-head full CI/every mandatory step remains required.

Keep source/runtime tests, hosted live-quality evidence and published-release
compatibility separate. These changes do not install the CLI, modify real user
config/grants, publish a release, flip gadget visibility, remove the legacy loader,
or rewrite the old fixture corpus, protocol/request goldens or trace-v1 contract.

## 6. Documentation maintenance during incomplete implementation

Every implementation PR updates this status page, README entry points/layout and
its relevant contract/feature docs **in the same change**. Mark each surface as
implemented, private-but-unwired, or planned; name the remaining prerequisites and
truthful limitations. Do not wait for phase completion to document code changes,
and do not describe an accepted proposal, mock run or merged source as a shipped
CLI, containment boundary or live-quality gate. Historical baseline evidence and
target diagrams stay labelled as such rather than being silently rewritten.


## 7. Private explicit native stdin progress — October25 (Asia/Taipei)

#125's guarded-file boundary merged as63395cf after ALL FOUR exact-final-head jobs
and every mandatory step, Windows included. Main counts Linux/macOS1154/1295 and
Windows1129/1259 were zero failed; only missing review was overridden. The tested
current-main merge tree/sole audited squash parent and all original scoped blobs
were verified. Local feature-off1151/3 native-startup failures remain preserved.

A separate private `StdinInvocation` now selects only positional `-` after whole
count/path grammar, then uses SAME Bootstrap/ConfinedFs/captured credentials. A
retained native byte-pipe handle and owned-mode/readiness-bounded polling admit the original
1MiB stream with10s first-byte/30s absolute total-read clocks, no background reader.
Strict EOF/UTF-8/preamble/attachment/frame admission plus owned mode restoration
precede sessions/IDs/store/provider construction. Existing argument/file/FileInvocation
APIs still refuse stdin; no normal frontend/CLI/STDout/result/meter is enabled.

This initial private subset refuses console/TTY/devices/sockets/regular files and
unsupported read/query/synchronous rights, rather than faking a bounded blocking
reader. Native read-only polling uses existing grants, not an access upgrade. Original
source bytes/facts and all old source/read/codec/permission limits stay separate
from public usage/receipts. Mode sharing/external native races and restoration/OS
limits are explicit; cancellation owns no pending read thread. Fixtures, corrected
first-attempt diagnostics and all-four fresh CI gates are tracked in
[named-stdin.md](named-stdin.md), not inferred from the earlier file CI. #97/remaining
public outcomes/results/trace/provenance/gadget/P2g/live/release gates stay open/off.


Collected local evidence for this new stdin source:39 normal native fixtures passed
per feature build, one argv-only helper intentionally ignored;22 bounded/old-suite/
production/contract/format/source checks passed warning-free. Full local four-thread
feature-off1191/2 failed unchanged FS/shell real-startup assertions (no-fail-fast
continued all other binaries green); gadgets1334 plus all other binaries passed.
The failed run is preserved, not retried or hidden by changed deadlines/assertions.
Current fences/55 local links and all older source/SDK/framework/host/frontend/
protocol/lock/pin scopes are verified; fresh all-four final-head CI remains required.

### Native Windows correction (PR128)

The first stdin revision50cec5/CI40113876534 passed fmt/Linux/macOS but failed
Windows **Test and Test gadget host**:1156/12/3 and1286/12/4
(passed/failed/ignored). The informational aggregate is not all-platform success;
no merge was performed. Corrective native code preserves Pending versus genuine
EOF with direct ReadFile; that first corrective attempt requested read/mode
attributes without data-write rights,
and refuses asynchronous/non-byte/non-readable handles before reads. Two added
regressions bring the inventory to41 normal cases plus the same ignored argv-only
helper. Old real-stdin/deadline assertions/limits and all public/dependency gates
stay unchanged. See [named-stdin.md](named-stdin.md) for the preserved failure
history and exact-revision verification requirements; earlier39 local passes are
not final corrected-source or successful Windows evidence.

The correction's22 bounded source-local rows passed without warnings (native41
normal cases in both modes; old suites and both locked production checks intact),
with66 resolving local Markdown links. This is separate macOS/local evidence,
not a successful Windows access request or a final all-platform CI claim.

### Original-rights native read-only polling after the second failed revision

Second revision3c0f3da/CI40121387466 passed fmt/Linux/macOS but failed both
Windows steps:1165/5/3 and1295/5/4 (passed/failed/ignored). The former false-EOF
cases passed; five unchanged actual inherited-stdin fixtures still failed per
build. Current same-access duplication never requests additional rights. Actual
HANDLE metadata selects owned NOWAIT where available or unchanged-mode native
availability polling for blocking read-only pipes. Open-empty sources never
start a blind data read; readiness/peer closure is not EOF or consumed-byte fact,
and actual ReadFile probes preserve queued bytes and complete only at real EOF.
Seven added read-only cases bring the inventory to48 normal/one ignored without
changing any earlier42 assertion/helper bodies or native/source limits. No generic
reader/thread/reopen/configured-strategy/error fallback is added. Synchronous
peek/foreign-reader/native-call limits and the missing public exclusive-input
lifecycle are explicit in [named-stdin.md](named-stdin.md). No merge or full Windows
success follows from either earlier revision/local suite; require every mandatory
step in ALL FOUR fresh exact-final-head jobs, with public/callable/meter gates OFF.

Current original-rights source-local verification passed22 warning-free bounded
rows, including48 normal native cases in EACH feature mode and both locked
production checks;68 local Markdown links/fences and exact protected scope pass.
No duplicate full native-shell retry was used; earlier full/local/hosted failures
remain historical, not current-source Windows or full-suite success.

## 8. Private stdin/guarded-file composition — October25 (Asia/Taipei)

The native-stdin foundation #128 merged as72abf26 on October24 UTC after all
FOUR final-head jobs/every mandatory step and raw tested-tree/parent proof. All48
native cases passed BOTH builds on Linux/macOS/Windows; old Windows warnings were
identical, not suppressed/new stdin warnings. Two earlier failed Windows heads
remain documented, not successful source evidence. No public frontend was enabled.

New StdinFileInvocation prepares complete source/destination grammar, SAME
Bootstrap/captured credentials/retained roots, and read-only original output target
BEFORE stdin inspection. Complete native EOF/UTF8/preamble/attachments/framing and
closed/restored native input precede a second retained identity/absence check,
then original session/store/runtime core. Only that opaque pre-run preparation
constructs the existing paired file invocation in its private descendant. Run,
terminal/record/publisher and all native/source bounds remain unchanged. A target
planted during input waiting refuses before IDs/provider effects; bytes cannot
be rewound, and this is not an external-writer CAS. Test-only readiness/pipe/fault
hooks act on the actual owner, never supply production authority. See
[stdin-file-output.md](stdin-file-output.md), [named-stdin.md](named-stdin.md) and
[guarded-output.md](guarded-output.md). Public exec/flags/R2/stdio/fullstdin/
trace/storage/provenance/callables/catalog/#97/P2g/live/release stay separate/off.
First smoke passes and later NEW fixture failures are preserved; full final-head
all-platform/every-step evidence is still required independently before merging.

Current composed-source bounded verification passed all24 rows warning-free:
new42 and unchanged native48 normal cases in BOTH modes, old suites and both
locked production checks intact, reference126/driver9/fmt/whitespace/lineage.
The exact24-path protected audit and86 resolving Markdown links pass. This is
local/synthetic evidence only; latest-head full all-platform/every-step CI remains
independent and mandatory. No public exec/stdio/usage/OS/default-on claim follows.

The first composition CI40147790277 at d869ff7 passed fmt/Linux/macOS but failed
BOTH Windows test steps: feature-off1216/3failed/4ignored and gadgets1348/1failed/
5ignored. Run-level success is insufficient because Windows is informational.
ONLY the NEW dot/parent fixture is corrected: raw OsString appends preserve its
original spellings instead of Windows verbatim PathBuf::join normalization.
All older production/test bodies remain exact. The TWO unchanged feature-off
shell failures (partial-output2s/PID-before-timeout) remain failed evidence;
no deadline/assertion weakening or unchanged full retry is used. The corrected
head must independently pass EVERY mandatory step in ALL FOUR jobs, old shell
cases included, before review-only admin merge. See `stdin-file-output.md`.

Corrected-source local verification repeated the SAME24 bounded rows warning-free,
including new42/native48 in BOTH modes. The exact24-path protected scope and86
Markdown links/fences pass; separate correction logs preserve earlier attempts.
No source/deadline/assertion fallback or full local workspace retry was used.

PR #129 subsequently merged as87b357d after all FOUR corrected-head jobs and
EVERY mandatory step in CI40155545193. All42 composed/all48 unchanged native
cases passed BOTH builds on all three OSes; Linux/macOS main1244/1385 and Windows
1219/1349 passed. Tested/merged tree213d428 and audited sole parent72abf26 match;
only missing review was overridden. The first failed Windows head stays preserved,
with no old shell assertion/deadline/process change or unchanged full-CI retry.
Ten Windows warning signatures/19 occurrences remained identical across both
#128 attempts, its final run and both #129 runs; no warning-free-Windows claim.

## 9. Private instruction argv routing — October25 (Asia/Taipei)

`ArgumentInvocation` and `ArgumentFileInvocation` first validate bounded ORIGINAL
native argv using only positional instruction/exact-`-`, `--prompt-file`, repeated
`--file`, joined values and end-of-options. Complete grammar precedes bootstrap,
source/target inspection, native input, IDs/store and providers. Borrowed native
lengths/counts precede decoding/retention; old complete input/path/count admission
is reused, never normalized into a grant. Unsupported full frontend flags refuse
instead of being silently accepted. Explicit Options and guarded Destination stay
pre-run typed inputs; this does not implement their public flag parser.

Only the original four factories construct actual runtime/source/target ownership.
The new opaque wrappers consume into EXISTING Run/FileRun, not caller Runtime/Run/
seeds/gates/bytes, a new terminal/record/publisher or post-run format/path switch.
Old native pipe rights/modes/EOF/1MiB/10s/30s and guarded-file epoch/create-only/
cleanup/OS limits stay unchanged. No stdout/stderr/exec/result/event/trace/storage/
headless-gadget visibility/#97/P2g/live/release gate is enabled. See
[input-arguments.md](input-arguments.md) for subset syntax and actual native fixtures.

The initial production check completed with one NEW private-interface warning;
only the fixed-text SelectionError visibility was corrected, not suppressed.
The initial57 normal cases passed with one unused NEW fixture import warning.
Adding actual-bootstrap coverage uses that import; strengthening the native helper
keeps BOTH actual literal/stdin requests without mock/history resets. The current
inventory is58 normal plus one argv-only ignored helper; its source-local/all-
platform evidence is independent of the earlier57/#129 results and must meet
ALL FOUR exact-final-head jobs/EVERY mandatory step before review-only admin merge.

Collected current-source evidence: all26 bounded local rows passed warning-free,
including new58, unchanged composition42 and native48 normal cases in EACH feature
mode, every selected older suite, both locked production checks, reference126/
driver9/fmt/whitespace/lineage. Exact14-path protected audit keeps older bodies
unchanged after one input-module append;68 local Markdown links/fences pass.
This is local/synthetic source evidence, not a substitute for ALL FOUR fresh
exact-final-head jobs/EVERY mandatory step, both Windows feature builds included.


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

Current corrected-source evidence: all30 bounded rows passed warning-free,
including all37 guest cases at four test threads, new58 argv/old48 native/old42
composition in BOTH modes, every selected older suite, both production checks,
actual marked/unmarked compile probes, fixture/root fmt, reference126/driver9/
whitespace/lineage. The exact22-path source/old-assertion/30s-1s-5s audit and74
local Markdown links/fences pass. Source branches/probe payload strings are
byte-exact, not only token-normalized. This is not final-head all-platform CI;
ALL FOUR fresh jobs/EVERY mandatory step still independently gate merge.


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

Collected current-source evidence: all32 bounded local rows passed warning-free,
including new58 controls and old58 source argv/48 native/42 composition in BOTH
modes, old37 guest tests locally (35 compiled on Windows), every selected prior
suite, both locked production checks, marked/unmarked existing compile probes,
fixture/root fmt/reference126/driver9/whitespace/lineage. The exact17-path audit
keeps EVERY older Rust body/test/fixture/host/read/write/codec/limit unchanged after
four narrow append-only declarations/old grammar helpers;78 local links/fences
pass. This is source-local/synthetic evidence, not fresh all-platform CI or a
public/default-on/live/usage/source-OS guarantee. ALL FOUR exact-final-head jobs
AND EVERY mandatory step/both modes/Windows still independently gate merge.


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


## Token-admission clarification — accepted November1 (Asia/Taipei)

The new [token-admission.md](token-admission.md) review artifacts address #97's
truthful30-used/100-budget refusal before an81-token reservation. Optional
`_meta.craftsman.limit.admission` holds estimated input/requested reply/remaining,
without changing measured role usage, logical call counts, existing outcomes,
units or standard ACP stops. Exhausted/overrun observations keep their old mode;
pre-exhaustion requires coherent context and STRICT insufficient remaining room.

Original25 fixtures/126 reference entries retain values/serialized text;70 new
record/stream/wire cases and eight independent native counter/ledger tests make196
reference cases/11 native tests. Integer/role/event/partial-output constraints are
not weakened. First NEW fixture null-output/missing-run-start failures are preserved;
corrections are NEW data only, not loosened old requirements/assertions/limits.

The user explicitly accepted this narrow contract and compatibility judgment
November1, replying "yes approve" to the specific field/strict reservation/v1/
older-validator/collision proposal. Legacy semantic validators still need an update;
recognizing the optional field can still collide with old unspecified metadata.
Neither risk is erased by acceptance. R1–R7's October20 acceptance is unchanged;
October31's general continue/CI/admin evidence is not retroactive acceptance.
Runtime/native readers/publishers/guest/SDK/WIT/framework/deps/lock/pin/frontends/
SafeBackend/ACP/v4/trace/goldens/corpus remain unchanged. #97's contract question
is resolved; exact-head all-four/every-step/both-mode/Windows verification gates
accepted-artifact merge. Shared meter/public producer/live/release stay unwired/off.


Historical pre-acceptance source-local evidence: all38 valid bounded rows passed warning-free,
including native11 in BOTH modes, reference196, all protected scope/control/source/
native/composition/runtime/guest suites, both locked production checks, existing
marked/unmarked compile probes, source/CLI/compatibility/Markdown/fmt/lineage checks.
The exact12-path audit preserves runtime/framework/SDK/WIT/deps/lock/pin/frontends/
protocols/goldens/trace/corpus and original25/126 serialized entries;62 local links
and balanced fences pass. Initial NEW fixture errors, a NEW comment-wording lineage
failure, and an unsupported EXTRA trace-driver switch are preserved, not called
passing evidence. Only new fixture/comment/driver setup was corrected; no old
check/allow-list/trace/source/limit was relaxed and no full suite/CI retry was used.
These are historical pre-acceptance source-local results, not fresh tests of the
November1 documentation-only acceptance update. Runtime/test/schema/checker/vector
bytes stay exact and fresh exact-head CI/document/source proof gates this update.
The explicit contract decision above is separate; shared meter/public producer/
live/release remain unwired/off.


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

### November 3: existing Windows partial-output fixture maintenance

[windows-shell-fixtures.md](windows-shell-fixtures.md) records the November2
post-merge feature-off Windows failure separately from #136's green exact-head PR
checks. Only the test command removes `Write-Output`/native `ping` in favor of
original stdout/stderr, explicit flushes and a direct 60s block. The old 2s call,
30s outer and 2s drain limits, assertion bodies, Unix input, production shell and
Job Object/group semantics are unchanged. Three Windows-only cases check input
order, real two-format 33-byte partial capture, and genuine zero-observed-output
refusal before the first write; no retry/warmup/fabricated output is selected.

Native Windows/all-job/every-step/both-mode evidence is still required, even
while Windows is formally informational. This is not the private headless six-second
readiness fixture, usage/settlement, public exec, OS shutdown or live acceptance;
those remaining gates and independently blocked provider work are unchanged.

### November 3: cfg(test)-only scoped-shell observations for unresolved #138

[shell-test-observations.md](shell-test-observations.md) records native launch
preparation/return and post-delegation pipe facts through the actual private
Shell owner. Fixture panic teardown reports only bounded fixed metadata, never
commands/paths/keys/raw bytes/errors; no global hook, env/cwd mutation, writer or
runnable-agent interface is added. Every original assertion/20s readiness/15s
invalid-byte/10s stop and production capture/pump/process behavior stays exact.
The failed local full runs remain failed; a new instrumented or focused pass
neither replaces them nor establishes the cause. #138 is unresolved. This is
not production tracing, public exec/usage settlement or a blocked-provider bypass.


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


## Test-only original shell poll/selection follow-up — November 4, 2026 UTC

[shell-poll-progress.md](shell-poll-progress.md) records a bounded diagnostic
follow-up to unresolved #138 after the independently verified rebrand. Original
inner read polls now retain optional count/Pending count/last result/time, using
the exact original waker/buffer/result before metadata recording. Existing
reader/wait select branches retain enter/return facts, not OS startup or
inner-wait polling. A pre-stop copy stays historical and separate from real
later drain; no additional native lookup or changed snapshot ticket/clock.
The entire new progress section fits4096 fixed ASCII/LF bytes or refuses whole.

This is cfg(test)-only, not a production fix or public producer. All13 new cases
pass both local modes without compiler warnings; both locked all-target checks
and frozen contracts/source/history/link audits pass. ONE changed-source full
run per mode then finishes warning-free: off FAILED1600/4/7 (12 new passes/one
new normal-output failure plus three old readiness failures), gadgets1747/0/9
passed. Original Pending polls/no wait-selection entry are phase facts, not a
root fix; the gadget pass does not erase failures. Exact-head hosted verification
is in progress. Every old assertion/deadline/process/reader/native bound remains
intact and #138 OPEN. No public exec/usage/settlement/callable-gadget/P2g/live/release or
independently blocked provider work is enabled.


## Corrective environment-test isolation — November 4, 2026 UTC

PR146's diagnostic source is now merged after ALL FOUR exact-head jobs/EVERY
setup/mandatory/post step/BOTH modes/actual macOS15+Windows/source/inventory/
warning/tree proof. Independent merged-main41825763199 is audited separately;
the prior FAILED1600/4/7 off/new-normal-output failure is NOT replaced.

The next [test-environment-isolation.md](test-environment-isolation.md) correction
removes actual unsafe test-global environment mutations. Config/MCP/route
assertions retain their original bodies in fixed marked children with immutable
pre-start environment plans; the same concurrent serve protocol/auth assertions
select a thread-safe cfgtest-only per-router source. Original production env/
empty/lossy/header/constant-time auth behavior is independently exercised, not
mocked. No runtime/shell/permission/deadline/frame/SDK or user-config change.
Validation is in progress; fixing this concrete isolation defect does not by
itself prove #138's root cause. Public exec/usage/settlement/callables/P2g/live/
default-on/release and independently blocked provider work remain off/separate.


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
