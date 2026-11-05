# Read-only native child-pipe snapshots

**November 5, 2026 Asia/Taipei / November 4 UTC: diagnostic implementation in
progress for #138, not a root-cause fix or public runtime feature.** PR147 and its
independent merged main86704e4 are all-four/every-step/both-mode verified; the
complete local feature-off1613/4/8 failure remains FAILED. The same four scoped
shell cases retain successful spawn returns, one original Pending read per pipe,
zero observed bytes and no wait-selection return. Those facts alone do not say
whether bytes are buffered but unread or the child has not emitted anything.

## The discriminating observation

Only the existing cfg(test)/macOS native failure snapshot now inspects the
**CURRENT owned child's descriptors1 and2** with `proc_pidfdinfo` and the public
`PROC_PIDFDPIPEINFO` flavor. It does not open/duplicate/read/consume a pipe, poll
again, replace a waker, change a launcher/environment/clock, sample or signal.
The old ProcessTree/readers/pump/args/output/assertions and production code stay
unchanged. No user configuration/home/SDK is inspected or mutated.

The exact complete public native struct must match before any field is read.
The existing libc `vinfo_stat` layout is reused; its missing wrapper types are
copied as cfgtest-only `repr(C)` integer structs, checked against a small native
C program compiled from the host's actual public `sys/proc_info.h` with
`-Wall -Wextra -Werror`. The locked libc/dependencies/features/pins do not change.
Validate pipe type, write grant, FIFO stat and positive native capacity; negative
buffer counts or any short/error/oversized native return refuse, never become
zero. Retain only native buffer-byte count/capacity/open flags/pipe status, not
opaque handles, names, paths, ids, timestamps, raw bytes or error text.

Apple's public kernel fill uses the peer's buffer count for a write descriptor
and reports it in `vst_size`; this is a metadata snapshot, NOT bytes read by the
harness, model usage, token/receipt counts, cumulative emission or EOF. Descriptors
can be rebound by the child, and two queries are not atomic. The record is
explicitly CURRENT-child-fd metadata: never substitute it for original pipeline
identity, authority, delivery/quiescence or startup/wait-cause proof. A nonzero
count helps investigate unread data in that current pipe; zero is not proof
that no bytes were ever emitted or another reader never consumed them.

## The SAME bounds, ticket and final identity gate

The original one non-clone active/pre-kill native ticket remains shared, with the
same original phase/clock and final original PID/start/current-parent/group
revalidation. All new pipe facts are discarded if that final check fails, even
after a native error. No old/reused PID or released/evicted entry is queried.
Saved teardown reads never make a fresh query or relabel history.

The **13 native-query cap and8-thread cap stay unchanged**. Five fixed original
queries plus the whole validated thread-list query reservation remain first;
failed thread work never refunds a slot. Two additional pipe queries run only
when at most6 thread queries fit. At7/8 threads the original complete thread
snapshot remains intact and the new pipe facet refuses at the original bound.
An unavailable/overbound/ambiguous list performs no thread queries and its
original refusal remains; no growing list, partial-complete set or retry is
introduced. The whole original snapshot still fits4096 printable ASCII/LF bytes
or returns the same whole fixed refusal, never clipping/raising the report cap.
Existing8 calls/views and native handle/directory/reader semantics stay exact.

## Additive validation, no softened original criteria

Pure cases check actual public ABI, checked query arithmetic, signed counts,
wrong/read-only/rebound non-pipe descriptors, invalid/short returns, final identity
loss, and omission of opaque handles/authority names. A real owned ProcessTree
writes fixed stdout/stderr BEFORE its actual separate ready file, leaves original
parent readers untouched through the native snapshot, then proves those exact
bytes remain readable after the same owned-tree kill/bounded drain. This is not a
mock byte count or a replacement for the original scoped fixtures.

A separate actual shared-builder1s zero-output timeout protects the SAME original
snapshot ticket, pre-stop phase/current descriptors/zero metadata, typed Timeout,
no fabricated exit, original observed capture bytes and stable saved report.
It deliberately has no readiness/partial-output claim. Existing15s invalid-byte/
20s readiness/10s stop/6s partial/Windows2s+3s/30s outer/2s drain/environment20s+5s
and all original assertion/helper bodies remain byte-exact. New fixture/compiler
failures must remain disclosed and corrected only in new code, never suppressed.

One full command per genuinely changed mode must preserve all old case names,
pass/ignored/doctest multiplicities plus additions and report every failure
truthfully. No unchanged broad retry/warmup/CI rerun/manual dispatch. Require all
FOUR exact-head jobs/EVERY setup/mandatory/post step/BOTH modes/native labels/
original Windows9 warning signatures17 occurrences/source/tree/inventory/review
proof before a missing-formal-review-only admin merge. Independently audit merged
main before branch-preserving live FF. Documentation is part of this partial
slice; #138 remains OPEN and prior failures remain FAILED.

Framework/SDK/WIT/deps/lock/Rust1.99.1/Wasmtime48.0.8/frontends/SafeBackend/
desktop-v4/standard ACP/schemas/goldens/trace/corpus and deterministic protected
GadgetCatalogMiddleware stay unchanged. No real model/installed gadget/user
config/global env/SDK mutation, agent/goal, public exec/usage/settlement/callables/
P2g/live/default-on/release enablement or independently blocked provider work.

## Primary API/layout references

