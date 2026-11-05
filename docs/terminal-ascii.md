# Private checked terminal ASCII encoding

**Status: October 24, 2026.** `Terminal::ascii(max_bytes)` projects the actual
checked terminal channels into ASCII-only JSON and selects whole retained messages
using **post-escape byte sizes**. It preserves the original checked final answer and
private return/approval facts, with the SAME terminal lease/typed epoch/execution
seal. This is not a public result/CLI/IO/trace/storage writer or human sanitizer.

## Actual authority and source admission

Only the consumed actual returned Run can own a Terminal. Its opaque scope and
unique publication lease mint `Ascii`; no caller bytes/config/Prepared/seeds/gates/
facts construct it. Runtime identity, execution closure, publication revocation and
captured typed approval epochs are checked before/after work (including failures)
and every bytes/facts/count/omission/binding read. Foreign fresh owners cannot bind.
Late actual approval supersedes old fragments; new capture retains genuine return
and stop observations, never reopening execution or rewriting a public outcome.

The actual main state lock covers complete channel admission/copies, ASCII projection,
layout arithmetic and final framing. Original final-text byte admission precedes
message copies; the unchanged producer admits every message/decoded argument before
its copies. BOTH complete UTF-8 channels pass their original source/view/work/encoded
bounds and literal checks before ASCII projection or any omission. An unsafe or
unadmitted message/output cannot be rescued by dropping it.

## ASCII representation, not decoding or aliases

Printable ASCII JSON syntax/data and already encoded C0/quote/backslash escapes
remain byte-identical. DEL and every non-ASCII scalar inside verified JSON strings
are emitted as lowercase `\uXXXX` UTF-16 units; supplementary scalars emit their
valid surrogate pair. C1, Unicode line/paragraph separators and a BOM inside data
therefore remain escaped data, not terminal controls, physical lines or a file BOM.
No lossy decoding, Unicode normalization, semantic argument parsing, JSON repair or
replacement/execution aliases are introduced. Literal backslash-u text stays literal
text. Decoding the ASCII JSON preserves original checked keys/ids/names/scalars/data.

The input is already verified compact JSON, not arbitrary raw JSON. Strict UTF-8,
string/escape context and structural-mark boundaries are checked. The unchanged
#120 span walk supplies at most256 record starts plus body boundaries; projection
maps those offsets at scalar boundaries rather than parsing an enlarged raw frame.
A size pass precedes ASCII allocation, then a projection pass produces exact bytes.

Each original encoded channel remains <=1MiB. ASCII staging has an explicit bounded
**6MiB per channel** ceiling: one admitted source byte can expand to at most six
ASCII bytes (DEL). This is additional PRIVATE projection work/memory, not a broadened
original source/view/record budget or publishable frame. No disk spill is used.
The SAME unchanged KMP stream checks every complete projected channel in <=64KiB
chunks, preserving cross-chunk matches without a new matcher or changed seed/chunk/
position/record limits. All projected records are checked before selection, including
records that will be omitted. A generated escape can introduce a captured credential;
refuse fixed rather than rewriting syntax or omitting the offending source.

## Exact post-escape selection and final verification

The caller's complete ASCII fragment budget can only tighten1MiB. It includes the
full original checked output/return/approval header, message-scope framing, commas,
ASCII escaping and omission digits. Select the largest contiguous newest whole-message
suffix in original order; if the newest record cannot fit, do not substitute older
small records around it. If complete header/output/empty-array framing cannot fit,
refuse without dropping facts or truncating the answer. At most257 arithmetic layouts
are considered; there is no repeated serialization/redaction search.

The complete chosen frame is checked again by the unchanged finite literal verifier.
New first-record/header joins and omission integers can introduce matches; refuse
fixed without alternate layouts or fabricated counts. Exact final bytes stay within
the requested budget and contain only printable ASCII JSON bytes (no physical C0/C1/
DEL/newline/BOM). Kept plus omitted equals the actual captured retained count, not
messages lost to earlier compaction or a complete all-role/child/audit transcript.

Null failure output and a genuine empty returned answer remain distinct. Original
state/arguments/roles/keys/scalars/tool-error flags/outcome stay unchanged. Snapshot
bytes remain immutable after state changes; no live/concurrent atomic-send guarantee
or magical retraction of already-read references is claimed.

