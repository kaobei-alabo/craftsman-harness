# Private bounded terminal-message selection

**Status: October 24, 2026.** `Terminal::selection(max_bytes)` keeps the original
checked final answer and actual return/approval observations while selecting only
a newest whole-message suffix. It uses #121's independent publication authority;
no live-view latch exemption, execution/resume or public IO writer is added.

## Authority and full source admission

Only an actual consumed `Run` can own the Terminal. Its SAME execution gate remains
closed, original outcome/state remains private and its unique publication lease
still checks the captured typed approval epoch before/after work and every read.
The selected type has no Debug/Serialize/raw/IO constructor. Foreign fresh owners
cannot bind; revocation/late approval supersede reads and codec failures as before.

The actual state lock covers both channel production, layout planning and final
encoding. The immutable original final-text byte admission precedes message copies;
the SAME unchanged message producer performs whole message/decoded-argument admission
before its copies. Both complete encoded channels are checked **before any omission**.
Their original byte/view/work/depth/node/identity/call/message/encoded caps remain.
An unsafe or unadmitted old message or output is never rescued by dropping it.

The original `Terminal::snapshot()` remains all-message-or-refuse with its original
combined1MiB ceiling, exact bytes and zero omissions. This new path instead admits
and checks the complete channels independently **before** applying the tightening
combined budget. It can therefore retain a complete admitted answer when the two
otherwise admitted channels do not fit together; it does not broaden either source.

## Exact whole-message policy

The private caller budget includes **all** terminal facts, complete final output,
message scope/framing, commas, escaping and omission-integer digits. It may be no
more than1MiB. Select the largest contiguous newest whole-message suffix that fits,
in original order. If the newest message cannot fit, retain no messages rather
than older smaller substitutes around it. If the complete header/output/empty-array
framing cannot fit, refuse fixed: do not omit facts or clip the answer to make room.

Selected records retain their exact already checked encoded bytes. The #120 fixed70
span walk is reused through a pure projection wrapper, not changed or duplicated
as a raw parser. At most256 starts/257 arithmetic candidates are examined. No
recursive decoding, JSON repair, string/marker/scalar cuts, argument execution
aliases, repeated encoding/redaction search or reordered records are introduced.
The final unchanged literal verifier checks the chosen complete combined frame.
New first-record/header joins or omission integers can collide with captured seeds;
refuse fixed without trying another layout, rewriting a count or changing a fact.

`kept + messages_omitted` equals the actual captured main retained count. It does not
count earlier compaction losses or imply an all-role/child/audit/live transcript.
The selected snapshot stays immutable after later state changes. Original roles,
ids/names, keys/scalars/tool-error flags, final answer and the Run outcome remain
unchanged or refuse. Missing failure output stays null; genuine empty output stays
an empty string. Empty selected messages never mean successful task completion.

## Compatibility and remaining gates

The #121 lease/typed epoch and deny-only execution seal are unchanged. A real
approval failure can yield a checked selected partial fragment while its live
View/Selection stays stopped; no model/tool/provider operation is reopened. A late
stop can coexist with the original private returned fact, never a manufactured
public completion/refusal/status/exit/verification/billing decision.

