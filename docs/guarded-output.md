# Private guarded one-file publication

**Status: October 24, 2026.** A new private `FileInvocation` route captures one
explicit destination **before session selection/IDs/store opening**, uses the same
actual shared runtime, then consumes its returned `FileRun` into `FileTerminal`.
The controller can attempt one create-only publication of a checked ASCII terminal
fragment or whole human preview. This is **not** `craftsman exec`, public result/text
eligibility, stdout/stderr, JSONL streaming, trace/storage or a usage producer.

## Actual preparation, ownership and limits

```text
explicit Options + Sources + Destination (original directory/name/format/budget)
  -> complete input grammar and output grammar
  -> same Bootstrap / captured credentials / retained confined input authority
  -> complete input/attachment framing and read-only output-directory capture
  -> only then session selection, IDs/store, same Runtime/shared builder
  -> consume one original input -> actual FileRun
  -> consume actual returned Run, seal execution -> paired FileTerminal
  -> checked actual-terminal Record -> guarded staging/link/cleanup
```

Destination fields are immutable after preparation. Only the actual FileRun's
module constructs FileTerminal; no caller Run/target/config/gates/seeds/bytes/
outcome pairing, raw record constructor, Debug/Serialize, Runtime/resume, path
accessor, stdout fallback or post-run format/destination selection exists. The
old configuration-only/input paths, Options shape, session policy and shared
builder remain unchanged; the new wrapper does not feed sanitized Config into
legacy defaults. Input uses Bootstrap's SAME retained filesystem and complete
admission before either optional session/store or provider effects.

This **conservative private destination subset** requires an explicitly selected,
existing absolute directory outside the physical agent workspace and selected
config. File names are single original components <=255 UTF-8 bytes; directory
paths <=4096 scalars/16KiB and <=128 normal components. Reject relative/dot/parent,
non-UTF-8, special/credential/runtime/build names and unsafe file names. Existing
pure conservative NFC/lowercase keys may **deny only**, never rename a destination.
The original supplied filename—including any credential-like substring—is the
executed name; raw names/paths never enter diagnostics or published records.

Only the OS root/drive is opened ambiently. All normal directory components are
walked no-follow under retained handles, capturing by-handle identities. Windows
accepts disk/verbatim-disk roots, not UNC/device roots. Caller-selected roots must
be free of symlink components: resolve trusted OS temporary aliases before invoking
this private API, not by a later publisher reopen. Missing parents are refused,
not created. Retained bootstrap workspace/config identities are denied at every
step, including renamed roots; this is not a path-spelling-only scope check.

All candidate target entry kinds, including existing files/directories/hard links
and dangling symlinks, are refused before session creation. Directory capture is
read-only: no file reservation, staging directory, SDK materialization, provider
request, thread ID or checkpoint/journal writer follows an invalid destination.
This outside-workspace host sink does not widen **agent** ConfinedFs/guest/tool
permissions or let model text choose a new path. It is not general output-path
validation for public R2; inside-workspace destinations remain a separate gate.

## Checked records before any publication effect

Format is fixed pre-run as `AsciiFragment` or `HumanPreview`, never the public
`json`/`text` schema vocabulary. Only the actual terminal scope mints Record;
its SAME state/gate/publication identity and actual captured approval epoch bind
all reads. Whole original admission, frozen literal redaction and old UTF-8/ASCII/
human codecs stay unchanged. No unadmitted source is rescued by file publication.

The immutable whole record and budget are prepared before staging:

- ASCII: the old checked whole-message suffix fragment plus one physical LF;
  reserve that LF **inside** the caller's complete budget and verify the new join.
- Human preview: whole checked answer, when present, then its fixed private-fact
  diagnostic, each already LF-framed. Missing vs genuinely empty output stays
  distinct. This is **not** public text stdout/outcome selection.

Budget can only tighten **1MiB including all framing**. The unchanged finite KMP
verifier checks the complete chosen file bytes/joins again before any IO effect.
A newly synthesized literal, escaping, source/work/cap or framing failure refuses
fixed; never truncate an answer, omit facts, change format/layout after failure,
repair JSON, decode a displayed execution alias or spill raw original data.
The actual original return, partial retained state and typed approval observations
remain owned; private source merges do not establish task correctness/public exit.

