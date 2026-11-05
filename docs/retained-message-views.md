# Private checked retained-message snapshots

**Status: October 23, 2026 (UTC).** The runtime can now produce a bounded checked
view of its actual main retained messages. This is not a public result/event,
transcript ledger, storage adapter or shipped `craftsman exec` command. Original state,
arguments and execution remain separate. Pending provider response PR #112 is not
included or implied to have merged.

## Actual runtime authority

Only `Runtime::message_scope()` pairs the actual agent-state handle, preparation's
frozen redactor and the SAME invocation fail latch. Its fields cannot be supplied
by callers. `Runtime::message_view()` holds that actual state lock throughout whole
source admission, copied data views and bounded finite JSON encoding. It never
clones the entire `AgentState` before checking its message data.

`input::Run::message_view()` selects that same path after an admitted input turn;
it does not format the old raw retained-state clone. `View` owns only immutable
checked bytes/count and borrows its originating runtime scope. It has no raw-state,
executable-argument, arbitrary-byte constructor, `Debug` or `Serialize` interface.
Binding rejects a different runtime even if its config, state and permissions are
equal. An older snapshot remains a snapshot after later state changes; it is not
represented as current, concurrent or an ordered all-role ledger.

The actual gate is checked before capture, after finite work (also on codec failure)
and before byte/count access. A later typed approval stop hides existing views and
refuses new capture; strings such as `NeedsApproval` never set or authorize that
latch. Terminal/approval-required final-result publication remains a separate gate;
this implementation does not reopen a failed model/tool invocation for output.

## Whole admitted messages, not aliases

The finite codec reuses the existing request producer's literal/text/structured
budget and bounded encoder. A complete first pass admits every selected role's
original content/identity strings and decoded argument nodes/keys BEFORE any
message or argument copy. It then creates separate data views and encodes a fixed
private frame:

```text
message_scope = retained_state
messages = original role/order/flags/identities, with separately checked data views
messages_omitted = 0 (all current retained records, or refusal with no frame)
```

The four actual message roles, original tool-error booleans, tool ids/names and
argument keys/scalars are preserved or refused. Decoded argument data is redacted
before encoding. Non-object/string argument shapes remain those shapes; they are
not parsed/repaired into execution control. No redacted label, id, argument or view
is substituted into the original state or reversed into an authority alias.

Only `AgentState.messages` is selected. Files, todos, custom values, system extras,
registry/inbox/child state and other channels stay raw/private and are NOT implicitly
published by serializing `AgentState`. Delegated content may appear only as actually
retained main messages, not as a claimed complete child transcript.

## Bounds and refusal

- Private capture: at most 256 current retained messages and 256 calls per assistant.
- Identity fields: at most 128 Unicode scalars, never clipped into another id/name.
- Shared whole original/view work: 1 MiB, depth 64 and 100k node operations.
- Exact finite encoded frame: at most 1 MiB, with whole encoded-literal verification.
- Literal/structured key, scalar, replacement and escaping/framing collisions fail
  fixed; no lossy decoding or safe-looking repaired/truncated JSON prefix is returned.

These are deliberately private resource gates, not promises that every accepted
R2 input or post-compaction state fits a public result. The combined original/view
work budget is separate from encoded byte length. Adjacent/overlapping literal
matches retain the existing redactor's coalescing semantics. Literal verification
checks generated JSON keys/roles/scalars and escaping joins, not only source text.

