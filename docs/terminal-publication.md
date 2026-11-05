# Private terminal-publication authority

**Status: October 24, 2026.** `Run::terminal()` consumes a real returned private
input invocation, closes its actual execution gate and owns the original runtime,
input facts and outcome. Its independent, revocable publication lease can capture
checked finalization fragments even after typed approval failure. This is not a
public exec/result/event/TTY/ASCII/storage writer or P2g ordered shutdown.

## Actual return witness, not a latch exemption

Only `input::Invocation::run().await` creates the opaque `Run`. No caller outcome,
status, bytes, Prepared, gate, seeds or config constructs a terminal snapshot.
`Run::terminal` permanently closes the SAME actual InvocationGate **without setting
or clearing its separate approval fact**, then transfers the original Run into the
private owner. There is no execution/resume/Runtime accessor or reopening method.

The actual Runtime issues TerminalScope only against that matching Run witness:
both state and gate Arc identity must match, and execution must already be closed.
A fresh equal runtime/config/state cannot bind. Calling the internal owner factory
without canonical sealing does not open its publication lease. Known unexpected
suspension (including a typed suspended AgentError) refuses publication; the owner
retains the original Run/outcome/state rather than manufacturing a completed turn.

Before sealing, existing runtime behavior is unchanged. After sealing, the early
`run_turn` guard prevents later input/state/provider effects, and existing all-role
provider/tool/middleware terminal wrappers refuse later dispatch. Closed tools use
typed DeniedCapability, not fabricated NeedsApproval. A genuine existing approval
latch still dominates. Fixed closed-tool diagnostics use the SAME captured seeds.
Closing execution alone cannot mint any publication scope.

This is a logical deny-only seal after the main future returned, **not proof of
all-role/process/IO quiescence**, rollback of started blocking work, transport/read
cancellation, OS containment or the public R7 started-call/final-result ledger.
Foreground/private builder boundaries remain; no child/registry entry is dropped
to fake terminal status. P2g and the remaining producer/lifecycle gates stay open.

## Independent publication lease and typed epochs

Each Terminal owns a unique private publication authority. `close()` irreversibly
revokes it without changing execution permissions or the actual approval fact.
Snapshots bind this authority AND the original runtime state/gate; another fresh
owner cannot bind even with equal data. Snapshot/Terminal have no Debug/Serialize,
raw constructor or IO interface.

A snapshot records the actual immutable outer return variant plus the current
monotonic typed approval bit. That bit is its publication epoch. Before/after codec
work (including failure) and on every bytes/facts/count/binding read, the lease,
execution seal and epoch are checked. If late NeedsApproval changes false to true,
the old fragment is superseded. A fresh capture retains the original return fact
and observes the real new approval bit. It does not reset the failed execution gate
or rewrite a genuine earlier return into a fabricated public outcome.

Actual publication revocation/epoch changes dominate codec errors too. Diagnostic
strings, arbitrary final-answer phrases and tool-error booleans never create a
latch or determine task correctness, refusal, public status, exit or stop reason.
Test-only after-work hooks use the actual same gate/lease; production accepts none.

## Checked private fragment and conservative bounds

The private fragment contains only:

```text
run_return = returned | approval_required | turn_failed | construction_failed
approval_required = actual typed fail-latch observation
final_output = checked original TurnOutput.final_text(), or null when absent
message_scope = retained_state
messages = whole checked actual main retained messages
messages_omitted = 0
```

`run_return` is a private coarse observation of the real stored return variant,
**not a public completed/refusal/limit/provider/verification/exit/ACP mapping**.
Raw inner errors remain owned/private; their diagnostic bodies are not echoed or
interpreted as authority. Absent failure output is null, not an invented empty
answer. An actually returned empty answer remains an empty string. A late approval
bit can coexist with the original returned fact; future public mapping must honor
that typed stop while retaining genuine observations.

Only the actual state's lock is used for whole message/argument admission before
copies and finite framing. The immutable original final-text byte admission precedes
message copies; checked final text is independent of the last retained message.
No whole raw AgentState clone is formatted. Original data/order/roles/ids/names/
argument keys/scalars/error flags and the Run outcome remain unchanged or refuse.
Malformed/string/non-object arguments are never repaired into execution aliases.

