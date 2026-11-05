# Test-only legacy-shell failure phases

**November 4, 2026 (UTC): diagnostic implementation in progress, not a shell
root-cause fix or merge approval.** The unsafe environment-test correction and
its original assertions pass on every hosted runner, but PR147 remains draft:
its corrected-head Windows feature-off shell case failed. Keep that failure and
all earlier #138 observations; do not substitute a later gadget-enabled pass.

## Why this observation is needed

The CRLF-corrected head `3919f6403b5d751d01990454c903e73b6e8e76de` ran once as
CI `41835883954`, attempt1. All four new declaration/environment source guards
and all five original environment/protocol assertion cases pass both modes on
Ubuntu24.04, actual macOS15 and Windows2025. Windows feature-off **Test** still
failed `tools::tests::head_tail_timeout_returns_the_partial_output`: the original
stdout assertion received the real 2s timeout's empty-output wording. Its main
result was1551 passed/1 failed/8 ignored; the later gadgets1684/0/10 pass does not
replace it. The interpreter post step was skipped. Formatting/Linux/macOS and every
other Windows step/inventory pass, with the same nine Windows warning signatures
and17 occurrences. The overall informational success is not all-four green.

Those logs cannot distinguish slow spawn, a pending reader, a returned read
error, or the existing wait/drain branches. The original child is already gone:
never query its old/reused PID after the fact or retry unchanged CI to obtain a
more convenient result. Record bounded original operations on genuinely changed
test source instead. This is separate from the unresolved local macOS #138
scoped-shell failures and from the completed deterministic gadget catalog.

## One owner, original operations, no extra polling

`crates/craftsman-rs/src/tools/observations.rs` is a `cfg(test)` child of `tools`.
An ordinary test `ShellRunner` owns at most8 calls; the actual private headless
runner installs no legacy owner and keeps its existing scoped recorder unchanged.
No global panic hook, environment/current-directory mutation, timer reset,
readiness callback, warmup, sampler, replacement launcher or native PID query is
added. Only the original assertion-panic teardown prints this owner's report.

The common executor is extracted into one shared `execute_impl_core`; the old
public-to-crate signature still delegates to it. The original `ProcessTree`,
scrub options, pump, capture, formatting, command text and all existing assertion
bodies remain unchanged after removing only the declared observation/extraction
seams. Both direct and runner calls execute the same core; there is no fake test
executor or prepared result. The observer parameter/fields/wrappers/Drop report
are absent from non-test builds.

For each actual call, record:

- Original call-clock spawn return and optional actual child PID. A PID is not
  interpreter/script entry or readiness; no external PID handle is opened.
- Entry/return of the existing reader, direct-child wait and post-kill drain
  selections. A joined reader return is not proof both pipes reached EOF; actual
  per-pipe results retain that distinction.
- Original `AsyncRead::poll_read` results **after** delegating with the exact
  Context/waker/ReadBuf, with no recorder lock around that inner poll. Pending,
  data, EOF, zero-capacity and error are distinct. Return the original result and
  buffer unchanged; never wrap a waker, poll again or retain raw byte/error text.
- Actual poll/pending/data/EOF/zero-capacity/error counts and original read-buffer
  byte increments. These are not capture/model/token/usage/delivery counts.
  Checked counter overflow stays unknown, never saturation/reset/refund.
- A copied pre-stop read history immediately before the existing owned-tree
  kill, with a direct original/current tree-id relation. Subsequent drain facts
  remain separate current history; no relabelled freshness or new PID lookup.

All report fields are fixed printable ASCII/LF labels, booleans or optional
integers. The **whole** report fits4096 bytes or returns one fixed whole refusal;
never clip a supposedly complete history. Over8 calls, duplicate/missing phases,
poisoned state or invalid delegated buffer/result facts refuse instead of
manufacturing data. There is no Debug/Serialize/public ToolMeta/result/event/
trace/writer interface. Already returned references cannot be retracted, and
these diagnostics do not establish process/IO quiescence, startup or wait cause.

## Additive checks and unchanged limits

Eight new cases cover exact waker/buffer/result delegation, preserved read errors
and empty-capacity-vs-EOF, checked overflow, call/report ceilings, historical
pre-stop-vs-current counts, duplicate/poison refusal and one actual legacy
zero-output2s timeout. That native case deliberately has **no** readiness or
partial-output claim; its real typed Timeout/zero observed capture bytes/no exit/
duration/unchanged wording and original phase facts must agree. Unix uses only
a shell builtin, so it cannot leave an ungrouped fixture grandchild. A CRLF-safe
offline declaration guard protects cfg(test) scope and absence of new native/
waker/public-output interfaces; it is not native execution evidence.

