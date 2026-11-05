# Immutable environment test fixtures

**November 4, 2026 (UTC): corrective implementation in progress.** This fixes
unsafe parallel test-global environment mutation, not yet the established cause
or complete fix of the unresolved native-shell issue #138. All earlier local
failures remain FAILED, including PR146's new normal-output diagnostic case.

## The concrete safety defect

Five existing test bodies called process-global `std::env::set_var/remove_var`
while other libtest/Tokio/native/server work could read the environment. Unique
synthetic variable names and a claim of sole writer ownership are insufficient:
Rust documents that Unix callers cannot rely on thread-safe foreign environment
access (including DNS), and recommends `Command::env` for child processes. A
new lock around only these setters would not serialize all foreign readers.

## Same assertions, immutable child environments

The four config/MCP/route cases keep their original names and complete existing
assertion bodies. Only unsafe setup/cleanup is removed. A fixed registered argv
marker chooses the same body in one actual source-built child test process;
exact synthetic values and missing-variable conditions are supplied with
Command.env/env_remove BEFORE process startup. Neither parent nor child mutates
its running environment. No ambient user configuration/stdin/SDK is inspected.
Unmarked/unknown/duplicate helpers return before environment/body inspection.
Successful native exit PLUS an exact completion marker written only AFTER all
original assertions prevents no-op/filtered helpers from being passing evidence.
Output is strict UTF-8/whole64KiB per pipe, with one20s test execution and5s owned
cleanup bounds; no unchanged retries, fake shell readiness, or production limit
change. A new ignored helper is fixture infrastructure, not a callable/CLI flag.

## Actual per-router auth source, not a manufactured auth result

The old concurrent serve protocol/auth test no longer changes the global proxy
secret mid-server. A cfg(test)-only source is bound to that actual router state;
its short mutex is released before request work. It starts absent and switches
to the same synthetic secret, retaining every real HTTP/WS/status/frame assertion.
The same existing absent/empty/lossy/header/constant-time comparison is reused.
Normal routers and the unchanged public authorized(headers) function still read
the original environment source; no auth default, protocol frame, route, policy,
capability or model behavior changes. Immutable-child present/absent/empty cases
independently exercise the REAL production environment adapter. Two independent
real routers protect source isolation without walking a real home directory.

## Limits and validation

All old shell/filesystem/guest/input/provider/assertion/SDK/WIT/native/process/
reader/clock limits remain unchanged, including20s readiness/15s invalid byte/
10s stop/6s partial/Windows2s+3s/30s outer/2s drain. No model call, installed gadget,
user/global env/SDK mutation, sampler/extra shell poll/new shell launcher, agent/
goal, warning suppression or independently blocked provider work. Framework,
deps/lock/Rust1.99.1/Wasmtime48.0.8/frontends/SafeBackend/desktop-v4/standard ACP/
schemas/goldens/trace/corpus remain unchanged. Documentation accompanies the
partial correction; private/public exec/usage/settlement/callable/headless catalog/
P2g/live/default-on/release gates stay separate and off.

Fresh genuinely changed-source validation must preserve every old assertion,
case name/multiplicity/ignored/doctest and disclose any old OR new failure. ONE
full per mode, not an unchanged retry; all FOUR exact-head jobs/EVERY setup,
mandatory and post step/both modes/actual macOS15+Windows/warnings/source/tree/
review proof precede missing-formal-review-only merge. A changed-source green
run does not by itself prove #138's cause or erase previous failures.

## Initial local validation — November 4, 2026 UTC

Both locked all-target checks pass without compiler warnings. In BOTH modes,
five old assertion cases plus five new main cases pass (10 total); one new
marker-only native helper remains intentionally ignored in normal enumeration
but is executed by the actual child fixtures. Three offline guards and60
branding/frame/request/headless/local-storage cases, framework dependency guard,
offline reference/9 driver contracts, formatting, lineage and225 Markdown links
pass. A source-wide proof restores the five original files byte-exact after
ONLY the declared child-env setup/cleanup/source-extraction changes are removed;
every old assertion/auth comparison and355 unrelated original files remain
exact. Shell/IO/runtime/SDK/protocol limits and signatures are unchanged.

The first source-audit utility failed because its inverse removal of a new
cfgtest footer also removed the original terminal LF. The original utility and
exact one-LF diff are preserved; correct only that inverse projection, not
repository code/assertions or a weakened verifier. Full changed-source and
ALL FOUR exact-final-head native CI jobs/every step/both mode/old+new inventory/
warning/source/tree/review gates remain pending. Local checks are NOT Windows
execution, #138 root-cause proof or replacement for earlier failed full runs.

## One full run per changed mode — November 4, 2026 UTC

Exact corrective sourceef9f1defcd88e6af412ddbe73cc32a1b1b98cfb8; no unchanged
broad retry. Both whole commands complete warning-free, with every old named/
ignored/doctest multiplicity plus five new main passes, three offline guards and
one new explicitly ignored marker-only fixture preserved. The five ORIGINAL
environment/protocol assertion cases and all eight new substantive cases pass
BOTH modes.

- **Feature off remains FAILED:** main1605 passed /4 failed /8 ignored, exit101;
  415.63s whole command. The same PR146 normal-output diagnostic and three old
  native-readiness cases fail; no new environment/assertion case fails.
- **Gadgets passes:**1752 /0 /10, exit0;88.24s whole command. This pass does NOT
  replace the failed off result or any earlier failure.

These results prove the concrete unsafe mutations are eliminated while their
original assertions still run. They do NOT establish those mutations as #138's
sole cause or resolve the observed shell stall: the same off failures persist
without them. No old PID is looked up and no deadline/assertion/production/
fixture workaround is made. #138 remains OPEN; later native wait/startup cause
work must use new real bounded observations, not warmed/broad unchanged retries.
Documentation-only follow-up keeps the same Rust source and does not rerun full
tests. ALL FOUR exact-final-head hosted jobs/every step/both modes/native labels/
old+new inventories/original Windows9 warning signatures17 occurrences/source/
tree/review proof still gate merge; public exec/usage/settlement/callables/P2g/
live/default-on/release and blocked-provider work stay separate/off.


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