## Guarded native publication and failure

Only one attempt is permitted, including codec/IO failures; `&mut FileTerminal`
serializes its controller. Failure seals its independent publication lease without
reopening execution or discarding the original Run. Production IO is concrete
capability-file IO, not a caller-provided writer/status/receipt implementation.
Stdout/stderr/TTY handles are not opened or inspected.

1. Recheck original directory identities/target absence and actual record lease.
2. Create one OS-random 128-bit, create-only `.craftsman-exec-tmp-*` directory under
   the retained destination, with no collision retry or recursive cleanup. Unix
   mode700 is supplied at creation; Windows retains inherited-ACL limitations.
3. Create a no-follow, create-only private record file (Unix mode600), retaining
   its original handle/identity. No raw input/model/tool/error data is staged.
4. Recheck the SAME lease/typed epoch before and after each actual write result,
   including errors/Interrupted, then flush and file sync. Writes <=16KiB and
   <=4096 calls, including Interrupted returns; WriteZero fails immediately.
   No blind write-all or unbounded Interrupted retry. Blocking native operations
   are not interruptible by this guard: it prevents subsequent operations, not
   rollback of an initiated call.
5. Bounded same-handle content/EOF checks and directory/entry identity/single-link
   checks precede create-only hard linking into the final name. Never append,
   truncate, replace, follow a planted final symlink or fall back to ambient IO.
6. Check after link, remove only owned staging identities, then verify final
   identity/single-link/length/content and SAME lease before returning. Five
   bounded content passes use <=16KiB scratch and <=4096 native calls per pass,
   including its one seek, payload reads/Interrupted and final EOF probe. This is
   additional private IO work, not a widened source/view/public R2 budget.

Late actual approval/revocation overrides local IO failures too. Test-only stage
signals/fault injection act on the same real owner/gate and native temporary files;
production has no callback or injectable writer. Simulated BrokenPipe/flush/sync
errors test contracts, not real stdout/pipe/PowerShell support.

Failure retains only a fixed typed kind and whether **our native link call returned
success**. That observation is minted by the actual controller, not a caller field;
it is not current file existence, byte/request/usage/billing receipts, public run
success, verification, complete delivery or a durability promise. If failure/stop
occurs after link, an already visible file can remain. Never delete a final name
which an external actor may have rebound, or claim complete output on a failed
sink. Source output, provider/state/tool facts and #97 semantics are unchanged.

Cleanup is deny-only: only owned random directory/data identities are considered;
unknown/replaced/nonempty staging entries are not recursively erased. Failed cleanup
can leave a private checked prefix or foreign entry; it does not authorize another
publication attempt. No power-failure durability/parent-directory fsync guarantee,
external-writer/namespace/content CAS, arbitrary concurrent sink interleaving,
process quiescence, rollback, signal/P2g or OS containment claim follows. Same-user
native shell processes/external actors are not contained and may race/change files
after a check; Windows capability-path operations remain best effort. These limits
must be resolved/evaluated before a public producer is enabled.

## Compatibility and remaining gates

Old codecs/redactor/live selectors/source caps, terminal authority/lease/seal,
filesystem operations/permissions, preparation/input/session behavior and older
assertions are unchanged; only narrow new routes and test-only actual-owner
observers were appended. Framework/SDK/WIT/dependencies/lock/Rust pin, native hosts/
process limits/frontends/SafeBackend, v4/ACP frames, schemas/goldens/trace-v1/corpus
and historical baseline/targets remain unchanged. Deterministic terminal gadget
catalog refresh is unchanged; this does not enable headless callables/catalog.

Public exec/stdin/flags, inside-workspace output paths, standard outcome/text
eligibility, full R2/16MiB result/event production, real stdio/TTY/PowerShell/code-
page IO, JSONL streaming, storage/trace/provenance, successful-response/alternative-
state adapters, #97 meter, P2g and live/default-on/release gates remain separate/off.
This is a synchronous private file API; a future async frontend must schedule
blocking IO appropriately and implement its own stop/deadline/sink lifecycle.
Independent pending material is not included, retried or bypassed.

