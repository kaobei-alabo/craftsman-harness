# Fresh coding evaluation preparation — November 17, 2026

This document preserves the original offline preparation and its limits.
[Owned-terminal evaluation v2](owned-terminal-evaluation.md) supersedes its
unverified capture assumptions, including the manual-compaction tail, and
documents the actual synthetic driver, scoped native-child witness, reviewed
false-accept repairs and remaining unverified private criteria. The original
preparation commit and v1 scenario files remain immutable evidence.

**The present $25 allowance cannot fund a full fresh run.** The original ledger
accounts for $24.285934, leaving $0.714066. Its 915 rows retain all 855 original
rows and the same 40 unknown-usage reservations charged $6.586682. Preparation
makes no model calls, resolves no credentials, changes no ledger contents or
binding, and creates no live allowance.

The proposal is **$25 additional, $50 total on the same original ledger** for
one fresh 30-task run, six selected-case repeats and three bounded terminal
scenarios. This is a proposal for explicit owner approval. The current budget
implementation rejects a cap above $25 and changed ledger bindings; the current
continuation runner rejects live full-suite forecasts. An approved admission
extension is separate work. Changing a price file alone cannot enable this plan.

## Exact proposed source and settings

The dispatched source is verified starting revision
`33a80d99431fd707b148485cb714f365b1946803`, tree
`ddee5694698835c48c9c7bbf37b4140b64d721dc`. The preflight checks that Git tree and
actual working files outside the permitted evaluation/documentation scope.
If integration changes runtime source, create and review a new plan version
before building or running it. Record the preparation commit separately from
the source being evaluated.

| Record | SHA-256 identity |
| --- | --- |
| [Plan](../evals/coding/latest-runtime-v1/plan.json) | `073e4991e8bacde1448cb8c964cdfdfd2083ddb5b634bfd55c15386bb383b477` |
| [Original 30-task corpus](../evals/coding/tasks.json), v5 | `5d45038dca136245f090d2e2ed83a3e99f7a7c18eff19f2c4ba2d750d71d89f7` |
| [Feature options](../evals/coding/latest-runtime-v1/feature-options.json) | `0f065bea4a3c995f7cc0bab6314cb0a2e1073c5a510d8a385a4b0d1d839e46a8` |
| [Long-session corpus](../evals/coding/latest-runtime-v1/long-session.json), v1 | `1ccacdb5c38accd8fe02b34f463cec2690927a37eacf5fc05c74289043e58be7` |
| Credential-free effective model settings | `2689e991578f60a236cada705d84bef5590d537f873979590032821ba0b17019` |
| Tracked runtime inventory outside preparation scope | `8923bed731142a37f5d9842f920e45793d91cb2e1b1fc3aa04e3dd9950610b1a` |
| Current original ledger bytes | `53e1884f68bc6ad7faacb40818f16cbec3c01bea86744e0008e3c8c5ff376676` |
| Original 855 rows, canonical JSON | `529e488b3e87cc63ea621a1f77b263c0642eecbcc79958b877b1f56434e8dd02` |

Use exactly `openai/gpt-6.1-sol` / API ID `gpt-6.1-sol`, protocol
`openai_responses`, effort `xhigh`, and 8,192 output tokens. Other generation
options are absent. Explicit experiment options enable 12,288-byte context and
set repeated-failure limit five; structural test-review signals stay off.
Voice, gadgets, MCP and LSP stay disabled in the disposable configuration.
Permissions remain manual with `auto_approve=false` and individual one-call
approvals inside the isolated container. User configuration and defaults stay
unchanged.

The full suite and each repeat keep the existing limits: 20 admitted model
requests and 300 seconds per case, 256 KiB raw request bodies, 270,000
input-reservation tokens, 8 MiB responses/captured output, 1 MiB events,
768 MiB agent memory, 128 processes and two CPUs. Independent grading retains
its 90-second bound and fresh target. The long-session request/wall limits are
listed below; native production and existing test deadlines remain exact.