- [Apple public proc_info.h types and flavors](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/sys/proc_info.h)
- [Apple proc_info.c exact pipe-type/output handling](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/kern/proc_info.c)
- [Apple sys_pipe.c fill_pipeinfo metadata behavior](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/kern/sys_pipe.c)

The host SDK's native ABI check and retained source hashes are implementation
compatibility evidence, not a portable latency/cancellation/security guarantee.
See [shell-test-observations.md](shell-test-observations.md),
[shell-poll-progress.md](shell-poll-progress.md),
[legacy-shell-observations.md](legacy-shell-observations.md) and
[runtime-status.md](runtime-status.md).


## Initial changed-source validation — November 5 Asia/Taipei / November 4 UTC

All **nine additive macOS cases** pass in BOTH locked feature modes: eight native
pipe/layout/bound cases plus one qualified exact actual-builder timeout case.
Selected counts are checked (8 and1 per mode), not inferred from a zero-selected
command. Locked workspace/all-target checks, rustfmt, lineage, nine offline driver
contracts and the offline headless reference also pass, with no compiler warnings.
These focused/check passes are not full-suite, hosted-platform or root-fix proof.

The initial NEW drain test held temporary `Take` readers across `try_join!` and
failed compilation with E0716; bind those SAME bounded readers before joining.
The NEW source guard first used the wrong relative include path; correct only
that new path. An unqualified exact builder filter selected zero tests and was
explicitly refused as evidence, then corrected to its full module name. All
original logs/correction receipts remain retained; no old fixture/assertion/
deadline changes or warning suppression accompany these new-code corrections.

The final diagnostic Rust aggregate is
`31dae461e7a2730b8213cdc25f1bedd784cf05156616aea561f45ebe2d10f015`.
Source comparison preserves362 unrelated old files byte-exact, every old shell
test/helper as an exact prefix, and original native query/thread/identity logic
through a checked inverse of the declared new pipe seams. Production readers/
pump/wakers/ProcessTree/environment/auth/framework/SDK/WIT/dependencies/pins/
workflows/frontends/schemas/goldens/trace and historical doc prefixes stay exact.
The host public C header independently confirms the184-byte complete pipe-fd
ABI and its offsets/flavor; this is native layout evidence, not wait/startup proof.

Exactly ONE full locked workspace command per mode is next on this genuinely
changed source. Expected main inventory is macOS1626/1769 (off/gadgets), with
ignored8/10; Linux1580/1723 and Windows1560/1692 stay unchanged because these
nine additions are macOS-only. Newly retained failure metadata must guide the
next investigation; zero current pipe buffer must not be relabelled no prior
emission, and a current descriptor must not be called the original pipeline.
#138 remains OPEN; all prior failed evidence and all public/runtime release
gates retain their original classification.


## Complete local validation — November 5 Asia/Taipei / November 4 UTC

Exactly one full locked workspace command per mode completed at diagnostic
source `753d7ffa49fbab262d48f7149df1e6ea65dfec43`:

| Mode | Main units passed / failed / ignored | Whole command | Whole elapsed |
| --- | --- | --- | --- |
| Feature-off | **1622 /4 /8 — FAILED** | exit101 |427.17s |
| Gadget host |1769 /0 /10 | exit0 |77.20s |

Neither reached the driver limit. Every original native name, ignored reason,
doctest and multiplicity remains present plus the nine macOS-only additions.
All nine new cases and all original environment/auth cases pass both modes,
with no compiler warnings. Exact outcome comparison retains only the following
four feature-off failures; the gadget pass never substitutes for them:

- `agent::shell_tests::actual_scoped_shell_poll_progress_retains_reader_return_then_wait_and_original_output`
- `agent::filesystem_tests::same_typed_filesystem_failure_interrupts_an_actually_started_scoped_shell`
- `agent::shell_tests::active_view_snapshot_is_not_queried_again_or_relabelled_fresh_before_stop`
- `agent::shell_tests::actual_shared_builder_latch_stops_a_running_shell_and_prevents_later_spawn`

The new saved facet is complete for each original native query: **all eight
CURRENT-child stdout/stderr descriptors report buffer0/capacity16384/write-bit2/
status0**. Each still retains one original Pending read per pipe and no wait
selection return; the final original child/start/parent/group and retained
workspace checks pass. Two classify system-sh with native Unix/Mach syscall
counts0/0; two system-bash108/107. These original signed/native observations are
not wall-time, exec-entry, startup policy or whole-process wait explanations.
No old/reused PID is queried to supplement them.

This narrows the evidence to no CURRENT buffered data at each saved query, not
proof of no earlier emission, original pipeline identity or a lost-wakeup fix.
The real owned unread-pipe case also passes both modes and independently proves
that the facet detects genuine nonzero buffered stdout/stderr without consuming
the original bytes. No production reader, wake, launcher, clock, environment or
assertion adjustment is justified by the four empty snapshots. Root cause remains
unestablished; #138 stays OPEN and every earlier failure stays FAILED.

The feature-off offline legacy source-guard binary again temporarily had no
reported test-start output, later completed its actual original assertion, and
the complete command/inventory is retained. This is not latency/cause proof,
not a fabricated ready marker and not an unchanged retry. Final publication may
change documentation only after this exact Rust evidence; fresh automatic
exact-final-head all-four/every-step/both-mode native CI remains required before
review-only merge, and independent automatic-main proof before live FF.


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
