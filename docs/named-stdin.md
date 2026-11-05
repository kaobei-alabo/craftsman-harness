# Private explicit byte-pipe stdin capture

**Status: October 24, 2026 (UTC) / October 25, 2026 (Asia/Taipei).** `StdinInvocation` is a new private,
explicit-only input route, not a shipped `craftsman exec` command. It captures native
byte-pipe stdin within the existing 1 MiB instruction bound and accepted 10 s
first-byte / 30 s total-read clocks. It uses no blocking reader thread, generic
caller reader, raw spill, public stdout/result/event/trace or usage producer.

## Separate source selection and actual preparation

```text
original Options + Sources (positional bytes exactly "-", no prompt file)
  -> whole instruction/count/path selection grammar
  -> SAME Bootstrap / original config / captured credentials / retained ConfinedFs
  -> explicit stdin same-access duplication + owned native polling strategy
  -> bounded original bytes / genuine EOF / mode restoration and handle closure
  -> strict complete UTF-8 / one counted encoding preamble / nonempty instruction
  -> SAME old attachment/file/framing admission through original ConfinedFs
  -> only then session selection/IDs/store and SAME Runtime/shared builder
  -> consume original input once -> existing actual Run/terminal authority
```

Missing/conflicting sources and all attachment count/path errors precede even
stdin inspection. An opaque `StdinSelection` is minted only by the exact positional
`-`; the native factory requires that token. `prompt_file = "-"` is still a workspace
file named `-`, not stdin. A literal `" - "` is not the stdin marker. An unnamed pipe
is never inspected by this route. It does not choose a shell/slash-command path.

The existing `Invocation`, `PreparedInput` and guarded `FileInvocation` entry points
still refuse positional stdin, with their older assertions/bodies unchanged. The
separate native route is opt-in and does not silently change old frontend behavior.
It returns the existing actual input `Run`; no caller-created Admitted/Prepared/Run,
origin/bytes/seeds/gates or public result record constructor is added. Combining
named stdin with the guarded destination route/full flag pipeline remains separate.

## Concrete native ownership and supported subset

This initial **conservative private subset supports readable native byte pipes**.
Console/TTY, devices, sockets and regular files are refused, not handed to a blocking
worker or lossy generic reader. Existing confined instruction-file input remains
available independently. Unsupported read/query/byte/synchronous rights or modes
refuse fixed; missing mode-setting attributes alone uses the guarded native path
below, not an access upgrade or blind blocking reader.
This is not full public stdin/console/regular-file support.

Only selected input duplicates the actual OS stdin descriptor/HANDLE, non-inheritable.
The duplicate is retained throughout capture. A process-local ownership lease
serializes this native factory; a second capture refuses Busy rather than waiting
or sharing source reads. Fixture-only owned native pipes have no production raw-file
constructor. There is no SDK/guest capability or model-selected descriptor.

- **Unix:** validate FIFO/read access and lease `O_NONBLOCK`, retaining original
  status flags. Duplicates share the open file description, so the temporary flag
  affects that selected pipe description, not only the duplicated fd.
