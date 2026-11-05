# Scoped native-shell test observations

**Status: November 3, 2026 (Asia/Taipei): test-only diagnostic implementation for
#138, not a root-cause fix or production tracing.** The original local macOS
full runs remain failed:1547/3/7 off and1690/3/9 gadgets (passed/failed/ignored).
Their saved evidence is independent of #137's passing PR/main CI and focused
single/concurrent/mixed diagnostic passes; those passes never replace full runs.

## What the next observation must distinguish

The original filesystem/shell fixtures did not see real readiness within20s,
and the original invalid-UTF-8 fixture returned Timeout rather than Failed at15s.
The logs alone do not tell whether launch preparation/native process creation,
child execution or pipe consumption was delayed. No cause is declared from
missing output, isolated passes or a green workflow badge.

Only the actual shared-builder private `Shell` creates its recorder. A view
queries that installed recorder through the existing `ShellRunner`; it does not
construct filesystem/gate/provider authority or accept caller commands, paths,
keys, roles, errors or PIDs. The new module is entirely `cfg(test)` and absent
from runnable agent builds.

Each call records monotonic elapsed times around ORIGINAL launch preparation
(including `headless_spawn_options`) and actual `ProcessTree::spawn_shell` return,
the actual optional child PID, and scoped-reader observations. A returned PID is
NOT a witness that the child executed the shell, became ready or finished.

The reader adapter delegates the exact inner `poll_read` FIRST, preserving its
buffer, waker, Pending/Ok/Err result and cancellation path. Only afterwards does
it record first poll/chunk, actually added byte counts, actual EOF or read error.
Zero-capacity successful polls do not invent EOF. No original byte or error text
is retained, and the original capture/UTF-8/redaction/pump logic is unchanged.
Scope release is just scope release, not normal return/cancellation/completion,
all-role quiescence, measured model usage or an OS rollback guarantee.

## Bounded failure-only fixture reports

A recorder retains at most8 call entries; fixed fields replace an event/chunk
log, so firehose reads do not create unbounded metadata. A fixture holds at most8
actual-shell views. On an ORIGINAL assertion panic, fixture teardown reports
only fixed ASCII labels, booleans and integer/optional-integer observations.
It never installs a global panic hook, modifies environment/current directory,
opens real configuration or writes a spill/journal/trace/result file.

The same original ready20s, invalid-byte15s, post-stop10s, call limits and all old
assertions remain. There is no warmup, retry, fake marker, synthesized bytes,
alternate launcher, clock reset or changed production process/group/job/signal
policy. Observations themselves may affect test timing and cannot be treated as
a fix if a newly instrumented whole run happens to pass.

## Validation / remaining work

Independent adapter cases exercise delegated original bytes/errors, no false
EOF, bounded eviction and literal-free fixed diagnostics. Actual-builder cases
check genuine native launch/read facts and denial without a fabricated launch.
Filtered/fake-reader tests are not native full-suite or Windows coverage.

A genuinely changed, instrumented source may receive one separately classified
full validation per feature mode to obtain original failure observations. Keep
all earlier failures/snapshots; do not retry an unchanged broad run to force green.
Require all FOUR fresh exact-head hosted jobs/every mandatory/setup step/both
modes/actual macOS15/Windows before a review-only merge; legacy `test (macos-14)`
is only the required context name. No failed/skipped check or substantive review
is bypassed. Root cause remains unproven until real observations support it.

This does not implement public `craftsman exec`, actual usage/settlement, result/
events/storage/trace, callable headless gadgets, P2g, live/default-on or release
acceptance. R1–R7/#97 acceptance is unchanged and independently blocked provider
work is not inspected or bypassed. Framework/SDK/WIT/dependencies/lock/Rust pin,
frontends/SafeBackend/v4/standard ACP/schemas/goldens/trace/corpus remain unchanged.
See [runtime-status.md](runtime-status.md), [headless-mode.md](headless-mode.md)
and [windows-shell-fixtures.md](windows-shell-fixtures.md).


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


## Native task/thread failure snapshot refinement

PR139's diagnostic-only slice merged on November3 after exact source/head/tree,
all FOUR hosted jobs/every setup/mandatory/post step/BOTH modes and warning/
old-inventory audits. Its latest refined local full off run still failed1554/2/7
(exit101); latest-head local full gadget mode was not run. These results remain
separate from first diagnostic-head1554/2/7 off and1697/2/9 gadget failures and
the earlier PR137 failures. The original issue remains OPEN; no root cause is
inferred from hosted/focused green coverage.

