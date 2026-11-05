# Private bounded token preflight and atomic reservations (R5)

**Status: November 2, 2026 (Asia/Taipei): implemented private opt-in foundation; not settled token usage, public `craftsman exec`, or a shipped token-budget
flag.** R1–R7 and #97's [accepted refusal contract](token-admission.md) remain
accepted. The merged [request-only default](request-admission.md) remains the
normal private runtime path; this slice adds a distinct explicit reservation
ceiling, not a complete R5 ledger.

## Authority and ordering

The SAME session-construction core creates one meter from its opaque
`RequestMeterScope`. `SessionConstruction`, `ProviderScope` and `SessionExtras`
field shapes remain unchanged. A new opaque `TokenProviderScope` binds the
actual constructed scoped client, concrete main/subagent/compaction role and
SAME meter/gate. Every private role must select the mandatory builder helper;
frontends do not opt in. Middleware/profile exclusion cannot remove admission.
The request-only wrapper rejects a token-enabled meter rather than bypassing it.

Only the actual scoped client builds the original request through the unchanged
whole request codec. Preflight uses that client's exact source-bound `Checked`
bytes, not caller input counts, arbitrary JSON, a response, model name or role
label. The final clamped request remains bound to that SAME client source and
checked-request identity. A non-clone combined permit travels with it; ordinary
clients/legacy wire APIs cannot send it. Every existing send/retry rechecks the
SAME meter/gate and source before and after its byte copy. Existing retry limits,
transport/error paths, successful SSE parser/finalizer and response/control data
remain original/private; this is not the separately blocked response correction.

Explicit positive `TokenLimit` values use integer u64 bounds (1..u64::MAX), not
ambient/project settings or model text. Choose this pure value and the accepted
1..10000/default50 request cap before preparation. `Invocation`'s new private
opt-in retains the SAME input-first preparation. Default, file/stdin/argv routes
and all public headless limit flags remain unchanged/rejected.

## Bounded estimate and reply policy

The original source/view/node/depth/encoded-body bounds stay unchanged: 1 MiB
work/body, depth64 and100000 nodes. Only after that admission, the token preflight
parses the finite checked body. It counts the whole checked JSON input envelope:
system/messages, role/id/name data, call arguments, tool descriptions/schema and
supported generation/metadata data. The one reply-control integer is excluded;
a huge configured output ceiling must not inflate input or manufacture a refusal.

Counting reuses `TiktokenCounter`'s bundled cl100k_base encoding, in scalar-aligned
pieces of at most4096 UTF-8 bytes. Complete input is admitted before tokenization;
all pieces/counts are bounded and summed without wrap. JSON-envelope overhead,
escaping, independent pieces and provider-specific tokenizers make this an
**estimate, not an exact count or guaranteed upper bound**. This does not change
ordinary compaction's existing message counter or count returned output tokens.
No tokenizer/dependency/lockfile/Rust pin is changed.

This first private chat-completions subset supports exactly one positive integer
`max_tokens` or `max_completion_tokens`, or a private4096 reply ceiling when
neither exists. Both fields, invalid/zero/non-integer values, `n` other than1,
unknown output/provider kwargs (including hidden candidate/extra-body controls)
refuse before charging/traffic. Known sampling/tool/schema/metadata controls keep
their original checked values. This is a conservative subset, not provider-wide
compatibility or a new public default.

The encoded reply field is a REQUESTED bound, not proof that the provider honored
it. This slice does not meter successful stream/partial output or expose reported
usage overruns; a provider may exceed/ignore the requested cap. Actual reported
values/control stay original/private. End-to-end token-budget enforcement and
completed/public overrun classification remain settlement/producer gates.

Required minimum reply is1. In one bounded mutex decision:

1. Recheck actual gate, source/mode and coherent accounting.
2. Refuse an already-full request cap first if both ceilings prevent this attempt.
3. Compute unallocated allowance = reservation ceiling − already allocated.
4. Refuse iff estimated input +1 is strictly greater than that allowance.
5. Otherwise choose reply = min(original/private ceiling, allowance − input).
6. Encode and verify the WHOLE final clamped body, including new digits/joins,
   before either counter/allocation changes. A collision/limit fails fixed with
   no request charge, allocation, traffic or invented token-limit fact.
7. Atomically increment the request/role count and input/reply allocation together.

No await or provider execution occurs under the mutex. The final synchronous
encoding/verification is bounded by the unchanged1 MiB codec limit. Native wider
math/checked updates cover u64 sum/subtraction edges; exact minimum fit succeeds.
This is not a post-charge token test followed by a refund or masked history.