- **Windows:** same-access/non-inheritable duplication preserves the original
  handle's EXACT grants. `NtQueryObject`'s fixed basic metadata supplies actual
  HANDLE grants (not a file object's older access mask); native pipe type and
  `FileModeInformation` verify readable byte/synchronous IO before data reads.
  Asynchronous/overlapped, write-only and non-byte sources refuse before reading.
  If the original handle already has NOWAIT, preserve it. If it has mode-setting
  attributes, lease `PIPE_NOWAIT` as before; an actual setter failure still refuses.
  A blocking read-only handle WITHOUT those attributes instead keeps its mode
  unchanged and polls `PeekNamedPipe` with **no data buffer**. This strategy is
  selected upfront from original mode/actual grants, never a caught-error fallback.
  An open/empty source returns Pending WITHOUT issuing `ReadFile`. Positive
  readiness tightens a real read to at most the observed available bytes and the
  existing scratch/cap probe. A terminal peek condition permits a concrete read
  probe, **not EOF**, so buffered bytes are not discarded. Only real `ReadFile`
  broken/disconnected EOF finishes. `ERROR_NO_DATA` and successful zero bytes
  stay Pending; capture native errors before the next metadata call. There is no
  access upgrade, handle/name reopening, blind idle read or reader thread.
  Both paths are synchronous, **not overlapped I/O**. See the limits below: this
  is not global stdin exclusion or guaranteed native-call interruptibility.
- Before/after every native read, verify the expected mode. Restore only original/
  still-owned mode, never overwrite a foreign mutation. Mode restoration and native
  handle closure precede returning admitted bytes. Drop performs one best-effort
  owned restoration and closes the duplicate before releasing the capture lease.

The native source is concrete capability/OS IO, not an injectable reader/status/
receipt interface. No pending read task or detached blocking thread keeps consuming
input after a timeout or a dropped waiting future. Future frontend scheduling and
whole-process/P2g shutdown remain separate.

## Exact source bounds and deadlines

A single monotonic reader-start timestamp backs both deadlines; total time is
never restarted after a first/trickle byte. Check the applicable clock before and
after each returned concrete `Pipe::read` result, including errors/EOF/Pending;
its guarded native sequence has no asynchronous pending operation. Before any observed data,
first-byte expiry takes precedence even if a delayed poll sees both clocks expired.
After observed data, the absolute total-read clock applies. A single byte followed
by an idle open pipe cannot evade it. Bootstrap/config setup precedes the read clock;
this is not a whole-invocation timeout.

Read buffers are at most 16 KiB. Retain at most the original 1 MiB; at the exact cap,
only a stack-byte EOF/overflow probe remains. Any overflow discards the entire
prefix, never clips. At most4096 `Pipe::read` API attempts include Pending/
Interrupted and actual EOF; an attempt may include readiness/metadata plus a real
read. Waits are bounded by the next applicable deadline with at most10 ms polling.
This private work ceiling is not a native-syscall/byte/token or billing ledger.
Peek availability is NEVER counted as originally consumed bytes or EOF.

Only genuine native EOF completes capture. Then strict whole UTF-8 admission removes
at most one leading BOM, counting its original bytes before normalization. Empty/
Unicode-whitespace-only instructions, malformed UTF-8, overflow, deadline, allocation,
mode/IO or work-limit failure refuse fixed before any session/store/provider effect.
Original data/CRLF/order remain separate from rendered size, public provenance,
usage or grants. `OriginKind::Stdin` records actual original bytes and preamble
removal with no execution path; it is not a Serialize/public-schema producer.

Attachments use the unchanged old byte/count/path/read/framing code: 1 MiB each,
8 MiB total,128 entries,4096-scalar/16 KiB labels and the existing derived whole
message bound. No declaration is a tool or filesystem grant. No raw input/model/
error data is written to a journal/checkpoint/trace/cache or public terminal.

## Truthful limits and compatibility

Owned NOWAIT reads and readiness-bounded read-only polling avoid ordinary
open/idle **data reads** in this single-owner factory. They are not an OS sandbox,
native-call interruptibility or portable shared-description CAS. A synchronous
PeekNamedPipe can itself block behind concurrent IO; a foreign reader can consume
availability between peek and read. Native actors can also change modes, so no
absolute OS deadline or prompt drop of an in-flight native call is promised.
The process-local capture lease excludes only this factory's other captures, not
prior buffered/ambient/concurrent stdin consumers. Metadata/mode/close/driver
limits remain. No pending kernel read or detached reader thread survives an
actually dropped waiting future because none is created. Source consumption
cannot be undone; failed owned-mode restoration can leave a changed mode, never
authorizing fallback reads, public output or success. Read-only readiness never
changes mode, and foreign mode changes refuse without being overwritten. A public
frontend MUST establish exclusive input/lifecycle guarantees before enablement.

Old config/input/file-output/session/core/permission/codec/lease/seal/source limits
and old assertions remain; only narrow new routes/declarations and native source
classification were appended. Framework/SDK/WIT/dependencies/lock/Rust pin/frontends/
SafeBackend/protocol-v4/standard-ACP frames/schemas/goldens/trace-v1/corpus and the
historical baseline/targets are unchanged. Rust1.99.1 still matched the official
stable manifest dated October15, checked on October25 local (October24 UTC); no
install/toolchain/dependency change was made.

Public `exec`/flags/full stdin/output-path support, standard public outcomes/R2/
results/events/JSONL/stdio/TTY/PowerShell, compatible gadget provenance/callables/
catalog, redacted persistence/trace, #97 meter, P2g and live/default-on/release remain
separate/off. Independent pending material is not included, retried or bypassed.

## Implementation evidence and preserved first attempts

The first private platform-only production check was followed by a composed check
that reported one new unused `Facts` import. That new import was removed; no old
source/lint allowance changed. The initial one-case actual native pipe fixture
passed. The expanded new test module initially failed to compile because its
borrowed Sources helper needed explicit lifetimes and an unimplemented subprocess
fixture left an unused Stdio import. Only new fixture definitions/imports were
corrected. These first attempts are not warning-free/expanded-test evidence.

The first31-case expanded run had30 passes/1 failure because its new approval
fixture granted ordinary writes while expecting refusal. It now uses a real
protected build.rs write under the unchanged policy; no permission/assertion was
weakened. The first native subprocess run had32 passes/4 failures/1 ignored: its
argv marker was mistakenly parsed as a libtest option. Only the new invocation now
places the marker after `--`; the ignored helper still checks it before any stdin
inspection. The corrected run passed36/0 with the native helper intentionally
ignored outside its marked subprocess. Those failed attempts are preserved.

Additional actual-single-stdin-lease, no-session-on-deadline and invalid-private-limit
fixtures bring focused native coverage to39 passing cases plus one intentionally
ignored argv-only subprocess helper. The production source already uses defaults;
shorter clocks, owned-pipe injection and read-size shaping exist only in new tests.
No frozen fixture/assertion or production deadline was relaxed for these passes.

New source/actual-pipe/UTF-8/BOM/cap/idle/trickle/timeout/cancellation/mode/attachment/
actual-builder fixtures are collected below when complete, with failed attempts
kept explicit. Native subprocess helpers require a precise argv marker before
any stdin inspection; unmarked ignored runs must return without reading. No real
user config/grant, guest execution, model key or SDK materialization is used.
Documentation is part of this incomplete implementation. ALL FOUR exact-final-head
full CI jobs AND every mandatory step, informational Windows included, precede any
missing-review-only admin merge; local/mocked runs are not public/OS/live approval.


## Collected source-local verification before hosted CI

All39 new normal cases passed in both feature modes; the one ignored helper ran
only through argv-marked native subprocess fixtures, and an unmarked invocation
returned without reading. The22-row bounded verification passed the unchanged
input30/file66/human60/ASCII47/terminal-selector43/authority43/live-selector39/
retained-view28/runtime42/construction22/confined32/provider47/LLM10 suites, both
locked production checks, reference126/driver9 and format/whitespace/lineage. No
compiler warning remained. Old source bodies/tests/limits and framework/SDK/WIT/
lock/pin/frontends/frames/schemas/goldens/trace/corpus are exact after only new
declarations, the additive private Stdin origin and a public-reader doc qualifier.
Markdown fences balance;55 current local links resolve.

The full source-local feature-off workspace at four test threads/`--no-fail-fast`
was **not green**:1191 main passes/2 unchanged real-native shell-startup failures
(the scoped-FS stop and shell latch fixtures did not observe actual startup).
No-fail-fast continued and every other integration/framework/SDK/macro/doc binary
passed. These are not successful fast-stop/native-startup assertions. The full
gadget workspace with the same settings passed1334 main units and every other
binary with zero failures/warnings. Both inventories contained all40 current test
functions (39 normal/one ignored), not a baseline binary. No duplicate full retry,
old assertion/ready marker/deadline/OS-service change or root-cause claim was used.
All attempts remain preserved. ALL FOUR fresh exact-final-head CI jobs AND every
mandatory step, Windows included, still gate missing-review-only admin merge.

## First hosted Windows failures and corrective native handling

PR128's first revision (`50cec5fa7f0cf8b51507cb72cc42425287bda5f7`,
CI40113876534) passed formatting/Linux/macOS but **failed both Windows test
steps**: feature-off1156 passes/12 failures/3 ignored; gadgets1286 passes/12
failures/4 ignored. Windows is informational in the workflow, so an aggregate
run success is not a successful Windows test result or permission to merge.
The native failures and original logs are preserved; this revision was not merged.