## Evidence and attempted checks

The first production check failed on new-module visibility/name resolution and
one new unused-mut warning. Only those new boundaries/declaration were corrected;
the corrected check was warning-free. The first59 native file fixtures passed
without failure/warnings. Additional actual-stop/root-identity/exact-cap/source-
limit fixtures bring bounded local coverage to64 (two Unix-only native cases;
Windows has62). No old assertion, deadline, source guard or production limit was
changed to obtain a pass. Final collected checks are recorded below.

Documentation belongs in every partial implementation. ALL FOUR exact-final-head
CI jobs and every mandatory step, including informational Windows, gate missing-
review-only admin merge. No failed check or substantive requested change is
bypassed. This evidence is not a public exec/stdio/TTY/OS/live/release rollout.


## Collected local verification and preserved failures

| Check | Result |
| --- | --- |
| Initial native file fixtures, feature-off / gadgets | 64 passed per local build before final audits; zero failed/warnings |
| Unchanged human / ASCII / terminal selector / authority | 60 / 47 / 43 / 43 passed |
| Unchanged live selector / retained view | 39 / 28 passed |
| Existing runtime / input / construction / confined FS | 42 / 30 / 22 / 32 passed |
| Existing provider / LLM | 47 / 10 passed |
| Locked production checks with/without gadgets | Warning-free |
| Accepted offline contract / benchmark driver | 126 cases / 9 tests passed |
| Earlier full local default-feature-free workspace | 1152 main feature-off /1293 gadgets; zero failed/warnings before Windows follow-ups |
| Format / whitespace / lineage / protected-source audit | Passed; old assertions/bodies/limits unchanged |
| Current Markdown fences / local links | Balanced; 44 resolved |

The initial full gadget run was **not green**: 1290 main passes and three unchanged
legacy shell startup/output-timeout failures. A focused 13-case legacy-shell rerun
had12 passes and one unchanged partial-output readiness failure; no failed attempt
is counted as successful evidence. No old test/production source, readiness marker,
assertion, six-second call budget or stop deadline was edited or weakened.

A read-only snapshot of committed parent875056f ran the exact partial-timeout case
successfully. Reusing its Cargo target exposed a stale baseline-binary hazard:
a subsequent1229-count run lacked the new64 fixtures and is **not** current-code
coverage. Current source contents were kept identical; invalidating only the
app source timestamp forced a real rebuild. Its compiled inventory contained all
64 new cases; the identical full gadget workspace command then passed1293 main
units and all integration/framework/SDK/macro/doc tests without failures/warnings.
Both failed runs, baseline results and stale-artifact attempt remain preserved.
This does not establish an external root cause or a strict-Clippy/voice/stdio/
TTY/PowerShell/OS/live/release result. ALL FOUR exact-final-head full CI jobs and
every mandatory step, Windows included, still gate merge.


## First Windows CI failure and initial correction

Initial PR125 heada7a8906 passed fmt/Linux/macOS but Windows failed both native
build steps (38 /37 main failures; no merge). Most new cases exposed staging
cleanup with live original file/directory handles: deleted names remained pending,
so removal failed and private entries persisted. The corrected publisher explicitly
closes those staging-path handles before owned unlink/remove-directory, then opens
ONLY the retained final directory's original name, verifies the originating
identity BEFORE reading, and retains that verified final handle for unchanged
content/EOF/identity checks. No raw/ambient/foreign IO fallback, weaker ownership,
final-name deletion, retries, deadline/permission changes or fake delivery are used.

New Windows path fixtures now construct literal dot/parent spellings through
OsString, because PathBuf::push resolves them for verbatim roots before validation.
The first selected-config correction still attempted actual rename, but incorrectly
expected every native denial to map to PermissionDenied; the next Windows run
exposed that assumption. Its same original/config scope rejection was not removed.
All older source/assertions stay unchanged. The failed Windows logs remain preserved;
ALL FOUR checks/every mandatory step must pass on the corrected final head before
any admin merge.


## Second Windows CI failure and fixture-only correction

