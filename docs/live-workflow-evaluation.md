# Task-workflow comparison — November 14, 2026

This continuation compares the merged first-cycle runtime (`b7f8994`) with the
task-workflow candidate's Rust sources (`cec77e4`). Later corpus/documentation
changes retain those Rust sources. Both arms use `gpt-6.1-sol`, the configured
Responses endpoint, the user's current `xhigh` effort, an 8,192-token output
ceiling and a 300-second task wall limit. This setting is separate from the
[first cycle](live-coding-evaluation.md), which used `max` effort.

The operator built immutable local images from tracked source, validated that
original fixtures fail and reference fixes pass, and alternated control/candidate
per task. Agent containers mount only their task files and the client script.
Graders run after agent processes stop, in a separate offline container and fresh
source snapshot. The same gateway/admission ledger, confirmed $2/M input and
$10/M output rates, and no request fee apply throughout. No host credentials,
graders or reference fixes are mounted in the agent environment.

## Selected tasks and contract correction

The three selected first-cycle traces showed post-check planning/review work,
long completed-edit turns and repeated unavailable diagnostics:

- JavaScript ledger refactoring (`js-ledger-refactor`).
- python create-only export (`py-create-only-export`).
- Rust report/skipped-row wiring (`rs-report-skips`).

Both initial JavaScript attempts completed normally but failed the held-out
grader: each implemented `summarize` internally while exporting only the prior
`balance`/`credits` wrappers. The task named a new helper but did not state the
public export as directly as its grader required, and its public checks covered
only the old wrappers. Those attempts remain failed and retained.

The clarified task explicitly requests the public `summarize` export and the
credit/debit count semantics. An additive public assertion checks that export;
all existing assertions, the reference implementation and held-out grader remain
unchanged. Both arms then passed the separate clarified pair. The committed
corpus contains this explicit contract; the first cycle's 17/30 and 21/30 records
are historical results and were not rewritten.

python and Rust each ran once more in both arms to observe repeat variation.
The primary comparison below contains the clarified JavaScript pair and two
python/Rust pairs: **five matched task instances per arm across three tasks**.
The original JavaScript failures remain a separate cohort and remain charged.

## Results

| Metric | Control | Workflow candidate |
| --- | ---: | ---: |
| Successful graded turns | 5/5 | 5/5 |
| Behavior-passing workspaces | 5/5 | 5/5 |
| Provider requests | 71 | 49 |
| Tool calls | 109 | 83 |
| Planning (`write_todos`) calls | 16 | 8 |
| Delegation calls | 3 | 1 |
| Failed tool updates | 4 | 0 |
| Total elapsed seconds | 614.390 | 505.524 |
| Accounted cost at confirmed rates | $1.210290 | $0.833132 |
| New unsettled requests in this comparison | 0 | 0 |

The candidate preserves correctness on this selected cohort while using 31.0%
fewer provider requests, 23.9% fewer tool calls and 31.2% lower accounted cost.
These observations support continued workflow-efficiency work. They do not
establish a broad coding-quality gain, statistical non-inferiority or equivalence to
another product.

Latency varies: the first python pair was 164.784/130.843 seconds
(control/candidate), while its repeat was 167.277/200.748. Rust was
118.248/68.763 and 90.664/49.011; the clarified JavaScript pair was
73.417/56.159. The aggregate elapsed reduction is not a universal speed claim.
This small selected sample and one extra repeat do not establish causation.
“Failed tool updates” counts returned-error statuses; it is not a count of all
failed tests or unexpected nonzero shell exits.

## Accounting and retained evidence

All 12 task attempts, including both original JavaScript failures, share the
first cycle's ledger. Conservative accounting is now **$23.000498 of the approved
$25 cap**, including the same **40 retained unknown-usage reservations**. This
continuation accounts for $2.322146. Every new request settled from valid reported
usage; no unknown reservation was reset or refunded, and cache discounts were
not assumed. These numbers are admission accounting, not a provider invoice.

Sanitized traces, original/clarified task inputs, held-out grades, source
snapshots, immutable image/source hashes and per-attempt ledger records remain
under `target/verification/task-workflow/`; the original ledger remains under
`target/verification/coding-quality/live-budget/`. User configuration and keys
were preserved. No paid Claude/comparison-product calls, binary uploads or image
publication were added.

[Task workflow](task-workflow.md) remains advisory. The subsequently merged
[declared verifier](verification.md) implements scoped completion checks and
protected-content review; its [matched live check](live-verification-evaluation.md)
is separate evidence. Broader semantic test-integrity, safe stuck escalation and
context compilation remain work with their own evaluations. Native CI and
independent-main checks remain delivery requirements, separate from live quality.