Two separate failure groups remain evidence, not fixture/deadline exemptions:
empty writer-open NOWAIT pipes were admitted as EOF (including idle/trickle/drop
cases), and the real inherited-stdin fixtures refused native mode setup. Rust's
Windows `File::read` maps all BrokenPipe-kind errors to zero; the correction uses
native `ReadFile` so `ERROR_NO_DATA` stays Pending. It captures `GetLastError`
before any subsequent metadata call, treats successful zero bytes as Pending,
and still requires actual broken/disconnected-pipe EOF. No available-prefix,
readiness-only or timeout result is promoted to EOF.

That first corrective factory requested only read plus mode-setting attributes for its
non-inheritable duplicate. Native metadata additionally verifies actual read
access, byte-pipe type and synchronous mode before a stack buffer is offered to
`ReadFile`; asynchronous/overlapped sources refuse before reading. OS refusal
still fails fixed: no ambient reopening, privilege/data-write escalation, generic
reader, detached thread or blocking stdio fallback is added. This access request
must be verified with the unchanged **real inherited process-stdin fixtures**,
not inferred from CreatePipe fixtures or an API's possible access upgrades.

Two added direct native regressions test Pending/one byte/Pending/closed-writer
EOF and reject a write-only end before reading or changing its mode. They extended
that revision's inventory to42 functions (41 normal plus the same one argv-only
ignored helper), on top of the preserved earlier39-case evidence. Existing source
bodies/fixtures/assertions,1MiB/10s/30s/4096/16KiB limits and frontend/dependency/
lock/pin/schema/golden/trace boundaries are unchanged. Corrected-source local and
hosted evidence is collected independently; ALL FOUR fresh exact-final-head jobs
**and every mandatory step**, Windows included, still gate a review-only admin
merge. Tests do not enable public exec/stdin, gadget callables/catalog or usage.

