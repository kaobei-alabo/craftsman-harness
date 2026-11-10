# Existing Windows shell timeout fixtures

**Status: November 3, 2026 (Asia/Taipei): narrow test-input correction with
additive Windows coverage; all-platform, both-mode verification remains a merge
gate.** This is maintenance of the existing shell tests, not a new headless
command, OS sandbox, usage ledger or live-model acceptance.

## Observed post-merge failure

PR #136's exact source head `71e33219134920ceb450fc65c02eee6e650273b6`
passed all four jobs in CI run `41395263361`. Its merged main commit
`4cca02814cab37a7fe4637acbedd714aeb676e74` then ran separately as
`41398538418` on November 2, 2026. Preserve those as distinct observations:
the post-merge Windows job `125125072743` failed its feature-off **Test** step.

`tools::tests::head_tail_timeout_returns_the_partial_output` reached the real
2-second timeout without either expected stream in the retained output. The
unit result was 1524 passed, 1 failed and 7 ignored. The later gadget-enabled
unit suite passed 1657 cases with no failures and 9 ignored, and its integration
suites also passed. That does not make the failed feature-off step green.
Formatting, Linux and actual macOS15 passed. Windows is still informational in
[ci.yml](../.github/workflows/ci.yml), so the overall run reported success despite
the failed Windows job. Inspect job AND step conclusions, not only the badge.

The actual failure log/metadata are retained separately from the earlier green
PR evidence. No unchanged test/CI retry, assertion reduction or deadline increase
may replace that failure with a claimed pass. The log establishes missing output
before timeout; it does **not** profile whether process startup, pipeline work or
host scheduling caused the delay.

## Narrow fixture correction

Only the Windows `PRINT_THEN_HANG` test input in
[tools.rs](../crates/craftsman-rs/src/tools.rs) changes. It writes the original
`before-the-timeout` stdout and `also-stderr` stderr through direct Console
writers, explicitly flushes each, then blocks with `Thread.Sleep(60000)`.
This removes the fixture's `Write-Output` pipeline and unrelated native `ping`
child. It does not promise a universal Windows cold-start latency.

The Unix command, original legacy/head-tail test bodies and assertions, and the
separate Windows grandchild/Job Object test are unchanged. Preserve the real
2-second call limit, 30-second outer native-test bound and existing 2-second
post-kill drain. No warmup, new readiness callback, retry, synthetic output,
production timer start change or alternate process launcher is added.

`ShellRunner`, `execute_impl`, capture/formatting, `ProcessTree`, environment
scrubbing and production defaults remain byte-exact. The fixture still goes
through the actual PowerShell/process-tree path, not a fake process result.
All framework/SDK/WIT/provider/input/publisher/guest/frontend/permission sources,
dependencies/lock/Rust pin, workflow/check names, schemas/goldens/trace and old
corpus stay unchanged.

## Additive Windows cases

Three new cases in `tools::tests::windows_timeout_fixture` select the SAME
existing runner and fixture:

1. A fast input contract checks that stdout write/flush and stderr write/flush
   precede the long direct sleep, without a cmdlet or native child.
2. Actual calls in **both** `legacy` and `head_tail` formats retain exactly
   33 original Windows bytes (20 stdout + 13 stderr, including CRLF), a real
   duration and typed Timeout, with no invented exit code. Legacy still drops
   the output text and records that truncation; head-tail returns BOTH original
   stream markers with the unchanged timeout wording.
3. A deliberate 60-second block **before** the first write must time out with
   zero actually observed bytes, no fabricated markers/exit/completion and the
   unchanged empty-output wording in both formats. It is not a ready-output proof.

These cases run only on native Windows; a macOS/Linux source check or a filtered
run with zero selected cases is not Windows execution evidence. The old tests
still run on every supported platform under both CI feature modes. Original byte
counts are shell observations, not model tokens, billing, delivery receipts or
OS/all-role quiescence.

## Verification and remaining boundaries

Require fresh exact-final-head fmt plus Linux, **actual macOS15** and Windows jobs,
every mandatory/setup step and both feature modes before a review-only admin
merge. Keep the retained `test (macos-14)` required-check name; it is not macOS14
coverage. Preserve pre-correction failures and audit old Windows warning
signatures/counts separately; no allow suppression or workflow weakening.

This does not change the private six-second/ready-marker headless shell fixture,
source approval, gadget discovery or guest deadlines. See
[gadgets.md](gadgets.md), [gadget-validation.md](gadget-validation.md) and
[headless-mode.md](headless-mode.md). Accepted R1–R7/#97 remain accepted; usage and
reservation settlement, successful-response publication, result/events/stdio,
redacted persistence/trace, callable headless gadgets and P2g remain separate.
Do not inspect or bypass the independently blocked provider-response correction.
Public `craftsman exec`, live/default-on and release acceptance remain unavailable.

## API references

The fixture uses established .NET APIs available to Windows PowerShell; these
references describe the operations, not a measured startup guarantee:

- [Console.Out: standard output writer](https://learn.microsoft.com/en-us/dotnet/api/system.console.out)
- [TextWriter.Flush: flush buffered writer data](https://learn.microsoft.com/en-us/dotnet/api/system.io.textwriter.flush)
- [Thread.Sleep: block for the specified milliseconds](https://learn.microsoft.com/en-us/dotnet/api/system.threading.thread.sleep)
- [Write-Output: PowerShell pipeline output](https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.utility/write-output?view=powershell-5.1)


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
call/user config/env/SDK mutation or blocked-provider work. the earlier full-run failure remains recorded;
public exec/usage/settlement/callables/P2g/live/default-on/release remain off.
