# Private shared logical-request admission (R5)

**Status: November 1, 2026 (Asia/Taipei): implemented private request-only
foundation, not public `craftsman exec` or a complete token/usage meter.** The accepted
[token-admission contract](token-admission.md) merged as #133; #97 is closed for
its representation/compatibility decision. This implementation does not reopen
that decision or enable token-budget flags, public results/events or release gates.

**November 5:** a separate private
[request-budget argv adapter](request-budget-arguments.md) now chooses this cap
using `--max-rounds` before input-first preparation. Default and earlier argv
routes retain their original behavior; no public command or usage producer ships.

## Actual-builder source and scope

`exec::meter` is app-layer code. `Runtime::build` selects the accepted default
50-call ceiling; a separate private factory accepts an opaque `RequestLimit`
validated in1..10000. Select that pure typed value before preparation. No ambient
config, project setting or model text can change the cap after construction.
Existing framework loop guards still apply: a ceiling is not a promise to keep
running until that many calls. Earlier private argv adapters still reject limit
flags. Only the explicitly selected request-budget adapter accepts `--max-rounds`;
unsupported token/deadline/public-output controls remain rejected.

The SAME shared session-construction core creates one meter from an opaque
`agent::RequestMeterScope` and its actual captured invocation gate. Each of the
main, subagent and compaction providers is then bound through an opaque
`RequestProviderScope` containing the ORIGINAL selected/gated provider, concrete
role and that same meter/gate. The counter wrapper accepts ONLY that scope, never
an arbitrary provider, caller gate, model-derived role or diagnostic status.

Core construction rejects headless-without-meter, foreign gate/meter and
frontend-with-meter BEFORE other builder effects. `SessionExtras` and the existing
`SessionConstruction`/`ProviderScope` shapes do not gain an opt-in escape hatch.
The three provider wrappers sit outside middleware/profile selection; excluding a
middleware cannot remove them or restore a raw provider. Ordinary TUI/legacy `-m`,
service and frozen desktop/ACP frontends do not select this private meter.

## Linearized admission, not HTTP or token billing

One bounded mutex owns the total, three fixed role rows and first refusal. No
await or provider execution occurs while that mutex is held. Every accepted
wrapper invocation increments the global and concrete-role admission count once
before entering its supplied inner provider. The last allowed response may finish
normally: equality with the ceiling alone is NOT a refusal or failure heuristic.

A later attempt at the full ceiling records the actual maximum/observed/role,
closes the SAME invocation gate deny-only and returns a fixed error WITHOUT
incrementing or entering the supplied provider. Repeated refusals retain that
first fact. This closure is not `NeedsApproval`; true typed approval remains
independent and dominates runtime classification before/after a turn. A caught
child/compaction error cannot be relabeled as completion or dispatch later
registered tools past the seal. No string matching grants or stops the run.

A non-clone permit records exactly one observed supplied-inner return or abandoned
polled future. For each role:

```text
admitted = active + returned_ok + returned_error + abandoned
sum(role.admitted) = total_admitted <= maximum
```

Unpolled futures have not admitted a call. Failure, cancellation or Drop releases
`active` but NEVER refunds admitted calls, resets history or fabricates a return.
A late stop hides final-success eligibility without erasing the real inner-return
observation. A poisoned/incoherent counter fails fixed/closed; it is not repaired,
zeroed, wrapped or converted into a fictitious cap refusal. The three-role sums
use wider arithmetic and every update is checked.

These are actual **logical wrapper-admission observations**, NOT HTTP-send/receipt
counts, retry ordinals, token usage, reported/estimated-call classification,
unknown billing, public status or a completed usage ledger. An admitted provider
can subsequently reject its checked request or never initiate traffic. The token
slice must put combined request/token admission before the charge point; it must
not add a token check afterward and then refund/hide this history to manufacture
#97's zero-call refusal. No token allowance, output clamp or failed-attempt billing
is implemented here.

## Private observations and stop limits

Only an actual input `Run` selects `RequestCounts`; it has no raw constructor,
Debug or Serialize. It contains an immutable, bounded point-in-time snapshot from
that runtime, with read-only cap, role rows and first refusal. Equal numbers from
two fresh runtimes are not interchangeable authority. Foreign snapshots/gates
fail binding instead of being inferred from shared permissions or identical model
names. Snapshots are historical facts, not a live or revocable public record;
public producers must later pair facts with actual terminal/approval authority and
seeded scalar/framing verification. Existing private terminal ASCII/human/file
previews remain previews, not schema result producers.

The private runtime returns typed request-limit/accounting failures; its existing
terminal preview classifies them as private turn failure, not a new public stop
reason or exit mapping. Original partial state/registry and genuine observations
remain private. `max_tokens`/exit3/public limit metadata and trace-v1 are unchanged.

