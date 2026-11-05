# Accepted token-admission refusal clarification (#97)

**Status: November 1, 2026 (Asia/Taipei): explicitly accepted contract with
implemented conformance artifacts; shared meter and public output remain unwired.**
R1–R7 remain accepted as recorded on October20. This narrow decision resolves #97's
representation/compatibility question; close that issue only after the accepted
artifacts merge with all four exact-head CI jobs and every mandatory step passing.
No shared meter, provider admission, public `craftsman exec`, result/event producer,
live/default-on or release behavior is enabled by accepting these artifacts.

## Explicit acceptance record — November 1, 2026

The user replied **"yes approve"** to the specific request to approve:

- Optional `_meta.craftsman.limit.admission` with `estimated_input_tokens`,
  `requested_output_tokens` and `remaining_tokens`.
- Strict estimated input PLUS actual bounded required reply > remaining refusal,
  while actual aggregate/role usage and admitted logical-call counts stay truthful.
- The existing empty-string initial output, limit/exit3/standard `max_tokens`,
  strict integer/u64/no-wrap rules and exhausted/reported-overrun variant.
- Keeping the v1 ids, with the documented older-semantic-validator update and
  previously unspecified optional-name collision risk below.

This is explicit acceptance of that contract and compatibility judgment, not a
formal repository review or inferred approval from October31's general continue
instruction, CI, or an admin override. It authorizes preparing the private shared
admission/usage foundation; concurrent settlement, actual provider witnesses,
remaining producer boundaries and public/live/release gates still require their
implementation evidence. It does not change the October20 R1–R7 decision.

## The uncovered case

A run has a token budget of100. One actual admitted main call reported30 input
and0 output tokens, so its role ledger and aggregate usage are30 and `rounds=1`.
The next request needs estimated80 input plus at least1 reply token; only70 remain.
R5 already requires refusing it BEFORE provider traffic/admission. Usage must stay
30, not be inflated to100 or111, and there is no extra logical request/usage event.

The old structural schema accepts the truthful30/100 record. The old semantic
checker rejects it because it requires every token-limit observation to reach
or exceed the maximum. Baseline reproduction, with and without the proposed
optional detail, is preserved separately from passing current-source evidence.

## Accepted representation

Keep the existing outcome, units, counts and standard ACP vocabulary:

```json
{
  "kind": "tokens",
  "maximum": 100,
  "observed": 30,
  "unit": "tokens",
  "admission": {
    "estimated_input_tokens": 80,
    "requested_output_tokens": 1,
    "remaining_tokens": 70
  }
}
```

This is `_meta.craftsman.limit` inside the existing result. The new optional
`admission` describes the refused request's **reservation**, not measured usage,
a provider call, billing precision or a new limit unit. Estimated input is
nonnegative; the bounded requested reply reservation is at least1. All three
fields are integer literals in0..u64::MAX (reply starts at1), never floats,
booleans, numeric strings or repaired values.
The reply figure must be the actual bounded required reservation used for that
refusal, not an inflated model maximum chosen to manufacture a limit. Allocation/
clamping and minimum-reply policy still belong to the reviewed future meter.

Two nonoverlapping modes are accepted:

1. **Exhausted/reported-overrun ledger:** no `admission`; existing `observed >=
   maximum` behavior is unchanged, including reported overrun.
2. **Insufficient remaining admission:** `admission` exists, kind is `tokens`,
   and `observed < maximum`. Remaining must equal `maximum - observed`, and
   estimated input PLUS requested reply must be STRICTLY greater than remaining.
   An exact fit is not an admission-limit refusal. Arithmetic must not wrap at
   the u64 boundary; native tests use wider arithmetic for the sum.

Maximum still equals `limits.token_budget`; observed still equals aggregate
input/output usage AND the role sums. Missing/incoherent admission context does
not waive the old limit checks. Request and wall limits cannot carry this detail.
Status remains `limit`, exit remains3, stop/reason remain `max_tokens`, error kind
remains `limit`, and retryability/signal remain unchanged. No new ACP stop reason.

The initial request can fail reservation with zero usage/requests/rounds and no
model-usage event. Its existing schema-backed output is an EMPTY STRING, not the
nullable private terminal-preview shape; messages are empty. Partial output and
whole retained messages from earlier calls stay intact for later refusals.

## What this does not prove

The offline checker validates a finished record's stated reservation and ledger,
not the historical fact that a provider request was admitted/refused. Positive
vectors pin existing call counts, usage and partial output; stream vectors reject
extra/missing usage events or inconsistent final counts. A producer MUST source
these facts from actual admission/receipt witnesses, never fabricate a coherent
ledger just to pass validation.

This contract does not define a concurrent meter, in-flight reservation settlement,
request-source identity, billing estimator, tokenization algorithm or all-role/OS
quiescence. A future meter must reconcile actual admitted calls/estimates with the
published ledger and source the refusal detail consistently. In-flight/cancellation
edge cases need reviewed implementation evidence before wiring; they are not
silently solved by `remaining = maximum - observed` in a validation artifact.