Corrected head7f90a0e, run40078891462, again passed fmt/Linux/macOS but failed both
Windows native steps:1124 passed/3 failed feature-off and1254 passed/3 failed gadgets.
The remaining failures were only three **new** directory-rename fixtures; the native
publication/content/cleanup cases passed. This is not a green matrix and no merge
was attempted. Initial and second Windows failure logs remain preserved.

The log reports native32 (ERROR_SHARING_VIOLATION), classified as Uncategorized by
the pinned Rust toolchain, not PermissionDenied. The pinned cap-std directory opens
intentionally omit FILE_SHARE_DELETE, so forcing Unix-style rename would require
weakening retained authority. Production code is **unchanged** by this correction.

The new test-only helper releases each temporary metadata probe before exactly one
real rename, leaving only the actual Prepared/runtime authority retained. It admits
only documented native Windows5 (access denied) or32 (sharing violation), never a
generic ErrorKind/skip/retry. An unexpected native error, including any error on
non-Windows, fails. Every branch proves the original by-handle directory identity:

- Actual movement: preserve Changed/no-own-link refusal, empty original staging
  cleanup and exact foreign replacement preservation. No ambient reopen succeeds.
- Native refusal: assert unchanged original identity and no moved directory, then
  require checked publication in that SAME original directory, exact resulting
  entries/whole answer and preserved unrelated data. This is not rebind evidence.
- Config: both outcomes still capture the actual Prepared selected root and reject
  its SAME physical identity with Scope; no path-spelling substitution or skip.

Both output-parent fixtures also assert the single-attempt rule after either
outcome. Original source/codec/lease/epoch/IO/ownership/permission limits and all
older assertions/dependencies/hosts/frontends remain untouched. The first local
format check requested only the new helper's line layout; that layout was corrected
without formatting older source. All-four exact-final-head CI/every mandatory step,
Windows included, still gates merge; local success is not Windows/full-rollout proof.