Gate closure prevents later guarded admissions/registered tool dispatch. It does
NOT notify or interrupt an already-started native/HTTP/blocking operation, reclaim
forgotten futures, undo side effects, supply P2g, or prove all-role/process/OS
quiescence. `active=0` only describes these provider permits' returned/dropped
futures; it is not a claim that started external work has stopped.

## Evidence and unchanged boundaries

Forty additive tests cover27 actual-builder/synthetic-provider cases and13 actual
loopback/input/runtime cases:1/50/10000 and invalid bounds, all concrete roles,
concurrent admission, exact last response, error/unpolled/drop observations,
first-refusal preservation, genuine approval dominance, foreign/equal source
identity, no raw construction fallback, and poison failure. Real main, named/
general-purpose child and manual/automatic compaction requests retain their
ORIGINAL histories; same-response allowed writes stay denied after budget stop.
No model keys, global environment, real user config/SDK or guest execution is used.

Initial evidence includes one new integration dead-code warning for the old pair-
return builder adapter after production began retaining the triple. That adapter
is now fixture-only and still calls the real builder; no `allow` suppression or
old test assertion changed. Initial NEW scope/test-access privacy and guessed
input-constructor failures are retained. Corrections only complete the new opaque
provider binding, add a test-only read facade/owned poison hook and fix new source
data. Production snapshot visibility remains exec-only. They are not passing rows.

One broader gadgets-mode local agent row also recorded an UNCHANGED old partial-
shell fixture failing all three existing six-second attempts with no original
bytes/ready marker. Its failure remains failed evidence. One exact isolated
DIAGNOSTIC run passed the same two-format readiness/11-byte/timeout assertions
without fixture/process/deadline changes; that does not relabel the broader row
as green. Remaining local rows continue without a full restart, and fresh whole-
head CI must pass that unchanged case in BOTH modes/on every compiled platform
before merge. No old assertion, attempt ceiling or timing is weakened.

Every old test body/assertion, native input rights/modes/EOF/restore/deadline,
filesystem/writer/guest/host limit, framework/SDK/WIT, dependency/lock/Rust pin,
frontend definition/SafeBackend, v4/standard ACP, schema/vector/golden/trace/corpus
stays unchanged. Existing production additions are limited to actual-builder
source/three role wrappers, private runtime classification/snapshot selection,
and the actual Run's observation path. Documentation is part of this slice.
All FOUR fresh exact-head jobs AND EVERY mandatory/setup step/BOTH feature modes/
Windows plus raw test/source/warning/review proof gate merge; historical #133
contract CI is not substituted for this new source. No unchanged full-CI retry,
old assertion/deadline loosening or independently blocked provider workaround.

## Remaining work

The next R5 slice needs real checked-request/receipt/partial-output witnesses,
existing-tokenizer fallback for missing usage, request+token reservations in ONE
atomic admission decision, requested-output clamping, concurrent settlement and
truthful reported/estimated/unknown billing. Then the public result/event/trace/
seeded storage adapters must source actual facts consistently. Public exec/full
flags/gadget callable/catalog/provenance/P2g/live/default-on/release remain separate
and unwired; this request-only foundation cannot clear them by itself.


Current source-local verification completed without a full restart:14 of15 bounded
rows passed warning-free, with new40 cases in BOTH modes, all protected exec/input/
reader/composition/terminal suites, old guest37, locked production checks, frozen
native goldens/contracts/loop tests, reference196, driver9, formatting/lineage/
source/Markdown checks. The15th broader agent-gadgets row remains a recorded failure
of the old native-readiness case; its one isolated unchanged diagnostic passed,
not a substitute green broad row. Fresh exact-head full CI and both-mode raw proof
of that old case are mandatory before merge. The15-path audit preserves every old
test body/assertion, all other tracked source and the historical plan prefix.


## Private atomic token reservations — November2 (Asia/Taipei)

[token-reservations.md](token-reservations.md) records a private R5 opt-in foundation.
The SAME builder binds concrete scoped clients/roles to one meter; bounded whole
checked-input estimates and final reply-body checks precede ONE request/token
allocation decision. Default request-only paths remain unchanged; only a separate
typed private factory opts in, with no public flags/CLI. Allocations stay active or
retained unsettled, never inferred measured usage, refunded calls or #97 public
observed/remaining facts. Fifty additive tests cover27 actual-builder/source/client/
concurrency cases,12 real input/runtime/child/compaction cases and11 arithmetic/
estimator cases. Real histories stay intact; initial NEW `--lib` driver failure is
preserved, corrected only by selecting the binary test target.

Receipt/partial-output witnesses, usage-presence/fallback, concurrent settlement/
public ledger and all other producer/P2g/gadget/live/release gates remain separate.
All old assertions, source codecs/response control/native limits/framework/SDK/WIT/
deps/lock/pin/frontends/protocols/schemas/goldens/trace/history remain protected;
additive tests/docs and ALL FOUR fresh exact-head CI jobs/every mandatory step/both
modes/Windows plus source/warning/review proof precede any missing-review-only merge.


