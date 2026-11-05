# Private bounded retained-message selection

**Status: October 23, 2026 (UTC; October 24 in Asia/Taipei).** A private selector
now consumes the actual runtime's already checked retained-message snapshot.
This is not public result/terminal publication, full R2 output-bound support,
ASCII machine encoding, a storage adapter or a shipped `craftsman exec` command.

## Authority and ordering

`Runtime::message_selection(max_bytes)` first calls the unchanged actual-runtime
`message_view()`. The SAME state lock still covers whole original message/argument
admission before copies and finite encoding. Only that opaque `View::select` can
mint a `Selection`; no caller state, bytes, seeds, gate or executable arguments
can construct it. `input::Run` selects the same runtime path, not a raw state clone.

Selection happens **after** the complete original source/view/encoded-frame checks.
An unsafe or unadmitted old message is not hidden by omitting it. The original
256-message/call, 128-scalar identity, combined 1 MiB source/view work, depth64,
100k node-operation and 1 MiB finite-frame limits remain. This deliberately does
not rescue larger unadmitted state or promise every accepted R2 input can be
published. Public output still needs its independent reviewed producer bounds.

## Deterministic whole-message policy

The caller's byte budget includes the **entire private frame**, must be no more
than 1 MiB, and can only tighten the existing ceiling. The selector keeps the
largest contiguous suffix of newest whole messages that fits, in original order.
If the newest record cannot fit, the suffix is empty: earlier small records are
not substituted around it. If even complete empty framing cannot fit, refuse fixed.

`messages_omitted` is the actual number removed from this captured retained snapshot;
kept plus omitted equals the captured message count. It is not a count of earlier
compaction losses or an all-role transcript claim. Whole source content, identities,
keys, scalars and tool-error flags remain unchanged in private state. Selected
encoded records retain the exact already checked bytes, without decoding/re-encoding
arguments, clipping fields/scalars/markers, repair or execution aliases.

A bounded one-pass span walk of **already verified finite JSON**, not a new raw JSON
admission/parser, identifies complete records. It honors quoted delimiters and
escapes; its fixed70 stack accommodates depth64 argument data plus fixed containers.
At most256 starts and257 arithmetic candidates are considered, with no repeated
serialization/redaction search. Exact lengths include JSON escaping, commas,
framing and omission-integer digit changes. Only the chosen frame is checked.

Final framing uses the SAME unchanged request encoded-literal verifier. A new
first-record join or omission integer can introduce a credential collision even
when every data view was safe. Refuse fixed on that collision; never try another
omission to hide a fact, rewrite a count or return a safe-looking partial prefix.
Keeping every message reuses the exact checked original frame.

## Immutable views and independent terminal gate

`Selection` contains immutable checked bytes and actual kept/omitted counts,
borrowing its original MessageScope. It has no Debug/Serialize/raw constructor,
status, final answer, usage, ToolOutcome or writer. Foreign/equal fresh runtimes
cannot bind. A later state change does not turn an older selection into a live
snapshot or a complete compaction/child ledger.

The SAME typed fail latch is checked before and after selection work, including
codec/bound failures, and before every byte/count/omission/binding read. A late
NeedsApproval overrides the local failure and hides existing views; diagnostic
strings and tool-error booleans never latch or authorize. Deterministic test-only
after-work hooks exercise the actual runtime's same gate; production has no hook.

The original `Run` outcome/final answer is not inspected, clipped, inferred from
messages or modified by this operation. An empty selection does not mean a run
completed or erase its original outcome. Terminal/approval-required publication
now has a separate PRIVATE actual-Run authority (`terminal-publication.md`), with
an independent lease and original-return/typed-stop observations. It cannot reopen
the stopped model/tool gate; terminal message selection and public IO/status remain
unwired. The terminal authority now has its own separate private selector
(`terminal-message-selection.md`), preserving complete output/facts under its lease;
it does not reuse or exempt this live selector's fail latch. A separate private terminal
ASCII encoder (`terminal-ascii.md`) now applies post-escape budgets under that
terminal lease; it does not change live view/selection or original execution data.
A separate private human renderer (`terminal-human.md`) uses only the actual
immutable final answer/fixed private diagnostics, not selected/decoded messages.
Whole-source/control/runner checks retain its terminal lease/epoch; no public text
eligibility/writer or this live selector's latch/caps/codec change follows.
Files/todos/custom/context/registry/inbox/child state remain private and unselected.

## Compatibility and evidence

No public exec/stdin/flags/result/events/ASCII/trace/storage producer, headless gadget
callable/catalog, token meter or provenance surface is enabled. #97, P2g, public
R2 reduction, terminal publication and live/default-on/release gates remain open.
Ordinary runtime/provider/control/auth, frontends/hosts/process limits, framework/
SDK/WIT/dependencies/lock/pin, schemas/frames/goldens/trace-v1/old corpus and historical
October16 baseline/Appendix A/target diagrams are unchanged.

Actual-runtime fixtures cover exact whole-record suffix bounds, all roles/flags/
keys/scalars, escaping/Unicode/markers, original source refusals, omission digit and
new-framing collisions, immutable snapshots, foreign binding, frozen seeds, typed
late stops on success/collision/limit, non-latching strings and ignored other state.
The actual admitted-input/shared-builder HTTP fixture preserves original outcome,
final answer and retained state even when all captured messages are omitted.

The initial focused implementation passed all38 fixtures, with no failures. A
maximum-admitted-depth boundary fixture and stronger original-final-answer checks
were then added; final bounded verification is recorded below. No full local
workspace, strict Clippy, live quality or public rollout is inferred from these checks.
Every partial implementation includes documentation and requires ALL FOUR
exact-final-head CI jobs/every mandatory step, Windows included, before any
missing-review-only admin merge. No failed check or substantive review is bypassed.

## Collected local verification

| Check | Result |
| --- | --- |
| Actual-runtime selection fixtures, feature-off / gadgets | 39 passed in each build, zero failed |
| Unchanged retained-message snapshot fixtures | 28 passed |
| Existing runtime / input / provider / LLM fixtures | 42 / 30 / 47 / 10 passed, zero failed |
| Production checks without default features, with and without gadgets | Passed without warnings |
| Offline accepted exec contract | 126 cases passed |
| Offline benchmark driver contracts | 9 passed |
| Formatting / whitespace / lineage / protected-source audit | Passed |
| Current Markdown fences / local links | Balanced; 21 links resolved |

These distinct bounded checks were collected on the approved6a43802 base. ALL FOUR
exact-final-head full workspace CI jobs and every mandatory step remain required
before merge; no full local native workspace or strict-Clippy run is claimed.

## #120 merge evidence

All FOUR exact-final-head PR jobs/every mandatory step passed on October23 UTC:
Linux/macOS main895 feature-off /1036 gadgets, Windows872 /1002; zero failed.
All39 selection fixtures passed in both builds on all three OSes. Fresh rules/
head/base/reviews/comments/threads were audited; only missing review was overridden.
Squashf99ef83 has the identical four-job tested tree and audited base as sole parent.


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