The existing message codec is unchanged: 256 messages/calls,128-scalar identities,
combined1MiB source/view work, depth64/nodes100k and exact1MiB encoded message frame.
The final-text channel has its own original/view1MiB work bound before copying;
expanded/escaped size can refuse sooner than a public R2 allowance. Complete prefix
and message fragments use the SAME unchanged finite encoder/literal checks; the
combined exact private fragment is <=1MiB. New schema/tag/boolean/null/escaping/join
collisions refuse fixed. No answer truncation, JSON repair, raw spill or safe-looking
prefix is returned. Terminal message reduction/selection and full public R2/16MiB
output support remain separate; no unadmitted source is rescued by omission. A new
private `Terminal::selection` now preserves complete checked output/facts and reduces
only whole retained messages after both channels pass admission. Whole snapshot
behavior is unchanged; see [terminal-message-selection.md](terminal-message-selection.md). An independent private
ASCII encoder now preserves its authority while projecting/checking complete
channels before exact escaped-byte selection; see [terminal-ascii.md](terminal-ascii.md).
Public result/OS/IO adapters remain separate. A private human display adapter now
uses only this owner's immutable original final answer and fixed private facts,
not retained-message/ASCII decoding. Complete redaction/control/runner checks and
LF/byte bounds retain this SAME lease/epoch/seal; public text eligibility and real
writers remain separate. See [terminal-human.md](terminal-human.md).

Files/todos/custom/context/registry/inbox/child state, provider control, raw error
bodies, provenance and other channels are not implicitly serialized. Snapshot bytes
are immutable after later state changes, not a current/all-role/complete-history
ledger. A future IO writer must still check the lease at publication; already-read
references cannot be magically retracted, and no concurrent atomic-send claim follows.

## Compatibility, evidence and remaining gates

No public `craftsman exec`, stdin/flag validation, result/event/ASCII/runner-command/
output-file/trace/storage writer, headless gadget callable/catalog, token meter or
origin publication is enabled. #97 usage admission, typed successful-response/public
outcome mapping, other producer/state/storage boundaries, P2g, live/default-on/release
remain open. The separate pending #112 work is not included, retried or bypassed.
Framework/SDK/WIT/deps/lock/pin, ordinary frontends/provider/host/process limits,
schemas/frames/goldens/trace-v1/old corpus and historical baseline/targets stay intact.

Actual returned-input/shared-builder HTTP fixtures cover correct origin/closure,
actual approval failure with preserved partial state, no subsequent compaction
traffic/tools/input, absence versus true empty output, independent final answer,
foreign binding, frozen seeds, typed late epochs/revocation on success/failure,
non-latching strings and original-state/control preservation. Bounds and scalar/
schema/escaping/marker cases exercise the same existing codec.

The first compilation failed only in a new fixture: StateFs has `new(state)`, not
`default()`. The fixture constructor was corrected without changing production
behavior, older tests or native limits. The corrected initial38 focused fixtures
passed; additional admission/depth/count boundary checks follow. Final bounded results are
recorded below; no failed attempt, full local workspace, strict-Clippy,
live-quality or public rollout is inferred. Every partial implementation includes
documentation; ALL FOUR exact-final-head CI jobs/every mandatory step, Windows
included, gate any missing-review-only admin merge. Substantive review/failed checks
are never bypassed.

## Collected local verification

| Check | Result |
| --- | --- |
| Actual-Run terminal fixtures, feature-off / gadgets | 43 passed in each build, zero failed |
| Unchanged retained-message view / selection fixtures | 28 / 39 passed |
| Existing runtime / input / provider / LLM fixtures | 42 / 30 / 47 / 10 passed, zero failed |
| Both production feature-off/gadget checks | Passed without warnings |
| Offline accepted exec contract / benchmark driver | 126 cases / 9 tests passed |
| Formatting / whitespace / lineage / protected-source audit | Passed |
| Current Markdown fences / local links | Balanced; 24 links resolved |

These are distinct bounded local checks on approved f99ef83, not a full local native
workspace or strict-Clippy run. The new fixture's failed first compile remains
recorded above; only its constructor was corrected. ALL FOUR exact-final-head CI
jobs/every mandatory step, including Windows, still gate merge.

## #121 merge evidence

ALL FOUR exact-final-head jobs/every mandatory step passed on October24: Linux/macOS
main938 feature-off /1079 gadgets, Windows915 /1045; zero failed. All43 authority
fixtures passed in both builds on all OSes. Only missing review was overridden after
fresh rules/head/base/reviews/comments/threads audit. Squash7d48ddd has the identical
four-job tested tree and the audited base as its sole parent.


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
