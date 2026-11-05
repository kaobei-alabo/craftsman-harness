# Gadget validation and the next gates

G1 runtime, G2 declarative surfaces/migration and G3 authoring are implemented.
That does **not** yet clear the model-facing default or permit immediate removal
of the legacy loader. This document distinguishes runtime measurement, live
model-quality evidence and the published compatibility cycle.

## Reproducible runtime measurements

Run the developer microbenchmark from a trusted checkout with `python3` 3.11+
(or the configured CI interpreter) and the installed, exact Rust toolchain and
`wasm32-wasip2` target from `rust-toolchain.toml`:

```sh
python3 -B scripts/test-benchmark-gadgets.py
python3 -B scripts/benchmark-gadgets.py --samples 500 --warmups 20 \
  --output target/gadget-benchmark-local.json
# After the dependencies are cached, add --offline to forbid Cargo fetching.
```

The driver diagnoses missing toolchains/targets read-only; it never installs
them. It builds only the primary test binary, in release mode, with gadgets
but without native voice dependencies. It runs one exact ignored benchmark,
not the workspace suite or any model-backed test. Report publication is
create-only: choose a new filename for each run.

The measured fixture is repository-owned Rust source. It has no file or HTTP
capabilities and gets an invocation-scoped explicit grant in a temporary
workspace/config directory. No real user configuration, grants, thread store
or model route is loaded. Conventional API-key/token/secret/password variables
are removed from benchmark subprocesses. The trusted native Cargo build is
**not an OS sandbox**; this is not a safe runner for arbitrary model-written
source, and no broader secret-redaction guarantee is claimed.

The ASCII JSON report (`craftsman-harness/gadget-benchmark@1`) contains:

- The source commit/dirty marker, compiler and crate versions, platform,
  component and measured test-binary SHA-256 identities. Dirty runs are local
  diagnostics, not clean-reference evidence. Nonempty custom compiler flags
  are marked without exposing their contents.
- Fixture setup (including source-component builds) and runtime initialization,
  separately from the cold dispatcher call. The wrapper's total build-and-run
  wall time is also separate; it is not gadget-call latency.
- One cold `gadget_call` through the actual tool dispatcher. This includes its
  rescan, schema/argument validation and first guest instantiation. It is not
  a cold-call distribution.
- Warm dispatcher and automatic catalog-refresh distributions, measured outside
  `Tool::call`/`prepare_model`: min, nearest-rank p50/p95/p99, max and arithmetic
  mean, in nanoseconds, after the specified warm-up count. Calls use fresh
  instances; the compiled component and description remain warm in memory.
- Assertions of one compiled component, no new notices during unchanged rounds,
  unchanged tool definitions, no `gadget_list` calls and zero model requests.
  The native-code disk cache is disabled for comparable warm/cold semantics.

The manual `gadget-benchmark` workflow runs the same driver on Linux, macOS and
Windows and uploads only its validated JSON report. It uses the Rust pin from
the checkout and needs no model credentials. It has **no timing threshold** and
is not a required PR check. Ordinary PR CI runs the driver contract tests and
compiles/tests the Rust statistics; the measurement itself stays ignored there.

Compare repeated clean-source runs on the same platform/runner class. No
latency threshold is adopted from one laptop result. The planned benchmark
job establishes thresholds only from the first three nightly measurements.
This microbenchmark reports `quality_gate_cleared = false` and cannot turn on
`gadgets.enabled`.

## Live evaluation: prerequisites still outstanding

The next larger engineering dependency is **the complete P2a/P2g unattended
runtime**, not acceptance of the specification: R1–R7 and the schemas in
`docs/headless-mode.md` were explicitly accepted and merged as #95 on October 20,
2026. #96 implements the legacy `-m` fixes and private invocation-policy parser;
the next private filesystem layer does not yet supply isolated construction,
guest-host enforcement, usage/redaction/output/trace or P2g shutdown controls.
`craftsman exec` is not exposed. See [runtime-status.md](runtime-status.md) for the
current architecture and open boundaries. Current synthetic `-m` tests prove
authoring and deterministic pickup, not a production-quality unattended runner
or model non-inferiority. #97 records an uncovered token-admission outcome case
that must be represented truthfully before the shared meter is wired.

The planned live validation needs:

1. **P2a/P2g headless contracts and controls:** explicit-only permissions,
   workspace/protected-path confinement, ephemeral runs, redacted result/trace
   output, a shared main/subagent/compaction usage meter, bounded rounds/tokens/
   wall time, process-tree cancellation and exactly one final result.
2. **P3a isolated runner and corpus:** throwaway containers or VMs mounting only
   the task directory, per-run network policies, scoped provider-key injection,
   pinned task dependencies and deterministic graders, with held-out tests
   applied only after the model stops. Do not run `-m --auto-approve` with real
   keys against the developer's host as a substitute.
3. **P3b readiness and comparisons:** the runner host/operator, approved budget,
   concurrency/rate limits and provider retention/benchmarking review; R0/R1
   baseline and A/A noise reports, per-model paired live suites, and replay
   smoke checks. Provider/model identifiers must be supplied explicitly by the
   approved run configuration, not guessed from old example configs.
4. **Gadget evidence:** a live skill run for every affected model family and a
   live comparison showing the gadget surface does not regress the reference.
   Only then may a separate reviewed change flip the model-facing default.

No real model calls or hosted-model expenses are introduced by the runtime
benchmark. The scored live evaluations, their authorization and their results
must be recorded separately. A successful mock-model or no-op measurement
must never be labelled a passed live suite.

## Plugin removal: a release gate, not another immediate commit

As checked on **October 20, 2026**, the latest published release is **v0.3.0**,
published **October 16, 2026**, before the G2 warning merged on October 20.
Therefore no released deprecation cycle has elapsed yet.

First publish a release containing the migration command, warning and release
notes. Keep originals and the legacy loader for that release. Removal is a
later breaking-change PR after the published cycle, with migrated command,
agent, precedence and approval tests retained. Do not treat merging G2 as
publication of that deprecation release.

The runtime benchmark, live default gate and compatibility-release gate are
independent; none can silently clear either of the others.


### November 3: job-level Windows shell evidence remains independent

The November2 post-merge main run41398538418 passed its gadget-host and authoring/
discovery steps but failed the existing feature-off 2s partial-output shell test.
Its overall informational success is not all-platform/both-mode green evidence.
[windows-shell-fixtures.md](windows-shell-fixtures.md) records the original failure
and a test-input-only correction with three additive native Windows cases. Original
assertions/limits/production process behavior and all gadget runtime/guest/SDK/WIT
surfaces stay exact. Require every mandatory/setup step in all FOUR corrected-head
jobs, Windows included; no unchanged full retry or weaker timeout/assertion.
These are runtime/source verification gates, not live model quality, default-on,
published deprecation or release acceptance.


### Scoped test observations do not clear live or whole-run gates

The cfg(test) [shell observation adapter](shell-test-observations.md) adds only
bounded fixed facts to original failed fixtures for unresolved #138. Read-adapter
unit cases and selected native cases are not whole-suite/root-cause/live evidence.
Keep failed local full runs, actual raw observations and fresh exact-head all4-job/
every-step/both-mode/Windows/macOS15 results distinct. No retries, weaker original
assertions/deadlines, actual installed-gadget changes or release/default-on follow.


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