## Compatibility and remaining gates

Existing UTF-8 snapshot/terminal selector/live view/layout/request codecs and the
terminal authority/lease/deny-only seal are unchanged. ASCII encoding does not grant
or reopen tools, providers, compaction, guest callables or session writes. Private
returned words/final-answer phrases cannot establish public completion/refusal/limit/
verification/exit/ACP stop mapping, permission grants or billing.

Machine content resembling runner commands stays inside one JSON record; this is
NOT human-channel `::`/`##vso[` defanging, terminal rendering or PowerShell redirection
support. A separate private renderer now selects only the owner's immutable actual
answer/fixed private diagnostics, with visible control/runner checks and unchanged
lease/epoch/source authority; see [terminal-human.md](terminal-human.md). Real
TTY/PowerShell/OS/IO/public-text adapters still require their own tested boundaries.
Raw errors, other state, registry/inbox/child/provenance channels remain unselected.
Public exec/stdin/flags/result/events/output-file/trace/storage/meter (#97), complete
public R2/16MiB output and outcome/lifecycle/P2g/live/default-on/release gates remain
open. The separate pending #112 material is not included, retried or bypassed.
Framework/SDK/WIT/deps/lock/pin, ordinary frontends/providers/hosts/native limits,
old schemas/frames/goldens/trace-v1/tests/corpus and historical targets stay intact.

## Evidence

The first47 actual-Run focused fixtures passed without failures or warnings. They
cover BMP/supplementary/noncharacter/BOM/control round trips, quoted delimiters,
literal escapes, Unicode identities/keys, normalization preservation, exact post-
escape budgets, bounded enlarged staging, complete-answer refusal, generated escape/
chunk/join/count collisions, original source refusals, actual approval/failure/empty
returns, immutable state, foreign binding, lease/typed-epoch overrides and no later
compaction traffic. These are private synthetic/shared-builder fixtures, not live
model quality, human rendering, OS redirection or a public rollout.

Collected checks below do not imply a full local native workspace or strict-Clippy
run. Documentation belongs in each partial implementation; ALL FOUR exact-final-head
CI jobs and every mandatory step, including Windows, gate any missing-review-only
admin merge. No failed check or substantive review is bypassed.

## Collected bounded local checks

| Check | Result |
| --- | --- |
| Actual terminal ASCII cases, feature-off / gadgets | 47 passed in each build, zero failed/warnings |
| Unchanged UTF-8 terminal selector / terminal authority | 43 / 43 passed |
| Unchanged live selector / retained view | 39 / 28 passed |
| Existing runtime / input / provider / LLM | 42 / 30 / 47 / 10 passed, zero failed |
| Both locked production checks, with/without gadgets | Passed without warnings |
| Offline accepted contract / benchmark driver | 126 cases / 9 tests passed |
| Formatting / whitespace / lineage / protected-source audit | Passed |
| Current Markdown fences / local links | Balanced; 31 links resolved |

These distinct bounded checks were collected on approved825cf09; no full local
native workspace, strict-Clippy, live/human/OS/IO/public rollout is claimed. The
initial47 focused suite and final47 builds were all green without warning corrections.
ALL FOUR exact-final-head CI jobs/every mandatory step, Windows included, were
required before merge; the verified evidence follows. No failed check or
substantive review was bypassed.

## #123 merge evidence

On October 24, 2026 at 04:05:26 UTC, #123 was squash-merged after ALL FOUR
exact-final-head CI jobs and every mandatory step passed, Windows included.
Linux/macOS main units 1028 feature-off /1169 gadgets; Windows 1005 /1135;
zero failures, and all 47 ASCII fixture passed in both builds on all three OSes.
Run `40057618477` tested checkout
`e62654f6cfd661edf45f035e6444840c8048cd38`;
final head `eadae5c891365af9846466274f5ad7448a8841b0` and audited
base `825cf09b049323384860be9601418dfaddcb9014` produced tree
`c2fcfeb5c334058fd7391a78bb8bfe10d148214c`. Actual squash
`b901d6ff274ceada4464d8bc2d90bb1f98d623b7` has that identical tree and the
audited base as its sole parent. Fresh rules/head/base/reviews/comments/threads
were re-audited; only missing review was overridden, no failed/substantive check.
This verified source merge is not a public exec/IO/human/OS/live/release rollout.


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