The next macOS-only `observations/native.rs` replaces the failure-time BSD-only
query with `PROC_PIDTASKALLINFO`, checking its COMPLETE exact native struct return
before any field access and ORIGINAL PID/current parent/process-group identity.
Only the fixed image class/status/flags and original task user/system/existing-
thread time, thread/running counts, context switches, Mach/Unix syscall counts,
faults and pageins are rendered. Signed/unsigned native integers stay original:
no percentage, delta, wall-time conversion, usage/billing or counter reset.

A single `PROC_PIDLISTTHREADS` query has a fixed initialized9-u64 scratch buffer.
At most8 entries are admitted; an extra slot detects an overbound result rather
than rendering a clipped "complete" set. Zero/short/nonmultiple/oversize/error
returns are unavailable and duplicate keys ambiguous. The returned pthread keys
are paired with `PROC_PIDTHREADINFO`, NOT the distinct thread-id64 flavor. Keys
are transient lookup data only, never retained in the report or exposed as caller
authority. Each complete thread struct contributes ONLY original native user/
system time, scaled CPU value, run state, flags and sleep time; names are never
copied into facts. At most8 fixed-fact records, no event/stack/wait-channel log.

After ALL subqueries (success OR error), another complete BSD query must match
the original PID/start/current-parent/group. Otherwise ALL thread facts are
discarded for fixed `changed-or-unavailable`, without erasing the earlier genuine
task observation. These reads are not atomic, a PID-reuse/exec/progress guarantee
or all-role/process quiescence. Native CPU/run-state facts do not by themselves
prove spinning, sleeping cause, lost wakeup, cold startup or interference.

The maximum is11 native calls per active recorded entry (one task, one list,
eight threads, one revalidation), <=8 entries/view and <=8 fixture views. Calls
run outside the recorder mutex and only at original assertion-panic teardown,
except the NEW actual-builder diagnostic test which queries its own active view.
No sampler thread, signal, sleep, timer, retry, new launcher, environment/cwd
change, raw name/pthread-address/path/argv/byte/error retention or spill exists.
A fixed call/work bound is not an absolute OS-syscall latency guarantee.

Six additive macOS metadata cases protect native return bounds, original child
identity/start, list ceiling/duplicates, exact signed/unsigned values/name-free
framing and discarding changed subqueries. One additive macOS actual-builder
case checks a genuine ready native child, original byte/typed stop facts and
that a RELEASED call view performs no further PID query. Other OS builds keep
all old named cases unchanged and have no native module. Old test bodies,
assertions, fixtures,20s/15s/10s bounds, capture/pump/process/job/group/defaults
and runnable production source remain exact. New-case/focused/native evidence
never replaces failed full-suite coverage, and unchanged broad runs are not
retried to manufacture green. All four fresh exact-head jobs/every step/both
modes/actual macOS15/Windows and source/warning/tree proofs still precede any
missing-review-only merge. Documentation accompanies this incomplete refinement.


## Native executable and retained-workspace CWD classification

PR140's merged-main41631110649 has now independently passed every job/step/mode,
with exact PR-tested tree, native macOS15/Windows labels, case inventories and
unchanged warning audit. That does NOT replace the preserved failed full local
1562/1/7 off and1704/2/9 gadget runs. #138 remains OPEN; low-CPU/waiting snapshots
have not identified the child wait channel/cause, actual exec completion or fix.

The next macOS-only TEST module `native/execution.rs` distinguishes two concrete
remaining alternatives without trusting the process display name or reopening a
caller path. The actual private Shell builds its recorder from its own opaque
ShellScope. It captures only original device/inode after cloning/querying the
SAME Prepared ConfinedFs directory handle through existing discovery_directory;
no ambient stat/path reopen/alias normalization, extra grant or authority. Close
the temporary clone immediately; the read-only view retains no path or handle.
Non-directory/zero-inode/unrepresentable32-bit native device fails unavailable,
never truncates a larger identifier to manufacture equality. This original
identity stays frozen across later namespace changes; it is diagnostic data,
NOT a shell filesystem confinement or publication/session capability.