The3c0f3da source-local bounded verification passed all22 rows without
warnings:41 native cases plus one ignored helper in each feature mode, the same
unchanged input30/file66/human60/ASCII47/terminal-selector43/authority43/
live-selector39/view28/runtime42/construction22/confined32/provider47/LLM10
suites, both locked production checks, reference126/driver9 and format/whitespace/
lineage. Scope checks preserve all older source/tests after the same narrow new
module declarations/additive private origin;66 current local Markdown links
resolve and fences balance. These macOS/source-local passes do NOT establish
that the native Windows access request succeeds; actual exact-head hosted Windows
steps and their real-stdin fixtures still have to prove it before any merge.

## Original-rights/read-only correction after the second hosted run

The second hosted revision3c0f3da / CI40121387466 passed fmt/Linux/macOS but
**again failed both Windows builds**: feature-off1165 passes/5 failures/3 ignored;
gadgets1295/5/4. Both first and second hosted attempts remain unmerged evidence.
The seven former idle/trickle/split-UTF8/drop/early-deadline failures were corrected;
all five remaining failures in EACH build were the unchanged actual inherited-
stdin scenarios, now returning native Io. Requesting additional duplicate mode
attributes did NOT demonstrate ordinary inherited-stdin support. An arbitrary
access upgrade is not inferred, repeated or turned into a fixture exemption.

The current native source instead preserves EXACT original access with same-access
non-inheritable duplication. Fixed actual-HANDLE grant metadata selects owned
NOWAIT only when already active or actually writable; blocking read-only pipes
keep their mode and use the narrowly guarded native-readiness path above. Unknown
metadata/type/synchronous/read rights or an attempted mode-set failure refuse.
There is no user-configured polling strategy, generic reader, namespace/handle
reopen, privilege/data-write escalation, blind idle read or background worker.
Only concrete `ReadFile` results observe source bytes/EOF; peek values are not
source facts/usage/receipts. No successful availability/peer-close observation
finishes capture or discards buffered bytes without that real read probe.

Seven additional native read-only regressions exercise Pending/data/real EOF,
queued peer-closed whole UTF-8/BOM and fixed empty/UTF-8 refusal before fresh IDs,
both absolute deadlines before sessions/traffic, no trickle reset, actual drop/
late-byte preservation, original1MiB exact-cap/overflow and the unchanged combined
4096-attempt data/EOF ceiling. Windows fixtures reduce an isolated native reader
handle to GENERIC_READ; Unix uses real FIFO readers. These are TEST-ONLY rights
shaping/owned-pipe constructors, not production source/status/grant injection.
The current inventory is49 functions:48 normal cases and the same one argv-only
ignored helper. All42 earlier test bodies/assertions remain an exact prefix,
including every real process-stdin fixture. Source/read/UTF8/framing/deadline/work
limits, old merged input/config/runtime/output bodies, SDK/WIT/framework/lock/pin/
frontends/frames/schemas/goldens/trace/corpus are unchanged. Documentation records
the synchronous-IO/external-reader gap explicitly; passing native fixtures cannot
enable public stdin/exec, assert OS cancellation/exclusivity, publish origin/usage
or waive ALL FOUR fresh exact-final-head jobs/every mandatory step before merge.

