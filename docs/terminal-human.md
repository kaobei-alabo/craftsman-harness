# Private checked human-channel rendering

**Status: October 24, 2026.** `Terminal::human(max_bytes)` renders the actual
owned Run's complete final answer and a separate fixed private-fact diagnostic.
It selects the SAME frozen redactor, terminal publication lease, typed approval
epoch and deny-only execution seal. It is **not** a stdout/stderr writer, public
outcome/exit mapping, result/event producer, CLI command or live terminal adapter.

## Authority and original source

Only the consumed actual returned Run can own a Terminal and mint this opaque
`Human` through its actual scope. No caller text/JSON/Prepared/config/gate/seeds/
errors/facts construct it. No Debug/Serialize/raw constructor, Runtime/resume
accessor, approval reset, writer or device/TTY passthrough option exists.

The final-answer source is the immutable original `TurnOutput::final_text` owned
by that Run, not a last retained assistant message or a decoded JSON/ASCII frame.
It is borrowed before complete source admission. Match the existing conservative
terminal-output work budget: **twice original UTF-8 bytes plus the complete
redacted view <=1MiB**. Reject oversized original work before copying/redacting;
then apply the unchanged bounded literal redactor to the whole source and verify
its complete view before any rendering. No prefix, clipping, spill, lossy decode,
normalization, second-pass redaction or replacement/execution alias is used.

This answer-only path does **not** read/copy/admit retained messages, files, todos,
custom/context/registry/inbox/child state, raw errors or provenance. It can render
an admitted final answer even when the independent all-message/ASCII path would
refuse a retained transcript. That does not admit/rescue/publish those unselected
sources or change any old message/JSON source cap, codec or assertion.

## Fixed diagnostics, not public outcomes

The separate diagnostic contains only the actual private return variant, captured
typed approval flag and whether the original final answer exists, for example:

```text
craftsman: private run return=returned; approval_required=false; final_answer=present
```

These observations do not establish task correctness, stdout/stderr eligibility,
public completion/refusal/limit/exit/ACP stop reasons, verification, permissions,
request/byte receipts, usage or billing. Final-answer words never authorize or
latch. An actual failed/approval-required return has absent final output; its raw
error/partial retained state is never formatted as a diagnostic or synthetic answer.
A later actual approval stop may coexist with the original returned observation;
a fresh capture keeps both genuine facts, never renaming the return to success.
Fixed words/scalars must pass the SAME literal verifier unchanged or refuse.

## Human display representation

Preserve readable ordinary UTF-8, spaces and LF-separated text. Do not parse
Markdown, ANSI, percent escapes, JSON, surrogate-like literals or shell commands.
Render the following as visible lowercase `\u{xxxx}` text:

- C0/C1/DEL controls except LF, including ESC, BEL, CR, TAB and vertical tab;
- U+061C, U+200E–U+200F, U+202A–U+202E and U+2066–U+2069 bidi marks/embeddings/
  overrides/isolates;
- U+2028–U+2029 line/paragraph separators and a data U+FEFF BOM.

Double an original backslash so literal escape-looking text remains distinguishable
from generated escapes. Preserve other scalars, including script/emoji joiners,
combining sequences, supplementary characters and noncharacters. This explicit
set is not an arbitrary Unicode invisibility/homoglyph/secret detector; there is
no normalization, grapheme/cell-width promise or attempt to execute repaired data.

Defang **every adjacent colon/hash delimiter anywhere**, not merely at a line
start and not via nonoverlapping string replacement. Keep the first colon/hash
and visibly escape every immediately following same source scalar:

```text
::warning::data  -> :\u{003a}warning:\u{003a}data
##vso[task.x]    -> #\u{0023}vso[task.x]
:::             -> :\u{003a}\u{003a}
### heading     -> #\u{0023}\u{0023} heading
```