For an active original recorded PID, one `proc_pidpath` uses an initialized4096-
byte buffer. Reviewed Apple's public libproc returns strlen EXCLUDING NUL. Admit
only positive length<buffer, exact NUL at that position, no earlier NUL and an
absolute whole path. Exact `/bin/sh`, `/bin/bash`, `/bin/zsh`, `/bin/dash` and
`/bin/sleep` map to fixed system-sh/bash/zsh/dash/sleep labels; every other valid
whole byte path maps to other, never a repaired/normalized/known-prefix alias.
Other is NOT unsafe/untrusted/wrong image evidence (e.g. a valid alternate system
layout may differ). Error/incomplete/invalid results are unavailable. Raw path
bytes exist only in fixed native scratch and never enter retained facts/reports.
Path identity is not a code hash, approval, executed-command or exec-completion
witness; do not choose a launcher, policy, status or usage from its label.

One complete exact `PROC_PIDVNODEPATHINFO` contributes only directory mode and
original native device/inode comparison to the captured invocation identity.
Embedded CWD/root path arrays and UID/GID/other fields are ignored. Fixed labels:
matches-invocation-workspace, different-from-invocation-workspace, unavailable or
expected-workspace-unavailable. No actual device/inode/path is rendered. The
ORIGINAL PID/start/current-parent/group revalidation AFTER all native subqueries
(success OR error) gates ALL new execution facts and existing thread facts;
mismatch/unavailability discards them fixed while preserving earlier genuine
task observation. These observations are not atomic/namespace CAS/PID-reuse/
exec/wait-channel/cause/OS containment or all-role quiescence guarantees.

The maximum becomes13 native queries/active entry (task, executable path, CWD,
list,<=8 threads,final BSD revalidation), still8 retained calls/view and8 fixture
views. Separate construction work is one retained-directory clone/metadata/close
only in cfg(test)/macOS, not runnable production. No sampling thread, signal,
sleep, retry, warmed launcher, timer/clock reset, raw argv/environment/name/error
log, spill or public producer exists. Native calls cannot promise absolute OS
interruptibility/latency. The original reader/buffer/waker/return/capture/pump/
process/group/job/permission/default semantics and EVERY old fixture/test body/
assertion/20s readiness/15s invalid-byte/10s stop bound stay exact.

Five new macOS metadata tests protect full native length/NUL/bounds, exact class
and unknown byte handling, directory/device/inode relations and representability,
plus fixed-framing discard on final identity failure. Two new actual-builder
cases require a REAL ready native child, original23-byte/typed NeedsApproval
facts and no released-PID lookup. One case moves ONLY its owned temporary root
and creates a new directory at the same name AFTER original scope construction:
real child CWD must compare DIFFERENT from the captured retained identity, not
be rebound by ambient stat. This explicitly demonstrates unchanged shell path
semantics, NOT added confinement or a simulated authority rebind. Old source and
assertions are untouched. Seven additions apply to macOS only; other builds keep
prior inventories with no new native module. Document every partial refinement,
preserve all failed evidence and require all FOUR exact-head jobs/every step/
both modes/source/warning/tree proof before missing-formal-review-only merge;
public exec/usage/settlement/callables/P2g/live/release/provider gates stay off.


## Shared one-shot native snapshot before original kill/reap

**November3,2026 UTC: another private TEST-only refinement, not a root fix.**
PR141's merged-main41647609134 is independently all-four-job/every setup/
mandatory/post step/both-mode/actual macOS15+Windows/source/tree/old-inventory/
warning verified. Its local full1566/4/7 off and1709/4/9 gadgets remain FAILED,
including NEW integration readiness failures; focused/hosted passes never replace
them. BOTH original invalid-byte cases returned Timeout at15s with0 received
bytes, scope release and EOFs from original timeout teardown. Looking up their
old PID later is invalid. The missing observation must happen while the actual
local tree is still owned, BEFORE its original stop kill/reap.

One cfg(all(test,macOS)) call now sits INSIDE the original stop.is_some branch,
immediately BEFORE unchanged ProcessTree::kill_tree. It takes ONLY the actual
local tree and original stop decision, through the installed Call; no caller
PID/root/gate/status/seed constructor or permission/execution authority. Stop
kind (timeout/needs-approval/failed/other), original call-clock entry time, actual
current tree-ID presence/equality and copied ORIGINAL observed stdout/stderr
poll/chunk/bytes/EOF/error facts are recorded before drain. Missing stop or an
already released/evicted call does no work. Missing/mismatched current tree ID
records fixed no-query facts, NEVER falls back to the saved old PID.

