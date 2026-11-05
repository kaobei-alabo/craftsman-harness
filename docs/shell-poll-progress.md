# Test-only original scoped-shell polling and selection phases

**November 4, 2026 (UTC): implemented diagnostic source; local validation is mixed,
exact-head hosted validation in progress.**
This follows the independently verified PR145 identity rebrand. It is NOT a
production root fix, new callable, public headless runner or release gate.
#138 remains OPEN; all earlier failed full-suite evidence remains FAILED.

## The missing observation

The existing actual-shell failure snapshots show native launch return and first
stdout/stderr polls with zero observed bytes. They do not distinguish one
original Pending poll from repeated Pending polls, nor show whether the existing
reader selection returned or its subsequent direct-child wait selection was
entered. Missing output/native flags/CPU/path labels still do not identify a
wait channel, shell startup stage, lost wakeup, interference or root cause.

`exec/shell/observations/progress.rs` supplies only these parent-side observations.
It is a child of the existing cfg(test)-only recorder, constructed through the
actual shared-builder Shell owner. No arbitrary command/path/PID/root/gate/seeds,
result/usage authority or production constructor is introduced.

## Original polling and branch facts

The existing reader adapter calls the ORIGINAL inner poll first, with its exact
Context/waker/buffer. Only afterwards, under the existing recorder update lock,
fixed fields retain total polls, actual Pending polls, last original call-clock
time and last result: pending/data/EOF/empty-capacity/error. No waker wrapper,
extra poll, raw byte/error text, new lock around inner polling or result repair.
Zero-capacity success is not EOF. Integer overflow becomes explicit unknown
(`None` plus loss), never a reset/refund/saturated apparent exact count.

Four cfg(test) enter/return hooks surround the existing reader and wait select
branches without changing any branch, bias, future, deadline or control flow.
Returned-ok/error/status and timeout/approval/other-stop are fixed ORIGINAL
selection observations. Entry is NOT proof a biased selection polled that inner
future; returned-ok is not unconditional EOF; a direct-child status is not
whole-group/all-role/IO quiescence, measured model usage or a public outcome.
First phase facts are retained; duplicate/missing-entry observations mark loss
without overwriting or inventing return history.

## Historical pre-stop copy and bounded output

The SAME Call copies progress with its original before-stop pipe facts, BEFORE
unchanged owned-tree kill/reap/drain. Later drain can add genuine polls/EOF to
current history but cannot rewrite the saved before-stop copy. Both are labelled
current-history/before-stop-history and use the original call clock. Existing
native active-view/active-query timing is NOT rebound, compared as if same-origin,
or reclassified fresh. This adds ZERO native queries/FDs/handles/samplers/signals.

The new progress section is WHOLE fixed ASCII/LF metadata <=4096 bytes or WHOLE
fixed refusal, never a clipped prefix. Existing reports and native saved-report
admission stay unchanged; a refused new section does not erase old facts. The
old13-query/8-thread/8-call/8-view bounds, one non-clone native ticket, original
PID/start/current-parent/group revalidation, released-native hiding and cached
teardown semantics remain intact. Reads/subqueries are not atomic and fixed
work limits are not absolute native latency/cancellation guarantees.

## Additive validation and remaining work

Ten portable pure cases and one macOS-only pure case cover original delegation/
waker/lock/buffer/results, empty capacity/errors, overflow/loss, phase history,
eviction, exact report ceiling/whole refusal and immutable pre-drain history.
An actual-builder native normal-output case runs both old formats with original
bytes/exit and verifies reader-return then wait-selection facts. A separate
macOS actual-builder one-second zero-byte timeout case runs both old formats and
checks pre-drain versus genuine later-drain history plus saved native reads
without another PID query. The latter is NOT startup/readiness/partial proof and
changes no older fixture's deadline. Pure metadata examples are not native process
or public receipt evidence; no model calls or installed gadgets are used.

The initial new-source compile failed with four E0624 errors on the NEW phase
hook declarations; no tests executed and no warning was suppressed. Only those
new method declarations now use the existing actual Shell owner's minimal
visibility. Existing authority types/fields/scopes/constructors and original bodies/assertions stay unchanged;
the failed initial compile is preserved separately from corrected validation.