The original-rights correction's22 bounded source-local rows passed without
warnings:48 normal native cases plus one ignored helper in EACH feature mode,
unchanged input30/file66/human60/ASCII47/terminal-selector43/authority43/
live-selector39/view28/runtime42/construction22/confined32/provider47/LLM10,
both locked production checks, reference126/driver9 and format/whitespace/lineage.
Exact24-path scope/older bodies and the42-test prefix were audited;68 local
Markdown links resolve and fences balance. This is current macOS/source-local
evidence, NOT yet a passed Windows actual-HANDLE/read-only path or public/global
IO guarantee. Fresh exact-head all-platform/every-step CI and raw-log/tree proof
must independently establish the Windows correction before any review-only merge.

## Merged native foundation and separate file composition

The final original-rights head fce22a6 merged through #128 as72abf26 on October24
UTC, after CI40124517968 passed ALL FOUR jobs/every mandatory step. Each of48
normal native cases passed in BOTH modes on Linux/macOS/Windows; tested/merged
tree and audited parent matched. Earlier failed Windows heads and local attempts
remain explicit. Existing Windows warning signatures/counts were unchanged; no
new stdin warning, suppression or warning-free Windows graph is inferred.

A NEW separate [stdin-file-output.md](stdin-file-output.md) route now captures the
original guarded target BEFORE stdin inspection, uses this unchanged native source
and old attachments, then revalidates target/absence BEFORE IDs/store/providers.
Only opaque pre-run preparation constructs the existing file invocation/controller;
this native-only route/old body remains unchanged. No new reader/status/provenance/
usage producer, public exec/fullstdin/exclusive lifecycle or OS guarantee follows.
The new composed-source final-head/every-step CI must be collected independently.


## Private argv routing over the same native factory

The separate [input-arguments.md](input-arguments.md) factories validate the
source/attachment subset of native argv BEFORE config or stdin inspection. Exact
positional `-` selects the unchanged StdinInvocation/StdinFileInvocation; a
`--prompt-file -` value is a file, while an unnamed pipe remains untouched by a
literal instruction. Unsupported full flags refuse, not consume data or imply
implemented wall/output/usage control. Actual original stdin and all same-access
rights/mode strategy/restore/close/EOF/1MiB/10s/30s/native-call/shared-reader limits
stay unchanged. No new native reader, public stdin/stdio/callable or receipt.
New argv-marked helpers are isolated and return before stdin/config when unmarked;
old49 native fixture bodies and42 composition normal cases remain exact. New
full final-head ALL FOUR jobs/EVERY mandatory step are independent of #129.


[control-arguments.md](control-arguments.md) now composes supported absolute
scope/permission/env/session syntax with this exact old stdin route. Invalid
control/policy/ID/scopes and configured credential env-pass sources refuse before
native input; unmarked new helper returns before config/stdin. Root/captured auth/
policy/read modes/grants/EOF/10s/30s/bytes/restore/close/OS limits remain unchanged.
Original pipe bytes remain available after rejected controls or literal input;
no full console/file/stdio/wall/usage/public/callable or native guarantee follows.


## Relative-scope/output-file admission — October29 (Asia/Taipei)

A separate [scope-output-arguments.md](scope-output-arguments.md) adapter now admits
normal-relative cwd/config and one original output-file path BEFORE the SAME old
control/input/native/composed preparation. Old complete syntax is reused and dropped;
old Bootstrap/ConfinedFs/physical target capture remains authoritative. Relative task/
output paths use selected workspace; physical workspace/config output is still denied.
Only typed private ASCII/human FileOptions exist: public text/JSON/JSONL, stdio and
result/event flags still refuse. No post-run path/format/target/Run/gate/seed/writer
constructor or new reader/publisher/clock/lease is added. All original no-consumption,
EOF/rights/mode/restore/close/budget/epoch/revalidation/one-attempt/cleanup/OS limits
remain unchanged; all-four fresh exact-head/every-step CI still gates merge. Public
exec/full path/stdin/stdio/R2/storage/trace/provenance/gadgets/usage/#97/P2g/live/release
remain separate/off. See the new doc for source attempts and current independent gates.