## Accepted compatibility judgment and consumer obligations

- Existing schema ids, result/event wrappers, required existing keys, units,
  outcome mappings, role counts and standard ACP stops remain unchanged. The
  common exec schema adds ONLY optional `limit.admission`, one local definition,
  and its token-only structural condition. Trace-v1, desktop-v4 and standard ACP
  schemas/frames are unchanged.
- All25 original fixtures and all126 original reference cases retain their values
  AND serialized entry text; only new fixtures/cases and required separator commas
  are appended. Existing exhausted/overrun cases are not rewritten to hide #97.
  The older three native schema-test bodies/assertions are byte-exact after one
  test-only module declaration.
- The old schema ignores the optional new field structurally, but the old semantic
  validator still rejects pre-exhaustion refusal. Consumers doing semantic checks
  MUST update to understand this variant; there is no claim of transparent
  compatibility with an old validator.
- `admission` was previously an unspecified extra member in the owned Craftsman
  namespace. Recognizing it tightens validation for any old arbitrary data that
  happened to use that name. This potential collision is explicit, not an assertion
  that EVERY previously schema-valid arbitrary JSON value remains accepted.
- Keeping the v1 ids was explicitly accepted November1 because the optional
  variant preserves existing defined valid records. This does NOT promise that
  arbitrary older extras using `admission` remain valid or that an unchanged old
  semantic validator understands the new variant; consumers must update.
- October31's general continue instruction was not acceptance. The later specific
  November1 approval supplies it. #97 may close after accepted-artifact merge and
  exact-head CI/source/review verification, not as a claim that the meter ships.
  No public producer or completed-turn classification follows from that closure.

## Independent review evidence

The updated reference self-test covers141 record/stream vectors,33 rules and22
wire cases:196 total, including70 new admission cases and all original126. New
cases cover main, initial-zero, subagent, compaction, mixed all-role, estimated/
unknown-billing and u64-sum boundaries; coherent context, exact-fit/non-refusal,
missing/wrong/incoherent values, preserved partial output/counts, no new stop reason,
stream usage/count consistency, integer literals, ASCII, duplicate members and LF.

Eight new native tests independently check the61 new record/stream cases (16
valid/45 invalid), actual retained facts, integer arithmetic and role/event sums;
they do not invoke the reference checker or a model. The original three native schema tests also
continue to run, including stable wrapper ids and standard stop-reason rejection.
Native checks are scoped to these admission vectors, not advertised as a complete
independent wire/event validator. Wire cases remain reference-checker evidence.

The first broadened NEW fixtures used null output and omitted existing run-start
entry/model/profile fields; their failures are preserved. Only NEW fixture data
was corrected to the existing accepted output-string/run-start requirements. No
existing schema requirement, old vector assertion, deadline, reader/writer/host/
fixture, framework/SDK/WIT, dependency/lock/Rust pin, frontend/golden/trace/corpus
was relaxed or worked around. No independently blocked provider correction was
inspected, executed, changed, retried or worked around.

Documentation accompanies this accepted contract. All FOUR fresh exact-head CI
jobs AND EVERY mandatory step/BOTH feature modes/Windows and the warning/source/
review audit still gate merge. Passing conformance is not runtime enforcement,
default-on/live quality, dependency-security immunity or release approval; explicit
compatibility acceptance comes from the user decision above, not from those tests.


Historical pre-acceptance source-local evidence: all38 valid bounded rows passed warning-free,
including native11 in BOTH modes, reference196, all protected scope/control/source/
native/composition/runtime/guest suites, both locked production checks, existing
marked/unmarked compile probes, source/CLI/compatibility/Markdown/fmt/lineage checks.
The exact12-path audit preserves runtime/framework/SDK/WIT/deps/lock/pin/frontends/
protocols/goldens/trace/corpus and original25/126 serialized entries;62 local links
and balanced fences pass. Initial NEW fixture errors, a NEW comment-wording lineage
failure, and an unsupported EXTRA trace-driver switch are preserved, not called
passing evidence. Only new fixture/comment/driver setup was corrected; no old
check/allow-list/trace/source/limit was relaxed and no full suite/CI retry was used.
Those rows were checked on the pre-acceptance source and are not relabeled as
fresh tests of this documentation-only acceptance update. All runtime/test/schema/
checker/vector bytes remain unchanged; fresh exact-head CI and document/source
verification still gate this update. #97's contract decision is accepted, while
meter/public producer/live/release remain unwired/off.


## Implementation follow-up — private request-only source

The accepted artifacts merged as #133 and #97 is closed for the contract decision.
[request-admission.md](request-admission.md) now records a separate private shared
logical-request cap and actual-builder source/role bindings. It does not implement
this token reservation or a public usage/result producer. Combined token/request
admission, actual receipts/estimates/settlement and public ledger/provenance/trace
remain implementation work; do not charge then refund a refused token request or
reinterpret the new private wrapper counters as a complete R5 ledger.


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
