# Explicit owned-child native stack capture

**November 5, 2026 Asia/Taipei / UTC: implementation in progress for #138, not a
root-cause fix.** The user explicitly approved one bounded native stack capture
of a **newly owned failing child**, expanding the earlier no-sampling diagnostic
scope only for this controlled experiment. PR148 and independent main151d4bf
are all-four/every-step/both-mode verified; the retained local off1622/4/8 failure
remains FAILED and #138 stays OPEN. All sealed prior evidence is read-only.

## Narrow manual selection, not a shipped sampler

This app-only cfg(test)/macOS module has no production route/config flag/model
tool/ACP frame. A marked native test invocation selects only the exact original
`agent::shell_tests::actual_scoped_shell_poll_progress_retains_reader_return_then_wait_and_original_output`
libtest thread. That name selects a diagnostic fixture, NEVER a runtime provider
role, invocation permission or source authority. No global panic hook, timer
sampler, environment switch or process-name/PID search is installed. Normal CI
and unmarked tests never enable sampling. One atomic attempted budget is never
reset/refunded, including failed preparation/spawn/access/output/cleanup.

The private seam runs only AFTER the original native pre-stop snapshot and an
actual typed Timeout with zero observed bytes/no EOF/read error, before the
ORIGINAL owned-tree kill/reap. Only a borrow of that still-owned ProcessTree,
matching the SAME live Call's original PID/sequence, can select the target. No
caller PID, old log PID, released/evicted entry or saved report authorizes it.
The owner remains across tool completion/owned cleanup. A child may exit/exec,
and native operations cannot promise an absolute OS latency or cancellation bound.

## One bounded external tool, explicit perturbation

The fixed `/usr/bin/sample` tool receives an ACTUAL owned numeric PID, one-second
duration,1000ms interval, `-mayDie` and a fixed file in the caller's explicitly
owned private capture directory. No `-wait`, process-name lookup, `-fullPaths`,
TextEdit launch, sudo/task-port privilege workaround or alternative-tool retry.
Apple's installed `sample(1)` explains that sampling suspends and resumes threads.
This is therefore **perturbing evidence**, not passive observation; it may change
progress during capture and cannot prove the exact uninstrumented wait cause.

Whole argv admission:64 arguments/64KiB native bytes,16KiB strict UTF-8 marker
path; exact one marker, absolute normal components, private `capture` directory
under an owned-child-stack-capture evidence root. Source/trace artifacts are
create-only, no-follow regular/single-link0600; destination final metadata must
be owned0700, and retained file/path identity is checked. This manual same-user
experiment is NOT a portable adversarial namespace/content CAS/public publisher.

Tool execution5s plus owned direct-tool cleanup2s, stdout/stderr8KiB each/whole
refusal, trace1MiB and sampler-only RLIMIT_FSIZE1MiB. Limits never change the
parent, target, production shell launcher, clock, signal handling, environment,
SDK/user configuration, original15s/20s/10s/6s/Windows2s+3s/30s/2s or env20s+5s
criteria. The marked original call's elapsed observation includes capture time;
its original deadline is NOT reset and a genuine Timeout does not become success.
Actual tool/cleanup return, optional exit code AND original signal, trace size/hash
and held-target/path identity are recorded separately. None is fake exit/reap/
readiness, model usage, billing, EOF, permission, all-role/OS quiescence or P2g.

## Original metadata and production boundaries stay intact

The existing one non-clone native ticket, original phase/clock/pre-stop/current
history, final PID/start/current-parent/group checks,13-query/8-thread/call/view
limits and whole4096 ASCII-LF report remain byte-exact. The external approved
trace is a separate explicit experiment, not extra calls hidden in that native
metadata cap or a relabelled fresh snapshot. Source projection removes ONLY the
new cfgtest capture seam/declaration; ALL old assertions/helpers/pumps/wakers/
ProcessTree/defaults/production code remain unchanged.

Eight additive guards check exact marked/unmarked selection, bounds/invalid argv,
private create-only/identity/symlink/whole-size refusal, non-refunding budget and
cfgtest-only tool ownership/boundaries. The first NEW source guard failed only
because rustfmt split the atomic call before the dot; keep exact declaration and
compare_exchange values/order. Its next NEW predicate confused ExitStatus.signal()
READ observations with a signal handler; forbid actual libc signal/kill calls
while retaining real returned signal facts. Both7-pass/1-fail logs are retained
and corrected only in NEW guards; no old assertion/deadline weakening/suppression
or unchanged broad retry. Precompile new include-path/reborrow reviews are saved.

Before any capture, verify the marked full-suite native inventory is identical
to ordinary inventory plus eight new macOS cases, not zero selection or ignored
helpers. One genuinely changed-source full invocation per mode is allowed; only
feature-off is explicitly marked. The snapshot/trace must guide any fix, not
manufacture a pass or guess OS policy. Required current docs accompany this
incomplete implementation. Public exec/usage/settlement/callables/P2g/live/
default-on/release remain off; framework/SDK/WIT/deps/lock/Rust1.99.1/Wasmtime48.0.8/
workflow/frontends/SafeBackend/v4/standard ACP/schemas/goldens/trace and protected
deterministic GadgetCatalogMiddleware stay unchanged. No model/installed gadget/
user config/global env/SDK mutation, agents/goals or independently blocked provider
work. Fresh exact-final-head all-four/every-step/both-mode native/warning/source/
tree/inventory/review proof precedes review-only admin merge; independent main
proof precedes safe branch-preserving live FF. #138 must stay OPEN absent cause/fix
evidence.