There is no evaluation image for this source in the inspected local inventory.
The plan records `image_id: null`; the older `12f16ae` image is historical.
A later local build must use the exact tracked source archive and existing
Docker recipe, record its real immutable `sha256:` image ID/source/tree, and
capture the recipe, base-image identity and actual Rust/python/Node versions.
Optional `--build-record` only checks a supplied record; it does not prove a
build or inspect an image. Binary/container publication remains disabled.

## Fresh cohort and repeated selected cases

`latest-full-v1` selects all 30 tasks in original corpus order. Prompts,
fixtures, public checks, independent held-out graders and reference fixes stay
unchanged. The preflight verifies the corpus against its blob in the recorded
source: refreshing a manifest digest cannot legitimize an altered original
grader. All 30 originals must fail and all 30 references must pass grading in
the new immutable image before a paid run.

Use the existing isolated `run_suite.py` and ACP client for this cohort and
repeats. Each invocation has a fresh workspace and create-only output directory.
Agent processes stop before separate no-network grading; graders, references,
host config and real keys are never mounted into the agent. Public
`craftsman exec` remains absent. Live execution needs a separately reviewed
invocation cohort after budget approval; forecast cohorts remain unable to
start it.

The noise check selects `py-page-boundaries`, `js-json-line-chunks` and
`rs-report-skips`, covering all three languages and the clarified contracts.
Their full-suite results are observation A0. A1 uses `latest-aa1-v1` in
python/JavaScript/Rust order; A2 uses `latest-aa2-v1` in Rust/JavaScript/python
order. The preflight emits six single-task runner invocations because the
existing runner retains corpus order for multi-task selections. Source, image,
corpus, options, model, effort and limits are identical in every observation.

Report all three observations per selected task, completion/behavior frequency,
requests, approvals, questions, cost and elapsed range/median. Include failures
and capped attempts; show elapsed time for all attempts separately from completed
attempts. Three observations do not establish equivalence or a general
improvement. Repeats cannot replace failed entries in the fresh cohort.

The fresh report retains a denominator of 30 and labels unrun cases explicitly.
Actual normal turn return, held-out behavior, original-check retention,
wall/output failure, human-input cancellation and admission refusal are separate
outcomes. A correct edit after a failed turn does not become successful turn
completion. Historical **17/30 successful turns and 21/30 behavior checks**
remain attached to their historical cohort, unchanged.

The coordinator's retained-failure review reconciles 18 actual normal returns:
17 passed behavior and one failed behavior, while four passing workspaces came
from failed turns. Five generic incomplete HTTP streams and four generic stops
at 20 admitted requests lack cause/refusal evidence; three cases hit the actual
300-second wall cap. The completed dry-run CLI task broke its inherited
CONFIG/PATH/TEXT invocation, and a Rust doctest failure still lacks a diagnosis.
These observations establish no production runtime defect. Effort/corpus/API
differences and the original/clarified cohorts remain explicit; no grader or
historical grade is changed to address them.

The preflight reports persisted admissions and unknown usage from the original
ledger, with attempted/refused counts set to unobserved (`null`). Admission is
a saved budget reservation, not proof of a provider send. Twenty admissions do
not prove a 21st attempt, HTTP 402 refusal, 50-round stop or dollar-cap refusal;
a historical task also completed at exactly 20. A prospective bounded private
attempt/refusal observer is proposed, not implemented here. It must record only
actual arrival/admission/refusal/unknown facts with fixed categories and numeric
identities; omit endpoints, credentials and prompt/provider body text; preserve
public schemas/trace/goldens and every ledger row/unknown reservation. Missing
final-client counts, including defaulted zero approvals, remain unobserved until
independently supported by actual frames.

## Bounded owned-terminal scenarios

