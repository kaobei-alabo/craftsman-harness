# Selected context and failure-control evaluation

Both arms passed the explicit-contract Rust report task and the python create-only
export task. The original Rust candidate run asked about an API compatibility
conflict and was cancelled by the unattended driver; that failed run is retained.
This small comparison gives functional composition evidence with mixed work/cost
results. The historical 17/30 successful-turn and 21/30 behavior baseline remains
unchanged.

## Runtime and fixed settings

- Control: merged main `b6b34a2`; candidate runtime: `12f16ae`.
- Each local image was built from that exact tracked commit and bound to an
  immutable image ID/source tree in the private build records.
- GPT-6.1 Sol over Responses, `xhigh`, 8,192 output tokens, at most 20 model
  requests per run and a 300-second task limit.
- Confirmed rates: $2/million input tokens, $10/million output tokens, no request
  fee. The same original $25 ledger and retained unknown reservations apply.
- Control uses default context/failure settings. Candidate explicitly enables
  12 KiB context and a repeated-failure limit of five; structural review signals
  stay off for the live pilot. Voice, gadgets, MCP and LSP stay off in both arms.
- All 30 originals failed their graders and all 30 reference fixes passed in
  both immutable images, on both the original and clarified corpus. These
  validation runs made no model requests.

The [runner boundary](coding-evaluation.md) isolates task files and uses a separate
key/ledger gateway. Each actual emitted configuration records the same canonical
generation/reasoning settings hash; failed setup and original runs are preserved.

## Original task and API clarification

The Rust task requests stored skipped counts while also saying to preserve the
existing public API. Its original `Report` has two public fields, so a third
stored field changes compatibility with old two-field struct literals. The
candidate asked before making that change; the unattended driver cancelled on
human input and the grader correctly recorded failure. The control had already
passed by using the field addition intended by the reference solution.

Corpus version 5 explicitly authorizes that public `skipped: u64` field in the
task prompt and README, while preserving existing field meanings and constructor
signature/behavior. Only that compatibility allowance changes. Code fixtures,
public checks, held-out graders and reference fixes remain unchanged. Both arms
then ran a separate clarified pair; the original failed case remains a failed
case. The python task is unchanged between versions.

## Actual results

| Task / contract | Arm | Outcome | Requests | Seconds | One-call approvals | Charge USD |
| --- | --- | --- | ---: | ---: | ---: | ---: |
| Rust report / v4 | control | Pass | 11 | 89.697 | 9 | 0.155562 |
| Rust report / v4 | candidate | Question → cancelled; fail | 5 | 53.387 | 1 | 0.062538 |
| Rust report / explicit v5 | control | Pass | 9 | 57.415 | 9 | 0.123188 |
| Rust report / explicit v5 | candidate | Pass | 7 | 54.753 | 5 | 0.098652 |
| python create-only export / v5 | control | Pass | 14 | 188.370 | 7 | 0.323096 |
| python create-only export / v5 | candidate | Pass | 14 | 210.254 | 10 | 0.369624 |

Every passing run ended normally, passed its held-out grader and retained the
existing checks with no unrelated edits. Rust used fewer requests/approvals and
less cost in the candidate. python used the same number of requests, more
approvals, more time and more cost. One attempt per arm and task does not
establish a general improvement, equivalence to another coding product, or a
fresh full-suite score.

## Accounting and independent gates

All 60 new requests settled to $1.132660. The original ledger now accounts for
**$24.285934 of $25**, leaving **$0.714066** nominal admission capacity. Its
original 855 rows and **40 unknown-usage reservations charged $6.586682** are
unchanged. No reset, refund or new allowance was made. These admission amounts
are distinct from a provider invoice. Paid work stopped after this pilot; a full
30-task candidate evaluation needs a new explicitly approved budget.

The candidate runtime also passed all 14 local checks: 2,079 core / 2,237
gadget-mode tests, no warnings, 117 added ordinary cases and two marked ignored
helpers. Original tests, SDK/WIT/pins/SQL/native limits/frames/goldens remain
intact. Native tests cover actual pause/save/cancellation/input, all four
protocol histories, compaction/transfer and protected-test review. The live
passing tasks do not exercise every refusal path; the isolated installed TUI
check and full hosted/independent-main validation remain separate delivery gates.
See [context](context.md), [failure controls](failure-controls.md) and
[improvement progress](harness-improvements.md).