The #119 capture itself has no message reducer. A too-large/unsafe state is refused whole,
not silently omitted. `messages_omitted=0` describes this finite retained-state frame,
not messages lost to earlier compaction. Public output bounds, whole-message omission
selection and final-outcome retention still require their own reviewed public adapters.
A separate private selector now consumes only this admitted View, keeps a newest
whole-message suffix and rechecks exact omission/framing bytes; see
[message-selection.md](message-selection.md). It never rescues unadmitted source
or changes the original outcome/state. A separate PRIVATE actual-Run authority now
owns terminal finalization (`terminal-publication.md`) without exempting/reopening
this live view's fail latch; public IO/status/storage remain unwired. Its private terminal selector
(`terminal-message-selection.md`) reduces only already admitted whole records and
keeps the original checked answer/facts, not raw state or an execution alias. Its separate
private ASCII adapter (`terminal-ascii.md`) preserves original source admission and
performs complete post-escape verification; no raw constructor/public writer is added.
A separate private human renderer (`terminal-human.md`) now selects only immutable
actual final output/fixed private diagnostics, not this retained state or decoded
frames. Visible control/runner display checks retain terminal lease/typed epochs;
this view's original source caps/live latch and public IO/storage gates are unchanged.

## Compatibility and verification

Original provider request construction and its codec semantics remain unchanged;
only a separate private finite-message function selects the same helpers. The
actual builder/runtime turn, original state and input/control/auth are unchanged.
No public CLI, stdout/event/result/trace writer, raw checkpoint/journal, meter,
callable headless gadget/catalog or new frontend option is enabled. Framework/SDK/
WIT/root dependencies/lockfile/toolchain, old hosts/process limits, schemas/frames/
goldens/trace-v1 and the historical October 16 baseline/Appendix A/targets remain.

Focused actual-runtime fixtures cover all four roles, immutable original args/state,
key/id/name/scalar/encoded-literal collisions, short/Unicode literals, private size/
count/depth/node/escaping bounds, ignored non-message state, retained-state replacement,
foreign-runtime binding, frozen config, typed late stops and non-latching strings.
Real synthetic HTTP/shared-builder fixtures and the admitted-input Run select these
views while original state remains unchanged and raw stores stay disconnected.
Local verification on October 23, after fast-forwarding to main's security-only
lockfile update (`87d5105`), is recorded below. These are bounded, distinct checks,
not a completed full local workspace run, strict Clippy, live quality or public
rollout proof. CI's full workspace and Windows evidence remain independent.

| Local check | Result |
| --- | --- |
| Actual-runtime retained-message fixtures, feature-off | 28 passed, 0 failed |
| Same retained-message fixtures, gadgets enabled | 28 passed, 0 failed |
| Existing private runtime fixtures | 42 passed, 0 failed |
| Existing input fixtures | 30 passed, 0 failed |
| Existing provider request/diagnostic fixtures | 47 passed, 0 failed |
| Existing LLM fixtures | 10 passed, 0 failed |
| Production `cargo check --locked --no-default-features -p craftsman-rs` | Passed |
| Same production check with `--features craftsman-rs/gadgets` | Passed |
| Offline accepted exec contract | 126 cases passed (80 records/streams, 33 rules, 13 wire cases) |
| Offline gadget benchmark driver contracts | 9 passed |
| Formatting, whitespace, lineage, protected-source/dependency/schema/golden audit | Passed |
| Current Markdown fences and local links | Balanced; 17 local links resolved |

The first feature-off focused run on the original base had 26 passes and two
failures: new fixture expectations incorrectly expected separate markers for
adjacent literal matches. Only those two new expectations were corrected to the
existing coalescing semantics; no redactor/producer behavior or older assertion
changed. The corrected focused run passed all 28 cases on that base and again
after the main lockfile update. Failed attempts are not relabeled green.

Every partial implementation includes documentation. ALL FOUR exact-final-head CI
jobs and every mandatory step, Windows included, remain required before any
missing-review-only admin merge. Other producer/state/storage/provenance/input/#97/
P2g/live/default-on/release gates remain open.

## #119 merge evidence

On October23, ALL FOUR exact-final-head PR jobs and every mandatory step passed:
Linux/macOS main units856 feature-off /997 gadgets; Windows833 /963; zero failures.
All28 snapshot fixtures passed in both builds on all three OSes. Fresh rules/head/
base/reviews/comments/threads were audited; missing review alone was overridden.
Squash `6a43802` has the identical tree checked out by all four job logs and the
audited base as its sole parent. This source merge is not a public exec rollout.


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