The old Windows2s partial-output/3s Job-PID/30s outer/2s drain, private6s partial/
20s readiness/15s invalid-byte/10s stop and new environment20s/5s bounds stay
unchanged. Existing scoped native13-query/8-thread/call/view/4096-byte/ticket/
PID-start-parent-group revalidation limits stay unchanged; this observer makes
zero native queries. No assertion/deadline weakening or warning suppression.

Documentation accompanies the incomplete diagnostic. One full run per genuinely
changed feature mode must retain every old name/assertion/ignored/doctest plus
the additions, and classify every failure honestly. Require ALL FOUR exact-head
CI jobs, EVERY setup/mandatory/post step, BOTH modes, native labels, original
Windows warnings, source/tree/inventory and review proof before a review-only
admin merge. No unchanged broad test/CI retry or manual dispatch. A changed-source
pass alone does not establish a cause or erase previous failures.

Framework/SDK/WIT/deps/lock/Rust1.99.1/Wasmtime48.0.8/frontends/SafeBackend/
desktop-v4/standard ACP/schemas/goldens/trace/corpus and production defaults stay
unchanged. #138 remains OPEN. Public exec, callable headless gadgets/catalog,
usage/settlement, input/output/result/events/persistence/trace/provenance/P2g/
live/default-on/release remain separate and unavailable; no independently
blocked provider correction is inspected or worked around.

See [test-environment-isolation.md](test-environment-isolation.md),
[windows-shell-fixtures.md](windows-shell-fixtures.md),
[shell-test-observations.md](shell-test-observations.md),
[shell-poll-progress.md](shell-poll-progress.md) and
[runtime-status.md](runtime-status.md).


## Initial local validation — November 4, 2026 UTC

Eight new pure/actual-owner cases and the new source guard pass in BOTH modes;
both locked workspace/all-target checks are warning-free. Formatting, offline
headless reference and9 driver contracts pass;239 repository-relative Markdown
links resolve. The complete old tools source reconstructs byte-exact after only
the declared observation/extraction seams are removed; every original inline
assertion/helper/Windows input and360 unrelated original files remain exact.

The first lineage check found the current draft's post-step interpreter label
in old/new documentation. Preserve its failed log; correct only that label to
"interpreter post", with every date/run/count/status/fact unchanged. No pattern,
allowlist, source, assertion or evidence file is weakened; the corrected check
passes. The exact eight current-draft prefix label changes are separately
recorded, not concealed as unchanged historical bytes.

Focused/pure/local checks are NOT Windows execution, a full-suite pass or #138
root-cause evidence. One full command per genuinely changed mode and ALL FOUR
exact-final-head hosted jobs/every step/both modes/source/tree/inventory/warning/
review gates remain pending. PR147 stays draft/unmerged; all prior failures
remain FAILED and public/usage/settlement/callable/P2g/live/release stay off.


## Complete changed-source full validation — November 4, 2026 UTC

Exactly one whole command per mode at source
`c1cac5450c0a79d71405ee24b690ec96ec2c5be5`; no unchanged broad retry. Both finish
within the original outer bound, without compiler warnings or a Rust-source
change during execution. Every prior native case-name/ignored-reason/doctest
multiplicity plus eight new main cases, the new guard and CRLF guard is retained.
All original environment/protocol/auth cases and all18 new substantive cases
relative to PR146 pass BOTH modes.

- **Off stays FAILED:**1613 passed/4 failed/8 ignored, exit101;998.48s whole
  command. The SAME four scoped-shell cases fail:
  `actual_scoped_shell_poll_progress_retains_reader_return_then_wait_and_original_output`,
  `same_typed_filesystem_failure_interrupts_an_actually_started_scoped_shell`,
  `active_view_snapshot_is_not_queried_again_or_relabelled_fresh_before_stop`,
  `actual_shared_builder_latch_stops_a_running_shell_and_prevents_later_spawn`.
- **Gadgets passes:**1760/0/10, exit0;73.85s whole command. This does not replace
  off or any previous failure.

During off, read-only log checks temporarily saw an offline binary's launch line
without test-start output. It later completed its original guard successfully;
the complete command/inventory is now retained, not an aborted/partial pass.
An interim evidence utility refused after its particular pending-launch
assumption no longer matched progressing logs; no test source/assertion was
changed. This is not measured native startup/cancellation/wait-cause evidence,
and no old PID was queried, runtime restarted or deadline widened.

The main failure reports retain the same original pending-read/no-wait-selection
facts; test-only legacy observations do not establish or fix #138's cause.
Documentation-only publication records both results without another broad local
run. PR147 remains draft/unmerged pending ALL FOUR exact-final-head hosted jobs,
EVERY setup/mandatory/post step, BOTH modes/native labels/source/tree/inventory/
old Windows warnings and review proof. All prior failures remain FAILED; public
exec/usage/settlement/callables/P2g/live/default-on/release stay unavailable.


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