Raw errors, other state/provenance/registry/inbox/child data remain unselected. No
public result/ASCII/runner-command/output-file/trace/storage/CLI/stdin producer,
headless gadget callable/catalog, meter (#97), P2g shutdown, all-role/process/IO
quiescence or live/default-on/release gate is enabled. Full public R2/16MiB output
production and other source/consumer boundaries stay independent. Old read refs
cannot be retracted magically; a future IO writer must recheck the lease at publish.

Framework/SDK/WIT/deps/lock/pin, ordinary provider/frontends/hosts/native limits,
old tests/goldens/frames/schemas/trace-v1/corpus and historical baseline/targets are
unchanged. Whole snapshot, request codecs, original live selector/layout algorithms
and terminal authority are preserved. The separate pending #112 material is not
included, retried or bypassed.

## Evidence

Actual returned-input/shared-builder HTTP fixtures cover exact full/last/empty
framing, retained output/facts, admitted combined-size reduction, source refusals,
all roles/arguments/flags, Unicode/escaping/markers, omission digits/new joins,
actual approval/failure/null/empty outcomes, late epochs/revocation, foreign binding,
immutable state, frozen config and no post-seal compaction traffic.

The first42 focused cases passed, with one new unused type-export warning. The
public methods were placed in the terminal module to use that export directly;
no older assertion, producer behavior or native deadline was changed. An omitted
numeric-argument collision case was added, giving43 final focused cases. Collected
results follow; no full local native workspace/strict-Clippy/live-quality/public
rollout is inferred. A separate private ASCII adapter now projects complete admitted
channels and applies post-escape byte budgets (`terminal-ascii.md`) without changing
this UTF-8 selector/lease or opening public IO. A separate private human renderer
(`terminal-human.md`) now selects only original immutable final output and fixed
private diagnostics, not decoded selected messages. It keeps this SAME authority/
lease/epoch/seal and adds no public text eligibility/IO or source-cap exemption.
Every partial implementation includes documentation and requires
ALL FOUR exact-final-head CI jobs/every mandatory step, Windows included, before
any missing-review-only admin merge. Substantive review/failed checks are never bypassed.

## Collected bounded local checks

| Check | Result |
| --- | --- |
| Actual terminal-selector cases, feature-off / gadgets | 43 passed in each build, zero failed/warnings |
| Unchanged terminal authority / live selector / view | 43 / 39 / 28 passed |
| Existing runtime / input / provider / LLM | 42 / 30 / 47 / 10 passed, zero failed |
| Both locked production checks, with/without gadgets | Passed without warnings |
| Offline exec contract / benchmark driver | 126 cases / 9 tests passed |
| Formatting / whitespace / lineage / protected-source audit | Passed |
| Current Markdown fences / local links | Balanced; 28 links resolved |

These are distinct bounded checks on approved7d48ddd, not a full local native
workspace or strict-Clippy run. The initial42/unused-export warning and the corrected
public-method placement remain recorded above; no failed test or older assertion
change is hidden. ALL FOUR exact-final-head full workspace CI jobs/every mandatory
step, including Windows, still gate merge.

## #122 merge evidence

On October24, ALL FOUR exact-final-head PR jobs/every mandatory step passed:
Linux/macOS main981 feature-off /1122 gadgets, Windows958 /1088; zero failed.
All43 selector cases passed in both builds on all OSes. Only missing review was
overridden after fresh rules/head/base/review/comment/thread audit; squash825cf09
has the identical four-job tested tree and audited base as sole parent.


## Separate private file-publication route

`FileInvocation` now captures immutable explicit external directory/name/format/
budget before session creation and only its actual FileRun can consume a paired
one-attempt FileTerminal. Old view/source/selector/terminal/lease behavior stays
unchanged; whole checked records/new LF joins precede guarded native staging and
create-only link. This is not public schema/text eligibility/stdio/JSONL/full R2,
raw state/provenance/storage/usage or a permission grant. Native blocking/Windows/
external-race/CAS/P2g limitations remain explicit. See [guarded-output.md](guarded-output.md).
Native directory-rename fixtures exercise real movement or exact Windows denial,
with original physical authority and all old view/lease/source checks unchanged.
The same native content-pass ceiling includes seek/payload/EOF, tested on actual
files without widening source/public budgets or changing the old codecs.
Native cleanup failures/early file results recheck the original lease/epoch before
classification, preserving only the actual own-link observation and foreign data.


A separate private `StdinInvocation` now captures only explicitly selected native
byte pipes before sessions through the same input/bootstrap authority. It does not
change these original view/codec/lease/state/source limits or introduce a public
producer/stdin alias. EOF/deadline/nonblocking mode ownership and unsupported
console/file/external-race limits are described in [named-stdin.md](named-stdin.md).

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

The separate private [stdin-file-output.md](stdin-file-output.md) composition now
captures original guarded output before stdin, rechecks it after complete admitted
input before sessions/providers, and uses the old paired actual file controller.
This boundary's bodies/codec/authority/lease/source caps remain unchanged: no
arbitrary Run/target pairing, new reader/writer or public/callable/meter gate is
added. Shared-reader/native-IO/OS and final-head all-platform gates remain explicit.