These cases use the actual owned interactive terminal inside the same task-only
container/gateway boundary. The current ACP single-prompt client does not
select owned durable turns or manual terminal compaction. An isolated PTY driver
with independent capture proof remains to be implemented/reviewed before live
execution; the prepared corpus and grader do not claim that adapter exists.

Each scenario allows 40 total model requests across task turns and compaction,
8,192 output tokens per request, fourteen user turns and a 900-second agent-scenario
wall bound. Ten exact 8 KiB irrelevant user updates add 80 KiB of history.
The original 20-request gateway default is unchanged: 40 requests requires
explicit driver/admission selection after approval. Grade separately after
agent processes stop. Keep existing cleanup, SQLite, permission and terminal
limits unchanged.

With no configured context-window metadata, the recorded source uses the
existing 120-message automatic trigger and keeps 20 tail messages. Ten update
pairs plus the two completed task/effect turns give at least 28 messages before
the explicit compaction. This places the early contract outside the kept tail
and avoids treating a short-conversation save as a summary. These existing
runtime defaults are observed in source and are not modified by the plan.

| Scenario / unchanged base task | Required actual operation | Deterministic independent outcome |
| --- | --- | --- |
| `compaction-v1` / `py-config-merge` | Complete the task, create the early C1 handoff, accept ten updates, invoke real idle `/compact`, wait for saved return, quit/reopen the same session, explicitly follow up | Original grader passes; actual message replacement and one context compaction are captured; files/todos and handoff bytes survive; final JSON is `{"checkpoint":"C1","mode":"careful","merge_policy":"replace_none"}` |
| `interruption-reopen-v1` / `py-create-only-export` | Create I1 once, capture its saved result, accept ten updates, cancel during a real pending model call, wait for cancelled return/final save, quit/reopen, explicitly follow up | Original grader passes; one I1 row and its original digest persist; completed original tool groups/results survive; reopen sends no request; final JSON is `{"ticket":"I1","count":1,"already_created":true}` |
| `retained-results-v1` / `py-page-boundaries` | Run the marked probe once, retain actual stdout/stderr/exit 7, have the operator change the isolated probe input, accept ten updates, quit/reopen, explicitly follow up | Original grader passes; exactly one probe invocation remains; the reported output is the original observation with exit 7 and `freshness:"historical"`, even though input has changed |

The probe writes and flushes bounded binary stdout/stderr and appends one real
invocation line. Unmarked execution returns before path/config inspection.
Its intentional exit 7 is an expected observation, not a failed harness test
or permission to retry. Its source and invocation log are protected. Original
public tests stay byte-exact for these new scenarios; prompts grant no permission
to weaken tests or repair the probe.

The interruption scenario uses a second marked, protected helper that calls
the original `export_rows` API once. Its actual `created I1` result and exact
command are retained; a second invocation must refuse the existing destination.
Unmarked execution returns before importing the task implementation.

The operator reads actual existing checkpoints in memory at before, after,
idle reopen and final stages. `safe_checkpoint_observation` exports only
bounded counts, sequence/session digests, accepted-input/result-group digests,
file/todo digests and the actual compaction count; checkpoint messages, command
arguments, endpoints, credentials and prompt/provider body text are not exported.
Capture gateway request counts and effect digests separately. Observe idle
reopen for at least five seconds without
new input; only the explicit final followup may sample. Cancellation remains
working until real return, and save failures remain failures. This scenario
does not inject a crash or establish power-loss/exactly-once behavior.

`grade_long_session.py` independently checks assistant/tool groups, including
reused IDs, retained result content, accepted original updates, session/sequence
continuity, real effect files and final JSON. It then applies the unchanged
original held-out grader using the existing isolated grader container.
Synthetic capture unit tests validate the grader; they are not runtime,
durable-recovery or paid quality evidence. The grader cannot authenticate
operator metadata itself: sealed actual PTY/checkpoint/gateway observations and verified
source/image binding are required to report a live result.

## Priced proposal and capacity