Corrected focused validation passes ALL13 new cases in both feature modes on
macOS, without compiler warnings. The feature-off result precedes only canonical
formatting of the new declaration; the exact format projection is recorded,
not an unchanged retry. Both locked all-target checks, frozen branding/frame/
request/headless/local-storage contracts, framework dependency guard, offline
reference/driver contracts, formatting, lineage,214 local Markdown links and
old-source/history proofs pass. Local macOS is NOT Windows execution evidence;
the completed full results below remain separate, and all-four hosted gates remain pending.

All older source/assertions and20s readiness/15s invalid-byte/10s stop/6s partial/
Windows2s+3s/30s outer/2s drain remain exact. Production shell/capture/pump/process/
group/job/defaults and framework/SDK/WIT/deps/lock/pin/frontends/SafeBackend/v4/
standard ACP/schemas/goldens/trace/corpus remain unchanged. No user config/global
environment/SDK mutation, warmup, sampler, extra poll, generic/stale-PID lookup,
unchanged broad retry, agents/goals or independently blocked provider work.

A genuinely changed instrumented source may receive ONE separately classified
full local invocation per mode; keep every failure and never replace it with
focused/hosted green evidence. ALL FOUR fresh exact-final-head jobs/EVERY setup,
mandatory and post step/BOTH modes/actual macOS15+Windows/old+new inventories/
warning/source/tree/review proof precede a missing-formal-review-only diagnostic
merge. Root cause and public exec/usage/settlement/results/events/storage/trace/
provenance/headless callables/catalog/P2g/live/default-on/release remain separate,
unfinished and off. See [shell-test-observations.md](shell-test-observations.md),
[runtime-status.md](runtime-status.md) and [headless-mode.md](headless-mode.md).

## One full invocation per changed mode — November 4, 2026 UTC

Exact source76ab4c56720aaef8bb467ac7f672806d1a1ca34a; no unchanged full retry.
Both complete commands retain every old named/pass/ignored/doctest multiplicity
plus exactly13 new cases, warning-free, without outer-driver timeout.

- **Feature off: FAILED**, exit101, main1600 passed /4 failed /7 ignored;
  380.44s whole command. Twelve new cases pass; the NEW normal-output diagnostic
  fails at its existing15s call deadline. Three old real20s-readiness cases fail.
  All other test binaries complete; their passes do not make this run green.
- **Gadgets: passed**, exit0, main1747 /0 /9;81.15s whole command. All13 new
  cases and all old inventories pass. This genuinely changed-source pass NEVER
  erases the failed off run or earlier failures, or establishes a production fix.

Actual off failures:

```text
agent::shell_tests::actual_scoped_shell_poll_progress_retains_reader_return_then_wait_and_original_output
agent::filesystem_tests::same_typed_filesystem_failure_interrupts_an_actually_started_scoped_shell
agent::shell_tests::active_view_snapshot_is_not_queried_again_or_relabelled_fresh_before_stop
agent::shell_tests::actual_shared_builder_latch_stops_a_running_shell_and_prevents_later_spawn
```

All four failure reports retain ONE original Pending poll per pipe and zero
observed pipe bytes. The NEW normal-output case returns the original timeout
branch at15s, before any wait-selection entry; its saved pre-stop copy remains
distinct from current history. The three old readiness panics observe an active
reader selection with no recorded return and no wait-selection entry. These
identify the ORIGINAL parent-side phase only: not a wait channel/startup stage,
lost-wakeup/emission/CPU/cold-start/environment/OS-quiescence or root-cause proof.
No old/released PID is looked up. The old invalid-UTF8 case passes in both of
these invocations; its earlier15s zero-byte Timeout failures stay FAILED.

No assertion/deadline/new-fixture/production/reader/process workaround was made
to obtain the gadget pass or fix the new off failure. Documentation-only
validation follow-up keeps the exact same Rust source; no second broad run.
ALL FOUR fresh exact-final-head jobs/EVERY setup/mandatory/post step/BOTH modes/
actual macOS15+Windows/raw inventories/warning/source/tree/review proof remain
required before a diagnostic-only missing-review merge. #138 remains OPEN, and
public exec/usage/settlement/callables/P2g/live/default-on/release remain off.