## Allocations deliberately remain unsettled

Private facts distinguish:

```text
allocated = sum(role.estimated_input_allocation + role.reply_allocation)
allocated = active_allocation + retained_unsettled_allocation <= ceiling
admitted = active_calls + inner_ok + inner_error + abandoned
```

A real supplied-inner return/error or a dropped polled future moves its allocation
from active to retained unsettled, **never back to free allowance**. Unpolled
futures have not admitted/reserved anything. No usage-presence/receipt witness,
streaming/partial-output estimator, retry billing or concurrent settlement exists
in this slice. In particular, `Usage {0,0}` cannot distinguish missing reporting
from actual zero. Returning successfully does not turn reservation into measured
usage; errors/drop do not fabricate zero usage or unknown-attempt receipts.

A first insufficient-allocation refusal retains role, ceiling, already allocated,
estimated input, actual minimum reply1 and unallocated remainder. It increments
neither requests nor allocations and seals execution deny-only, not NeedsApproval.
Actual typed approval dominates private Runtime classification. Exact final
response may finish without a new refusal. Poison/incoherence fails fixed/closed,
never resets, refunds, masks, wraps or manufactures coherent zero facts.

Only an actual input `Run` selects `TokenReservations`; it has no raw constructor,
Debug or Serialize. Historical snapshots retain SAME source identity; equal fresh
runtime values are not aliases. These are **reservation observations, not #97's
measured observed/remaining fields**, public status/exit/usage/event/trace records,
billing, receipt counts, token overrun classification or all-role quiescence.
Returned reservation exhaustion may conservatively refuse even when actual usage
was much smaller. A public producer must not serialize these as the accepted
usage ledger or claim that this temporary behavior implements final R5 settlement.

Sealing prevents later guarded calls/registered tools, not already-started native/
HTTP/blocking work, external side effects, forgotten-future reclamation or P2g/OS
cancellation. Request count is logical admission, not initiated sends/retries.

## Evidence and remaining gates

Fifty additive tests cover27 actual-builder/source/client/concurrency/return/drop
cases,12 real input/runtime/child/compaction cases and11 arithmetic/bounded-estimator
cases. The first corrected source run passed its then-existing48 new tests (plus
two unchanged native admission tests); the final two actual-permit seal/late-approval
cases join the full fresh source checks. Real calibration/input/HTTP histories stay
intact; no reset or fake zero baseline supplies refusal evidence.

The first NEW local-driver setup incorrectly selected `--lib` for this binary-only
package and failed before compilation. That failed log/source snapshot is retained;
only the new driver selection was corrected, not production source/old assertions.
The first production check and corrected narrow test run were warning-free; final
source-wide local checks and exact-head full CI are independent gates, not inferred
from these early rows. A later new source-audit literal mismatch and dependent
local-driver attempt with no proof file also failed BEFORE any cargo row; only
new audit/driver setup was corrected, not old checks or production assertions.

Source-local broad gadgets admission recorded the SAME unchanged partial-shell
fixture failing its three existing six-second attempts with Timeout/zero original
bytes/no ready marker. All27 new builder cases passed in that row, but the broad
row remains FAILED. Continue other originally planned rows without a full restart;
one exact isolated diagnostic is separate, not a substitute green broad row.
Fresh exact-head full CI must pass that OLD readiness/11-byte/two-format case in
BOTH builds on all three OSes. Never loosen deadlines/assertions/process semantics
or relabel unready native launches as partial-output evidence.

Every older test/assertion, source
codec, successful SSE/control/usage behavior, native reader/writer/process/guest
limit, framework/SDK/WIT/dependency/lock/pin, frontend/SafeBackend, frozen v4/standard
ACP, schema/vector/golden/trace and historical corpus must remain unchanged.
Documentation accompanies this incomplete slice. Initial failures, if any, remain
failed evidence; no old assertion/deadline loosening, unchanged full retry or
independently blocked provider inspection/execution/workaround is authorized.
All FOUR fresh exact-final-head CI jobs AND every mandatory/setup step, both
feature modes/Windows, raw source/warning/review proof gate any review-only merge.

Next: actual request/send/receipt/partial-output identity and usage-presence
witnesses, bounded output fallback, reported/estimated/unknown billing, real
concurrent settlement and overrun exposure. Only then can a public ledger source
#97's measured usage/remaining/refusal consistently. Public exec/full flags,
results/events/stdio/seeded storage/trace/provenance/gadget callables/catalog,
P2g/live/default-on/release remain separate and unwired/off.


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