Use the confirmed prior endpoint rates only for offline arithmetic: $2/million
input tokens, $10/million output tokens and zero request fee. Assume no cache
discount. The ledger's isolated 30-case cohort used 376 requests and charged
$9.252698 including seven unknown reservations; three contract reruns added
$0.925522. Those are historical observations at `max` effort on earlier
source/corpora. Latest-runtime `xhigh` cost and long-session cost remain
unmeasured.

| Proposed coverage | Planning allocation USD |
| --- | ---: |
| One fresh 30-task attempt | 18.50 |
| Six selected repeats | 3.00 |
| Three bounded long-session attempts | 3.00 |
| Planned total | 24.50 |
| Buffer within projected $25.714066 capacity | 1.214066 |

The full allocation is about twice the observed isolated cohort cost. The
repeat allocation allows $1 per pair of selected attempts and the long-session
allocation $1 per scenario; the latter is an unverified planning target.
Prioritize full-suite completion, then repeats, then long sessions. These
allocations are planning guidance, not new ledger allowances or completion
guarantees. Stop when admission capacity or a reviewed stage allocation is
unavailable; retain all charged/unknown rows and label remaining cases unrun.
Fresh run IDs cannot be used to disguise cap refusals.

One maximum request reserves $0.621920; a maximum 20-request case reserves
$12.438400 and a 40-request scenario $24.876800. All permitted requests with
no settlement would reserve $373.152000 for the full suite, $74.630400 for
repeats and $74.630400 for long sessions: **$522.412800** total. Thus the
proposed $50 cap funds the empirical plan conditionally and cannot guarantee
completion at every declared ceiling. Provider failures/missing usage retain
full new reservations; the original 40 unknowns are never refunded or replaced.

## Offline checks and remaining gates

From this worktree, with a fresh report destination:

```sh
python3 -I -B scripts/coding-eval/preflight_latest.py \
  --plan evals/coding/latest-runtime-v1/plan.json \
  --output /path/to/create-only/latest-preflight.json
python3 -I -B scripts/coding-eval/test_preflight_latest.py
python3 -I -B scripts/coding-eval/test_long_session.py
python3 -I -B scripts/coding-eval/test_budget.py
python3 -I -B scripts/coding-eval/test_scope.py
python3 -I -B scripts/coding-eval/test_runner_options.py
python3 -I -B scripts/coding-eval/test_preflight_continuation.py
```

Local preparation passed 15 new preflight and 16 long-session checks plus the
original 22 budget / 4 scope / 11 runner-option / 31 continuation checks.
The existing historical-image full-suite forecast also passed read-only
preflight. The original ledger digest and corpus bytes remained unchanged.
No app build, latest-source image, full native/hosted run or fresh live grade
was produced by this worker.

The independent grading CLI also ran all three original/reference pairs in the
historical immutable `12f16ae` image: references passed, originals failed as
required. Their capture bundles are explicitly synthetic grader fixtures, not
observations of terminal compaction, cancellation or reopen. Both marked
helpers were separately executed without model calls in isolated temporary
directories. The first lineage check's new-document capitalization failure was
retained and corrected without changing the allow-list.

Keep first-cycle preliminary failures/grader corrections, the original v4 Rust
question/cancellation, the separate v5 pair and the failed Windows mixed-source
metadata preflight with its skipped core step. Preparation or synthetic grader
success never overwrites them. See the [first cycle](live-coding-evaluation.md),
[context pilot](live-context-evaluation.md) and [runner](coding-evaluation.md).

Before live work: obtain explicit owner approval for the additional cap on the
same ledger; review admission/cohort selection without weakening original
budget tests; bind the final source/image and validate all originals/references;
prove the isolated owned-terminal driver and captures. The coordinator retains
full local/native/all-four/every-step/both-mode/inventory/warning/tree/independent-main
integration gates. No global install, PR or artifact publication is authorized
by this preparation.