See [native-pipe-snapshots.md](native-pipe-snapshots.md),
[shell-test-observations.md](shell-test-observations.md),
[legacy-shell-observations.md](legacy-shell-observations.md) and
[runtime-status.md](runtime-status.md). Primary local API source is the actual
Apple-installed `/usr/share/man/man1/sample.1`, retained verbatim/hash as private
evidence, not a newer portability/latency/security guarantee.


## Initial validation before any native capture

All eight additive macOS guards pass BOTH feature modes with exact selected
counts8/mode and no compiler warnings. Locked workspace/all-target checks pass
both modes. A single marked `--list` invocation confirms ALL1634 prior native
unit names/multiplicities plus8 additions (1642 total,0 benchmarks), without
executing test bodies, ignored helpers or any sampler. The owned capture
directory is still empty. This inventory check is not replacement full-suite
coverage, startup readiness, a warmup-to-pass or root-fix proof.

The first evidence-parser assertion strictly refused an interleaved CI header:
framework stdout appeared after the app stderr Running header. Pinning the
actual `running1634 tests` start isolates the same exact original unit inventory;
no test source/assertion/deadline or capture selection changed. The original
7/1 NEW source-guard failures remain retained; fixes are NEW guards only, and
actual tool/cleanup return plus real signal facts are explicit. Final Rust
aggregate before full capture validation is
`8691fef4294810e70f5080e5f8f36be13804b8199b99211b36eb9cd3a7d1942d`.
Source projection preserves365 unrelated old files, ALL original assertions/
helpers and production logic, and the original metadata13-query/8-thread/call/
view/4096-byte ticket/clock/identity code. Only the new cfgtest seam and appended
private declaration remain. All prior evidence is read-only; no old PID query.


## Completed one-capture experiment — November 5, 2026

The explicit marked changed-source invocation at
`4952baa189a3088f6abed8572aacd013ce750350` reproduced the SAME four original
feature-off failures: **1630 passed /4 failed /8 ignored**, exit101,429.47s
whole command. The unmarked gadget-host command passed1777/0/10, exit0,83.77s.
All eight additions and every original environment/auth case pass both modes;
compiler warnings0. These results do not resolve #138.

### Marker transport correction and truthful coverage

The first marked command used an empty free filter plus the marker. Libtest
selected every unit/integration test, but rustdoc interpreted the extra free
marker as a filter and excluded BOTH original doctest cases. That first command
remains a FAILED diagnostic/native-integration invocation, **NOT single-command
complete doctest coverage**. A targeted unmarked `--workspace --doc` companion
ran the original SDK doctest and original ignored macro doctest with their
unchanged assertions/reason. Exact combined outcomes/names/pass/ignored/doctest
multiplicities match the prior complete inventory plus8, except the four explicit
off failures. No unchanged broad/full retry or second sampler was run.

For any future explicitly authorized capture, transport the exact marker as a
**nonmatching `--skip` option VALUE**, not a free test filter. Metadata-only
`--list --skip <marker>` checks preserve every1642 native unit and BOTH doctest
names without executing a sampler or any test body. This corrects the new
diagnostic command's filtering defect; it never skips an actual test name.
The source parser accepts that same exact marker token without runtime/config
changes. Any later capture requires fresh explicit scope/ownership, not reuse
of the old PID or current already-consumed experiment directory.

### One real trace, bounded and owned

Exactly ONE sampler invocation succeeded while the original owned child/Call
was still held before its original kill. Actual tool return: exit0/no signal,
1.127321s elapsed; source call entered at15.003449s and returned at16.130770s
on the ORIGINAL unchanged call clock/deadline. The859-byte trace and its retained
private0600 path/identity/hash match the same actual target in original native
pre-stop history; no old PID lookup/second capture/privilege fallback occurred.
The original15s timeout/assertion remains FAILED, never rescued into success.

The report identifies an ARM64 system-sh child and ONE frame:
`_dyld_start (in dyld) + 0`. Binary-image description is unavailable, and the
actual `_sigtramp` symbol-lookup warning is retained. The original snapshot still
has one Pending read per pipe, zero current fd1/2 buffer data, kernel status2,
one waiting-thread native state3 and Unix/Mach syscall counts0/0. This places
the sampled userspace PC at loader entry, consistent with an early-launch hold,
NOT a parent reader-loop stack. It does not reveal the kernel wait event or
prove the exact uninstrumented cause; sampling itself suspends/resumes threads.
No wall/CPU/startup/EOF/usage/authority inference is substituted for observations.

Apple's retained official `dyldStartup.s` initializes the dynamic-process entry
there and branches into loader startup. Its current public `kern_exec.c`
START_SUSPENDED path sets BSD SSTOP; the recorded status2 is not that named
path, and current public source is not the exact26.6.2 host implementation.
Neither that source nor the one PC sample justifies SIGCONT, launch-flag,
scheduling, environment, reader/waker, timeout or permission workarounds.

The exact installed Rust compiler remains1.99.1/commit58cf8f9c35b355199727627d07ae34a4e2846d67/
aarch64-apple-darwin/LLVM23.1.1. Retrieval of the guessed official1.99.1 process
source path failed and is retained as insufficient evidence, not proof of actual
spawn flags or an excuse to alter the pin/dependencies. No unsupported root fix
is claimed. #138 stays OPEN; the external/native wait cause needs further
authorized evidence. README/current feature docs preserve the incomplete state,
and all public/release/usage/settlement/callable/P2g/live gates remain off.