No contiguous `::` or `##` can remain, including overlapping runs, embedded and
indented commands, legacy `##[` formatting and other hash-prefix syntax. Isolated
colon/hash/URL punctuation and percent-looking data stay data. No stateful runner
command is sent to switch interpretation off and no environment/TTY mode weakens
this rule. The independent test recognizers follow the opening/name/closing syntax
of pinned primary [GitHub Actions parsers](https://github.com/actions/runner/blob/f4a774978d9562d6539be5055c17c0eb970d08df/src/Runner.Common/ActionCommand.cs)
and the [Azure logging parser](https://github.com/microsoft/azure-pipelines-agent/blob/d5d308a62e1ac33454b86b653d8a5940dd297d10/src/Microsoft.VisualStudio.Services.Agent/Command.cs):
original fixture commands must be recognized, rendered lines must not. Those
bounded recognizers are not a live runner, .NET locale or OS-redirection test.

## Complete bounds, framing and final literal checks

An exact size pass counts UTF-8, all visible escaping/defanging, doubled backslashes,
both complete channels and their final LF **before either rendered allocation**.
The caller can only tighten a combined **1MiB** private byte cap. There is no
larger projection staging or disk spill. Preserve the whole answer and private
facts or refuse; do not truncate/omit the answer/diagnostic, choose alternative
escaping or claim the full public R2/16MiB output allowance.

Every present channel ends in LF. Keep existing trailing LF/blank lines; append
one LF only when absent. A genuine empty answer is `Some` containing that one
framing LF; missing failure output is `None`. This delimiter is display framing,
not original model bytes, a token/receipt count or a mutation of the Run's output.

The SAME unchanged KMP byte stream verifies complete rendered channels and their
joins in **both fixed orders**, in <=64KiB chunks. This catches credentials newly
created by controls, backslash doubling, defanging, final LF, diagnostic scalars or
channel joins, including cross-chunk matches. Refuse fixed, never rewrite facts,
try alternative layouts or return a diagnostic-only fallback for an unsafe answer.
No arbitrary concurrent stdout/stderr interleaving, OS re-encoding, atomic-send,
process-quiescence or retraction guarantee follows from checking these two orders.

## Lease, compatibility and remaining boundaries

Check the actual runtime identity/execution closure, revocable publication lease
and captured typed approval epoch before/after all work, including failures, and
before every answer/diagnostic/facts/binding read. Late actual approval supersedes
old channels; revocation closes them. Foreign equal fresh owners cannot bind.
Production has no after-work callback; deterministic tests exercise only the
actual same gate. No tools/providers/compaction/execution or persistence reopen.
Already-read references cannot be retracted; a future writer must recheck the
actual lease/epoch at every publication boundary and handle failed sinks honestly.

UTF-8 snapshots/selectors, live-message selection, checked ASCII JSON, request
codecs/redactor, terminal authority/lease/seal and all original data/state remain
unchanged. Existing frontends still use their existing paths; TUI SafeBackend is
untouched and cannot be bypassed. No standard ACP/frozen desktop-v4 frame, schema,
golden, old fixture, SDK/WIT, dependency/lock/pin, native host or process limit changes.
The unchanged Rust pin 1.99.1 matched the official stable manifest on October 24, 2026
(manifest date October 15, 2026); this slice does not install/update a toolchain.

Public exec/stdin/flags, public outcome/text eligibility, complete R2 result/events,
create-only guarded IO/output files, PowerShell/code-page/real TTY redirection,
successful-response/alternative-state/storage/trace/provenance producers, callable
headless gadgets/catalog, #97 meter, P2g and live/default-on/release gates remain
separate and disabled. The independent pending material stays untouched. See
[terminal-ascii.md](terminal-ascii.md) for #123's verified source-merge evidence,
not a public rollout.

## Verification record

The initial 55-case feature-off run had 54 passes and one new fixture failure:
a single-byte captured test seed made the existing provider-request guard refuse
before the target output was reached. The new fixture now uses a distinctive
captured two-byte seed and retains its assertion that a genuine returned answer
was obtained. No production/request/redactor behavior or older assertion changed.
Independent runner-recognizer and source/codec-isolation fixtures were then
added. An initial provider-test filter matched no tests; the corrected actual
module filter passed all 47 existing cases and the empty attempt is not counted.
First attempts are not relabeled green; collected final checks are recorded below.

Documentation belongs in every partial implementation. ALL FOUR exact-final-head
CI jobs and every mandatory step, Windows included, gate missing-review-only admin
merge. No failed check or substantive requested change is bypassed. These private
synthetic/shared-builder checks are not live-model/TTY/OS/IO/release evidence.


## Collected bounded local checks

| Check | Result |
| --- | --- |
| Actual-Run human fixtures, feature-off / gadgets | 60 passed in each build; zero failed or warnings |
| Unchanged ASCII / UTF-8 terminal selector / terminal authority | 47 / 43 / 43 passed |
| Unchanged live selector / retained-message view | 39 / 28 passed |
| Existing runtime / input / provider / LLM | 42 / 30 / 47 / 10 passed; zero failed |
| Both locked production checks, with/without gadgets | Passed without warnings |
| Offline accepted contract / benchmark driver | 126 cases / 9 tests passed |
| Formatting / whitespace / lineage / exact protected-source audit | Passed |
| Current Markdown fences / local links | Balanced; 36 local links resolved |
| Read-only toolchain verification | Unchanged pin 1.99.1 matches the official stable manifest |

These distinct bounded checks were collected on verified #123 base
`b901d6ff274ceada4464d8bc2d90bb1f98d623b7`. They do not constitute a full
local native workspace or strict-Clippy run, live model quality, actual runner/
TTY/PowerShell/OS IO, public exec or a published release. The original failed
fixture and empty-filter attempt remain recorded above; no old test assertion,
producer/codec/redactor, source cap or permission was changed to obtain a pass.
ALL FOUR exact-final-head full CI jobs and every mandatory step, including
informational Windows, remain required before merge.


## #124 verified merge evidence

On October 24, 2026 at 04:56:18 UTC, #124 was squash-merged only after all FOUR
exact-final-head jobs/every mandatory step passed, Windows included. Linux/macOS
main units1088 feature-off /1229 gadgets; Windows1065 /1195; zero failures and
all60 new fixtures passed twice per OS. Run40062116333 checked out
`5118b6926ab4d0f07b95eb5fa76e94d0226cda59` with tree
`8450cb52edade69c2bccdc336d2961174af91456`. Final head
`d87fd2fc8b8a6fe4f2d1a55e6284fda38aa7fd4b`, audited base
`b901d6ff274ceada4464d8bc2d90bb1f98d623b7` and actual squash
`875056f94de2dc763da7b137a2bafcfd3fe5c3ab` have the verified tested/merged
tree and audited sole-parent lineage. Fresh unchanged rules/head/base/reviews/
comments/threads allowed only missing-review override, no failed/substantive check.
Both additional complete local default-feature-free workspace builds passed without
warnings (1088/1229 main units); no strict-Clippy/voice/stdio/TTY/OS/live/release
claim follows. This source merge did not expose exec or public human IO.

## Separate private file preview

A new `FileInvocation` route (`guarded-output.md`) now captures one immutable
external destination before sessions, then consumes only its actual Run. Whole
checked human answer + fixed private diagnostic can form a bounded private file
preview under the SAME lease/epoch, not public text eligibility or stdout/stderr.
Old human codec/source limits and data stay unchanged; guarded native file staging/
create-only link and honest failure do not reopen execution or enable full R2,
JSONL/stdio/TTY/PowerShell/storage/provenance/#97/P2g/live/release gates.
Native directory-rename fixtures verify the actual unchanged directory when Windows
denies movement; they do not loosen retained authority or change the human codec.
Actual native boundary fixtures count seek/payload/EOF inside each unchanged pass
ceiling, preserving the whole preview and old source/public budgets.
Native cleanup/early errors recheck the same actual lease/epoch before failure
classification; visible prior links remain possible and no native rollback is implied.


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