`snapshots.rs` linearizes ONE non-clone native query ticket per retained call.
Existing active-view observations and pre-stop share it: reserve under bounded
recorder mutex, perform the unchanged<=13-query native task/executable/CWD/thread/
PID-start-parent-group validation OUTSIDE that mutex, then record actual return.
No await/inner poll/native call/sleep under the lock. Repeated pre-stop/active/
teardown reads do not query again or refund/rebind a lost/evicted ticket. Native
snapshots captured earlier stay labelled active-view, NOT a new pre-stop sample.
Their monotonic active-query clock begins0; before-stop enter/return use ORIGINAL
call clock. Printed native_snapshot_clock distinguishes both, so never subtract
incomparable origins or infer wall-time/rate/exec/wait cause/quiescence/usage.

Retain ONLY whole fixed ASCII/LF native fact text<=4096 bytes (still<=8 calls/view
and8 fixture views), or whole fixed refusal for size/control/non-ASCII violation;
no clipping/raw error echo/path/name/argv/environment/byte text/source spill. A
pending ticket is labelled pending/return None, not completion/cancellation or
measured usage. Native query count1 is a historical diagnostic attempt, NOT a
wire/model/request/billing count or proof all13 native calls ran. Original task
facts and native identity/limits keep their documented non-atomic/PID-reuse/
blocking-OS-latency limits; cache cannot promise absolute native interruptibility.

The existing report() remains its original fixed launch/read/scope view.
failure_report() still includes native facts ONLY while a recorded call is
ACTIVE; released-call native data remains hidden and no PID is looked up. Active
reads are now clearly labelled first-snapshot history/phase/clock. ONLY the NEW
teardown_report() (used by ORIGINAL fixture-panic Drop and explicit TEST cases)
can show bounded cached native facts after scope release, with pre-stop pipe
facts kept distinct from later genuine drain EOF/bytes. A successful normal
call invents no stop/snapshot. This is not a public writer/trace/result/receipt
source and does not modify gate/stop/outcome/status/authorization semantics.

Eight additive macOS pure cases protect one shared attempt, original kind,
missing/mismatched ID/no stop, released/evicted non-rebinding, whole safe-text
cap/refusal, original pre-drain pipe facts, pending/eviction and compatibility of
active versus teardown views. Four additive actual-builder cases protect real
owned-child Timeout and saved data after release, real approved-start/typed
NeedsApproval/original bytes, prior active snapshot without duplicate/relabel,
and normal success without invented stop. The NEW1s timeout fixture requires
no ready/partial-output/exec proof and NEVER changes the OLD15s invalid-byte or
6s partial fixture/20s readiness/10s post-stop limits. New actual readiness/stop
cases retain20s/10s; all older Rust bodies/assertions/fixtures stay exact.

Runnable production/capture/pump/process/group/job/default/signal behavior stays
byte-exact after cfgtest hooks; no warmup/retry/new launcher/global panic hook,
clock reset/sampler/signal/sleep/model call or user config/env/SDK mutation. Hook
adds TEST-only post-decision native overhead, not rollback/OS-latency guarantee
or a fix if a differently instrumented full run passes. Documentation accompanies
this unfinished implementation; require all FOUR fresh exact-head jobs/every
setup/mandatory/post step/both modes/actual macOS15/Windows and raw old+new case/
warning/source/tree proof before missing-formal-review-only merge. Preserve all
failed full results, public exec/usage/settlement/callables/P2g/live/release and
the independently blocked provider work remain separate/off.


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


## Original reader-poll and select-phase follow-up — November 4, 2026 UTC

PR145's identity rebrand is independently all-four/every-step/both-mode verified
on merged-main41808669282; its local1585/6/7 off and1728/6/9 gadget invocations
remain FAILED, with five native failures unresolved and the separate exact
renamed-token expectation corrected only. #138 stays OPEN.

The next bounded cfg(test) slice adds actual inner-poll count/Pending count/last
fixed classification/time and existing reader/wait select enter/return facts.
It preserves the exact original Context/waker/buffer/result and adds no poll,
native query, FD duplication, sampler, signal, deadline/assertion/launcher or
production change. Pre-stop progress is copied once and remains separate from
later drain/current history. Whole4096-byte new ASCII/LF section or whole fixed
refusal; the old native snapshot cap/ticket/phase/clock/revalidation and all old
reports/assertions stay intact. See [shell-poll-progress.md](shell-poll-progress.md)
for implementation, additive validation and non-claims. This is not a wait-cause
or root-fix conclusion, replacement full coverage or public producer approval.


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
