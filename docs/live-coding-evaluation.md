# First coding-quality cycle — November 14, 2026

The first cycle used the actual Craftsman ACP server in isolated Linux
containers with `gpt-6.1-sol`, the user's configured Responses endpoint,
`max` reasoning effort and an explicit 8,192-token output ceiling. The user
confirmed $2/M input, $10/M output and no request fee, and approved a $25 total
allowance. All paid stages shared one admission ledger.

The 30-task baseline covers bugs, features, refactors, failing tests and changes
across multiple files in python, JavaScript and Rust. Every original fixture
failed its grader; every reference fix passed. Graders were introduced only
after the agent stopped, and the agent mounted only task files and its client
script. Native fixture checks confirmed that grader files and the gateway's
private credential were absent from the agent environment/configuration.

## Baseline and corrections

The first isolated 30-task pass completed 15 turns successfully, while 19 final
workspaces passed the original graders. Case review found underspecified return
contracts in JSON-lines finalization, dry-run results and nonfinite cache-key
inputs. Those contracts were made explicit before rerunning the affected cases.
The timeout grader also assumed an internal loader return shape that the task
had not requested; grading was corrected to check the documented public output.

After those documented corrections and reruns, the baseline records **17/30
successful turns and 21/30 workspaces passing behavior checks**. Twelve cases
ended with transport, execution or wall-limit failures, including some cases
whose final source passed grading. These failures remain failures; a correct
edit alone is not a successfully completed agent turn.

Earlier preliminary attempts are retained separately. An evaluation logger
initially mishandled a standard nullable response field, and an overly broad
driver mount initially exposed fixture files. The logger received dedicated
regressions and the mount was narrowed before the isolated baseline. Recorded
preliminary traces contained no driver-directory reads, but those attempts are
not used as the primary coding-quality cohort. Unknown-usage reservations from
interrupted attempts remain charged to the same allowance.

## Paired memory-guidance comparison

The candidate changes only the explicitly selected TUI/ACP memory guidance.
It uses the already loaded turn-start snapshot, gives current instructions
priority, and avoids treating task instructions as lasting preferences. The
framework default, legacy/private/frozen callers, permissions, storage layout,
native limits, model settings and tool definitions retain their prior behavior.

Six selected tasks used identical model/effort/endpoint settings and graders.
The corrected JSON-lines contract was used for both sides.

| Metric | Baseline | Candidate |
| --- | ---: | ---: |
| Successful turns | 4/6 | 6/6 |
| Provider requests | 57 | 53 |
| Memory tool calls | 8 | 3 |
| Total elapsed seconds | 692.631 | 786.949 |
| Accounted cost at confirmed rates | $1.258718 | $0.996742 |

The sample shows 62.5% fewer memory calls, 7.0% fewer provider requests and
20.8% lower accounted cost. Total elapsed time increased because the candidate
completed two cases whose baseline attempts failed early. It does **not**
establish a speed improvement or prove that the guidance caused every success
difference. The change addresses observed unnecessary memory work; endpoint
stream interruptions remain an independent limitation.

## Product comparison and repeat variation

The pinned Codex CLI ran the same six selected tasks through the same gateway,
model, effort and output ceiling. Its final source passed **6/6** graders;
**5/6** turns completed normally. It used 41 provider requests, 723.361 elapsed
seconds and $1.653832 of accounted usage. Host auth, plugins, hooks and MCP
connections were omitted; the disposable container supplied its execution
boundary. This is a controlled comparison, not the user's entire normal setup.

Two repeat baseline runs passed again. Pagination varied from 66.529 to 78.237
seconds; the expiring-cache case varied from 130.705 to 178.025 seconds. This
variation and the small sample limit causal and latency claims. More models,
larger repositories, longer sessions and repeated paired runs remain future
evaluation work. No broad competitiveness claim follows from these tasks.

## Accounting and retained evidence

The cycle accounts for **$20.678352** of the approved $25 allowance, including
40 retained unknown-usage reservations. That is conservative admission
accounting at confirmed full input/output rates, not a provider invoice;
unknown usage is not zero and cache discounts were not assumed. No paid Claude
calls, gadget-default changes, binary uploads or image publication
were introduced by this cycle.

Sanitized task traces, original/corrected grades, source snapshots, native
fixture proofs and the complete ledger are retained locally under
`target/verification/coding-quality/`. The baseline core image was built from
`234cc86`; the candidate's Rust sources were built from `27a9799` and remain
unchanged by later corpus/fixture/documentation corrections. The publication
hold and public headless execution/gadget gates remain separate.
