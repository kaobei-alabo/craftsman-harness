# Private admission-correlated send/HTTP-header observations (R5)

**Status: November 2, 2026 (Asia/Taipei): implemented private opt-in foundation; not token usage, billing, reservation settlement or public
`craftsman exec`.** R1–R7 and #97's refusal/v1 compatibility remain accepted.
[token-reservations.md](token-reservations.md) implements the preceding private
combined reservation. Its allocations remain active or retained unsettled;
this slice must not refund calls/allocations or infer usage from response headers.

## Bounded scope and truthful observations

Only the actual token-admitted body/non-clone permit may mint the new observation
scope. It retains the SAME concrete role, logical admission ordinal, meter/gate,
checked-client source and request identity. No caller role/name, response/status,
provider, receipt count, seeds or arbitrary gate can construct it. Existing
request-only/default frontends remain unchanged; absence of this selection is
not a fabricated zero-attempt ledger. Main/child/compaction token providers share
one actual builder/meter, outside middleware exclusion.

One bounded meter mutex correlates at most10000 logical admissions and at most2
send slots each. Whole final request/token admission still precedes charging;
observation storage must be admitted before either counter/allocation increments.
No provider execution or await occurs under this mutex. Non-clone attempt tickets
retain exact source/request/slot identity; duplicate, foreign, out-of-order,
post-end, overflow or incoherent accounting fails fixed/closed, never resets facts.

Preparing a ticket is NOT a send. A client-only witness is minted only AFTER the
supplied request-builder send future is actually first-polled. That observation
is still **not proof of a socket/HTTP wire attempt, provider processing or billing**.
A real response-header witness is issued only after the actual awaited send
returns a response. A failed-send witness is issued only after the actual awaited
send returns an error. Drop before poll stays unpolled; drop after poll records
abandonment without fabricating an error, response, body EOF or zero received bytes.
Real received status survives later stop, body failure, retry, return or Drop.

The fixed observations distinguish unprepared, prepared-never-polled, polled
awaiting a result, actual send error, actual received HTTP status and abandoned
polled future. Retry slots remain within their logical admission, never an extra
request charge. The existing maximum2/250ms retry policy,15s connect/120s read
limits and successful SSE/parser/finalizer/control/usage/completion behavior stay
unchanged. Headers do not establish body completion or usage presence. Error-body
redaction/diagnostics keep their original independent facts and source authority.

Before every new send poll, recheck SAME gate/source/accounting. Once a send has
really been polled, record its genuine returned failure/HTTP status even after a
late deny-only/typed approval stop; do not erase receipts to fake eligibility or
zero activity. Genuine typed approval still dominates Runtime/terminal eligibility.
Sealing does not notify/interrupt/rollback already-started native/HTTP work, reclaim
forgotten futures or prove P2g/all-role/OS quiescence.

## Private snapshots, not a public or usage ledger

Only an actual `Run` selects source-bound immutable observations; no raw constructor,
Debug/Serialize/result/event/trace/billing conversion exists. They describe actual
selected logical admissions/roles and transport-future/header facts, not reported/
estimated-call classification, known/unknown billed attempts, measured tokens,
price, complete delivery, success stream admission, terminal status or all-role
quiescence. Equal fresh snapshots are not authority aliases. Historical facts
are not live/revocable publication; public writers still need actual terminal/
approval authority plus seeded scalar/framing checks.

No successful response/control parsing, usage-presence tracking, partial-output
estimation or settlement is selected. This is NOT the independently blocked
provider-response correction or a workaround for it. Do not inspect/execute/retry
that blocked work, infer `Usage {0,0}` as reported zero, clamp real provider totals
or source #97's measured observed/remaining from private reservations/receipts.

## Evidence and remaining work

Forty additive tests cover10 actual-builder/client/source/replay/concurrency
cases,20 real input/runtime/HTTP/retry/stop/drop/foreground/named/compaction cases
and10 independent coherence/storage/phase/arithmetic cases. The first then-existing
28 NEW cases (plus6 old cases) passed but reported5 new unused test interfaces;
those warnings remain preserved. Completing the new actual-owner app coverage
made the next37 NEW cases/6 old cases warning-free, not by an allow suppression or
production visibility/old assertion change. Final three stronger real-budget-stop/
named-role/coherence cases and post-observation fail-closed guard complete the slice.

The NEW native-header fixture now accumulates original header chunks under the
SAME5s/16KiB bound before detecting the terminator; no assumption about one TCP
read or fake receipt/readiness/EOF is made. This changes only new fixture input
handling; production transport/reader/writer/process/SSE limits remain unchanged.
Final source-wide local and all-platform exact-head CI checks are independent
merge gates, not inferred from initial production/narrow checks. Preserve every
old test/assertion, native/guest/host/input/writer/process limit, request codec,
SSE/control/usage/diagnostic behavior, SDK/WIT/framework/dependencies/lock/Rust pin,
frontends/SafeBackend/v4/standard ACP/schemas/vectors/goldens/trace/corpus and the
historical harness-plan prefix. No warning suppression, old deadline weakening,
unchanged broad test/CI retry, fake receipt/status/bytes or public flag enablement.
All FOUR fresh exact-head CI jobs/every mandatory/setup step/both modes/Windows
and ACTUAL macOS15 labels plus source/warning/tree/review proof gate review-only
admin merge; the retained `test (macos-14)` name is only a legacy required context.

Next: actual bounded successful-response/usage-presence/partial-output witnesses,
reported/estimated/unknown billing, concurrent settlement/overrun exposure, then
public result/events/stdio/seeded storage/trace/provenance and P2g. Public exec,
callable headless gadgets/catalog, live/default-on/release remain separate/off.
Other retired release/manual-benchmark image references remain independent
maintenance work; no release/tag/benchmark is run by this slice.


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


The first NEW offline feature guard guessed `pub(crate) fn enabled` while the
unchanged actual function is `pub fn enabled`; that2-pass/1-fail log/source is
preserved. Only the new guard literal was corrected to the original public
function and existing default-false body. No production visibility, policy,
old test or assertion/deadline was changed to manufacture a pass.