### CI image correction — November2 (Asia/Taipei)

The first #135 head's macOS job was cancelled in four seconds BEFORE runner
assignment/checkout/any test. GitHub's official runner-images#13518 and check
annotation identify the macOS14 retirement brownout (November1 14:00–November2
00:00 UTC); full retirement is November2, 2026. That cancellation is preserved,
not a source-test failure or green evidence, and is DISTINCT from the unchanged
local partial-shell readiness failure above.

`ci.yml` now separates stable check ids from actual images: required
`test (macos-14)` retains its exact context name but runs on explicit `macos-15`.
Ubuntu24.04/Windows2025 names/images and EVERY action/test/build/limit/condition
remain unchanged; no ruleset/branch protection mutation, macos-latest alias,
old assertion relaxation or unchanged full-CI retry. Only real image selection
and current docs change; every Rust byte from the50-case implementation/local
rows is identical. Fresh corrected-head ALL FOUR jobs/every mandatory/setup step/
both modes/Windows plus ACTUAL macOS15 labels/raw logs/source/warning/review proof
are required. Do not call the legacy name macOS14 test coverage or reuse the
cancelled original head's status as coverage of the corrected workflow.


## Private admission-correlated send/header observations — November2 (Asia/Taipei)

[admission-receipts.md](admission-receipts.md) records the next private R5 slice after
#135. Only actual token-admitted body/permit/checked-client identity mints the scope;
SAME builder/meter/gate and concrete main/subagent/compaction keys bind bounded
<=10000 calls/two slots. Client-only witnesses record ACTUAL first send-future poll,
awaited send error or returned HTTP headers; preparation/no-poll, abandonment and
late stop retain truthful distinct facts. No socket/wire/billing/body completion/
usage/settlement inference, extra retry request charge, fake status/bytes or refund.
Default request-only/frontends/flags remain unchanged; only actual Run selects
non-Debug/non-Serialize historical source-bound observations. Forty additive tests
(10 builder/identity,20 actual runtime/loopback,10 coherence/bounds) and docs accompany
the incomplete adapter. Initial5 NEW unused test interfaces are preserved/corrected
only by completing new actual-owner coverage; no suppression/old assertions changed.

Original successful SSE/control/usage/retry/diagnostics, source codecs, native/guest/
input/writer/host limits, framework/SDK/WIT/deps/lock/pin/frontends/SafeBackend/v4/ACP/
schemas/vectors/goldens/trace/history stay unchanged. No independently blocked provider
inspection/execution/workaround. Actual bounded successful response/usage presence/
partial-output, billing/settlement/overrun/public producers/provenance/P2g remain
separate; exec/callable headless gadgets/catalog/live/default-on/release stay OFF.
ALL FOUR fresh exact-head CI jobs/every mandatory/setup step/both modes/Windows plus
actual macOS15 label/source-warning-tree-review proof precede missing-review-only merge.


### Required engine security patch — November2 (Asia/Taipei)

During this slice GitHub surfaced two critical manifest/lock alerts for the SAME
Wasmtime AArch64/large-br_table advisory GHSA-mghf-wjfp-p5xm. The advisory/48.0.7 fix
were published October19, 2026; our repository alerts appeared November1 at20:06UTC.
This is newly surfaced evidence, not a new November2 advisory or proof that earlier
48.0.6 was safe. Vendor48.0.8 (October29) additionally fixes component/WASI issues;
both direct host pins now use exact48.0.8. Rust stays1.99.1, SDK/WIT/guest-host policy/
capabilities/production limits/defaults/assertions and CI/check names remain unchanged.

A separate necessary dependency commit updates ONLY the two host pins and30 existing
Wasmtime/Pulley/Cranelift version/checksum entries; no dependency edges/features/
registry sources, unrelated libraries, standalone gadget SDK sources/lockfiles or
Rust toolchain changes are bundled.
Three additive offline guards pin the reviewed matching runtime/WASI versions/family
and original feature/model-opt-in shape; no exploit or unpatched guest is executed
by these guards. The local machine is nativeARM64. Fresh patched-head locked checks,
all43 NEW receipt/security tests, protected native/guest/SDK/WIT suites and ALL FOUR
CI jobs/every mandatory/setup step/both modes/Windows/actual macOS15 evidence gate merge.

All15 earlier bounded local rows passed warning-free BEFORE this prerequisite; they
are retained as pre-security evidence, NOT relabeled patched-head coverage. No whole
unchanged retry/empty commit/assertion/deadline weakening; only the real dependency
security update justifies fresh source validation. Other open advisories, public
exec/usage settlement, full containment/P2g/live/default-on/release remain separate.
