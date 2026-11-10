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
call/user config/env/SDK mutation or blocked-provider work.
public exec/usage/settlement/callables/P2g/live/default-on/release remain off.