Native constants and sharing behavior are documented in the Windows API reference:
[system error codes](https://learn.microsoft.com/en-us/windows/win32/debug/system-error-codes--0-499-)
and [CreateFileW sharing modes](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew).


## Current-source full local follow-up after the second correction

Both compiled fixture inventories contained all64 current cases; no baseline export
or shared-target substitution was used. The default-parallelism feature-off full
workspace run was **not green**:1149 main passes/3 failures in unchanged shell
startup/output-timeout fixtures. Actual ready observation timed out, the partial-
output case saw zero observed bytes in all its existing bounded attempts, and the
invalid-pipe-UTF8 case reached Timeout rather than the required typed Failed. Those
are real failed assertions, not fast-stop/partial-output/UTF8 evidence. Cargo stopped
at the failed main binary; a completed full integration run is not claimed.

The same current-source full default-parallelism gadget build passed1293 main tests
and every integration/framework/SDK/macro/doc binary with zero failures/warnings.
A separate single feature-off full run with `-- --test-threads=4` passed1152 main
tests and every integration/framework/SDK/macro/doc binary without failures/warnings.
It lowers test concurrency only, preserving every actual assertion/real readiness
marker/six-second call/strict stop deadline. It does not establish default-concurrency
cold-start behavior or erase the failed run. No duplicate default-parallelism full
retries were queued.
A read-only process snapshot showed high CPU but proves no external cause. No host
security service, old source/assertion, deadline, dependency or native limit changed.
All failures and commands remain preserved; all FOUR exact-final-head CI jobs and
every mandatory step, informational Windows included, remain separate merge gates.


## Final native content-pass call audit

The final review found that the loop counter bounded payload reads but omitted
the native seek and EOF probe. A new actual native-file boundary test reproduced
publication beyond the stated combined4096-call ceiling. The test instrumentation
first hit a new mutable-slice borrow compile error; its local length calculation
was corrected before the red boundary run, not counted as successful evidence.
The real red run compiled, then failed because the over-ceiling record published.

The unchanged4096 constant now covers **one seek + every payload read (including
Interrupted) + the final EOF probe** in each of five passes. If no call remains for
EOF, refuse the whole record before that probe, any later pass or create-only link.
Never raise the cap, publish a checked prefix or retry another format/destination.
The one seek remains guarded; every native read/result keeps SAME lease/epoch
checks, with unchanged16KiB scratch/source/record/permission/ownership bounds.
The ceiling counts initiated blocking file API attempts, not all internal kernel
syscalls, a byte/usage ledger or an interruptibility/OS-work guarantee.

The fixture uses actual owned native files and the actual terminal/target chain.
It observes the staged record's real length:4094 bytes succeeds with one seek,4094
one-byte payload reads and one EOF per pass;4095 bytes exhausts the first pass
before EOF/link, leaves no final/staging entry and cannot be retried. All five
content passes are observed in the success case. max_read/read_calls exist only
in test Signals; production still uses concrete native capability files with no
callback/raw writer/status/byte/request/usage/receipt interface.

Coverage is now66 native cases (64 on Windows; the same two Unix-only cases).
All64 prior cases and their real assertions remain; the new test adds two native
boundary outcomes without changing old hosts/frontends/codecs/framework/dependencies/
Rust pin/public schemas or limits. Earlier full-run counts above describe the
pre-counter-fix source honestly; new-source focused/full runs and ALL FOUR fresh
exact-final-head jobs/every mandatory step must be collected before merge.


## Final native-result and failed-cleanup audit

A second final guard review found `staging.remove()?` could bypass its post-result
lease/epoch check on an ownership/IO error. The actual native-name rebound fixture
reproduced Changed rather than the expected late-stop result; that compiled red
runtime failure is preserved separately, not counted as green fixture evidence.

Cleanup now **retains the native result**, fires the test-only post-attempt signal,
rechecks SAME actual lease/epoch, then uses that result. The actual paired controller
also rechecks Record after every publish return, including early IO/random/error
paths and owned staging cleanup via Drop. No raw path/error echo, caller gate/status,
retry, final-name deletion or recursive erase is introduced. Native failure without
a stop stays native failure; actual typed epoch change or lease revocation takes
precedence, preserving only the real own-link returned-success observation.

The one added fixture covers three real native outcomes after linking: replace
the staging name with a foreign identity, keep the original checked data under a
new staging name, then observe no stop / actual NeedsApproval / actual revocation
after the failed native cleanup attempt. Results are respectively Changed /
Superseded / Closed, always with the actual own-link observation true. Foreign
bytes, original checked data and the already visible final entry must survive;
one-attempt and no-store assertions remain. This does not promise absence, complete
delivery, rollback or public output correctness. Hooks remain test-only actual-owner
observations, not production callbacks, writers, schemas or receipts.

Final private coverage is66 cases (64 Windows). Both new fixtures and all64 old
fixture assertions/limits are retained. Fresh feature-off/gadget focused/full suites,
protected-source/Markdown audits and ALL FOUR exact-final-head CI jobs/every mandatory
step, including informational Windows, still gate missing-review-only admin merge.


## Collected verification for the final native guard corrections

Both feature-mode focused suites passed all66 cases without failures/warnings.
The21-row bounded verification also passed all unchanged human/ASCII/terminal/
live-message/runtime/input/construction/filesystem/provider/LLM suites, both locked
production checks, accepted contract126/driver9, format/whitespace/lineage. Protected
source/old assertions/dependencies/hosts/frontends/schema/trace audits remain exact;
all current Markdown fences balance and44 local links resolve.

The new-source full feature-off workspace at four test threads was **not green**:
1151 main passes/3 failures in unchanged native-startup fixtures. The scoped-FS stop
case and shell latch case did not observe real startup, and invalid pipe UTF8 hit
Timeout instead of the required typed Failed. The failed main binary stopped Cargo;
this is not a completed feature-off integration/fast-stop/UTF8 proof. The new-source
four-test-thread gadget full workspace passed1295 main units and every integration/
framework/SDK/macro/doc binary with zero failures/warnings. No duplicate retry of
this final failed full command was queued, and no old readiness assertion/deadline,
host-security setting or source was altered to hide it. All commands/results and
the two compiled red guard reproductions remain preserved.

These final local results are separate from all earlier full counts and future CI.
The fresh final-head ALL-FOUR full CI jobs AND every mandatory step, including
informational Windows, must succeed before any missing-review-only admin merge.
None of this enables public exec, stdio/TTY/PowerShell, R2, JSONL/storage/trace,
provenance/callable gadgets/catalog, #97 meter, P2g or live/default-on/release gates.


#125 subsequently passed every mandatory step in all FOUR exact-final-head jobs
on October24 UTC: Linux/macOS1154 feature-off /1295 gadgets; Windows1129 /1259,
zero failed, all66 native cases in each Unix build/all64 applicable Windows cases.
Only missing review was overridden. CI tested the current-main merge tree
bcb2aa7, and squash63395cf has that identical tree and auditedaf8c4e1 sole parent;
the original28 scoped blobs stayed identical and unrelated #126 Docker/workflow
changes were preserved. Final local feature-off1151/3 native-startup failures remain
documented, not retroactively counted as green. No public gate was enabled.


A separate new private named-input route now reads only explicitly selected native
byte-pipe stdin, with bounded source/deadline/mode ownership before sessions. It
does not change this FileInvocation/target/terminal pipeline or combine stdin and
file destination flags; see [named-stdin.md](named-stdin.md).

The first hosted native-stdin revision failed both Windows builds (12 failures
each); its earlier local passes are not cross-platform completion. The separate
native correction preserves real-stdin/deadline/source assertions, distinguishes
Pending from genuine EOF and verifies read/type/synchronous ownership without a
blocking-reader fallback. This document's existing boundary is unchanged; see
[named-stdin.md](named-stdin.md) for the preserved failures and fresh exact-head
Windows/other-platform merge gates. No public frontend/callable/meter is enabled.

A second Windows attempt still failed the five real-stdin cases in both builds.
Current native polling keeps original grants: owned NOWAIT where supported, or
unchanged-mode availability before read-only data/actual EOF probes. No blind
idle/generic/background reader, access upgrade or public OS/exclusive-input
claim is added; all earlier42 stdin assertions and this boundary stay intact.
The additive read-only cases and final-head gates are in `named-stdin.md`.

## Separate native stdin/file composition

[stdin-file-output.md](stdin-file-output.md) adds only an opaque pre-run native
source route into the EXISTING paired FileInvocation/FileRun/FileTerminal/Record.
All source/destination grammar precedes IO; SAME Bootstrap roots capture the
original target before stdin inspection and a second read-only retained identity/
absence check follows whole native/attachment admission before sessions/providers.
That additive Target checker cannot mutate/rebind/grant a destination; older
argument/file/native/controller/source/write/codec bodies and bounds stay exact.
No arbitrary Run/target constructor, post-run path/format switch or new writer is
added. Input may already be consumed when a late target change refuses: no rewind,
raw-spill fallback, namespace CAS or public IO/outcome/usage claim. Old limits and
ALL FOUR fresh final-head/every-step gates remain prerequisites for each slice.


## Private instruction argv to the same guarded controller

The new [input-arguments.md](input-arguments.md) `ArgumentFileInvocation` validates
only source/attachment argv before IO, then selects the original FileInvocation
or StdinFileInvocation using an explicit pre-run ORIGINAL Destination. It does
NOT parse `--output-file`/public `text|json|jsonl` flags, turn private preview/
fragments into public formats, expose caller Run/target pairing or switch paths/
formats after execution. Both routes return the EXISTING FileRun/FileTerminal/
Record/publisher; source/capture ordering, same-owner epoch/create-only/one-attempt/
cleanup/foreign-preservation/limits and all native/Windows/namespace/OS caveats
remain unchanged. #129 merged with exact tested-tree/all-step evidence; current
argv-source CI is a separate gate. No public exec/stdout/stderr/results/events/
R2/storage/provenance/gadget/usage/#97/P2g/live/release enablement follows.


[control-arguments.md](control-arguments.md) composes supported control/source
syntax with the SAME old file invocation using an ORIGINAL pre-run typed
Destination. It still refuses output-format/file flags, does not reinterpret
private fragments/preview as public text/JSON, and cannot pair arbitrary Run/
target/bytes/gates/seeds or change post-run format/path. Old pre-session authority,
actual lease/epoch/record/write/cleanup/create-only/single-attempt/OS limits stay
exact; current control-source all-four exact-head/every-step CI is independent.


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
