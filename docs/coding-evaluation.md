# Coding-quality evaluations

See [improvement progress](harness-improvements.md) for completed work and the
remaining context and broader quality milestones.

The starter corpus contains 30 small repository tasks: six bugs, six features,
six refactors, six failing-test repairs and six changes across multiple files.
Each group has two python, two JavaScript and two Rust tasks. Public checks
ship with the task; held-out graders and reference fixes remain with the
operator. The original sources must fail and the reference fixes must pass
the graders before a live evaluation starts.

These are small, synthetic repositories. They measure the selected harness,
model and settings on this corpus; they do not establish performance on large
repositories or equivalence to another coding product. Starter gadget tests
and native runtime tests remain separate evidence.

The [first live cycle](live-coding-evaluation.md) records the baseline, grader
corrections, six-case memory-guidance comparison, pinned Codex comparison,
repeat variation and conservative accounting under the approved $25 cap.

The merged [task-workflow guidance](task-workflow.md) for TUI/ACP main agents
was compared against the merged first-cycle runtime; paired runs retain
the same model, effort, corpus, graders, limits and admission ledger. The full
verification/stuck/context roadmap remains separate from this advisory slice.
The [workflow comparison](live-workflow-evaluation.md) records selected paired
runs and repeats, original/clarified contracts and the same capped accounting.
The [declared-verifier check](live-verification-evaluation.md) then compares one
matched task with identical policy files and records an actual verification result;
the same original ledger remains authoritative.

The [context/failure comparison](live-context-evaluation.md) preserves a
candidate question/cancellation on the original Rust task. Corpus version 5
makes the already intended public `skipped` field addition explicit in that
task's prompt and README. Code fixtures, public checks, held-out graders and
reference fixes remain unchanged. Both arms use the clarified input for its
separate comparison; earlier scores remain attached to their original corpus.

Explicit [context](context.md) and [failure controls](failure-controls.md) options
can be compared in a selected paired continuation. A live continuation requires
an immutable image for each recorded source/tree, matching task/corpus/options
and fixed model settings, and a read-only checkpoint of the original ledger.
The preflight rejects changes to original rows, unknown reservations,
rates or the total allowance. The gateway checks the current ledger digest again
before starting admission. Full-suite forecasts are preparation only; the
selected pilot does not authorize running them.

## Runner boundary

`scripts/coding-eval/run_suite.py` runs the existing `craftsman acp-serve` in a
fresh Linux container. It does not implement or expose `craftsman exec`.
The operator uses a local macOS/Linux Docker host. Each agent container mounts
only its task directory and a single client script. The driver does not mount
the corpus, graders, reference fixes, host configuration or real keys. Root
filesystem access is read-only, privileges are dropped, and memory/process/
wall/output limits are explicit. The network is an internal Docker network;
the agent's only model route is a separate trusted gateway.

The gateway receives the real configured endpoint/key on stdin. The agent gets
a temporary run-specific key usable only for the selected model and route.
The gateway rejects hosted tools, server-side conversation references, extra
models, redirects, multimedia inputs and oversized requests. It supplies the
experiment's explicit output ceiling when a client omits one. Successful
provider SSE frames are streamed to the client; error bodies are kept private.

Agent processes stop before grading. The operator captures source through
retained no-follow directory handles, rejects links/special files and bounds
the snapshot. Grading uses a separate container with no network and a fresh
build target. File edits, existing checks, actual stop reason, human questions,
approval count, elapsed time and grader result are recorded separately.
One-call approvals apply only inside the disposable agent container; user
configuration retains its own permission settings.

Container isolation is supplied by the operator's Docker daemon. This does not
complete the app's private filesystem, public result/usage, signal shutdown,
callable headless gadget or release gates. Binary/container publication remains
disabled; the Docker recipe is for a local evaluation build.

## Spending

Fill [prices.example.json](../evals/coding/prices.example.json) with the endpoint's
actual USD prices per million input/output tokens and any per-request fee, then
confirm them explicitly. Unknown pricing refuses live calls. The first cycle
is bounded to the approved $25 allowance.

Every stage uses the **same ledger path**. One gateway owns that ledger at a
time. Before a send, it reserves a conservative text-input byte allowance plus
framing headroom and the complete output ceiling. Only valid provider-reported
usage can release unused reservation. Failed, interrupted or unknown-usage
requests retain their reservation; missing usage is not zero. Reopening keeps
those costs and never replays requests. Cache discounts are not assumed.

These calculations depend on confirmed rates and provider compliance with the
declared text/output limits. They are not a provider invoice or a promise to
stop remote billing when a connection closes. A provider/accounting mismatch
closes further admission and must remain visible in the report.

## Commands

The Docker build context must contain tracked source only. For example:

```sh
mkdir /tmp/craftsman-eval-source
git archive HEAD | tar -x -C /tmp/craftsman-eval-source
docker build -f /tmp/craftsman-eval-source/evals/coding/Dockerfile \
  -t craftsman-coding-eval:local /tmp/craftsman-eval-source
python3 scripts/coding-eval/check_corpus.py \
  --image craftsman-coding-eval:local --corpus evals/coding/tasks.json \
  --output /tmp/corpus-validation.json
python3 scripts/coding-eval/run_suite.py \
  --image craftsman-coding-eval:local --corpus evals/coding/tasks.json \
  --config ~/.craftsman/config.json --prices /path/to/confirmed-prices.json \
  --ledger /path/to/evaluation-cycle/ledger.json \
  --output /path/to/new-baseline-directory --label baseline
```

Output directories are create-only. Keep the same ledger for comparisons,
noise runs and candidate reruns. `--tasks` selects comma-separated task IDs;
`--arm codex` selects the pinned Codex CLI with the same model, effort,
endpoint and experiment ceiling. That comparison uses the external container
as its sandbox and copies no host auth, plugins, hooks or MCP connections.
It is a controlled same-model comparison, with those settings recorded.

For a selected continuation, supply `--effort xhigh`, `--feature-options`,
`--continuation-manifest`, `--build-records`, `--ledger-checkpoint` and
`--continuation-arm control|candidate`, using the arm's recorded `sha256:` image
ID. `preflight_continuation.py` independently checks those records and the
original ledger without loading credentials or making a model request. Unknown
task IDs, duplicate IDs and an explicitly empty selection refuse. Feature files
contain only reviewed context/failure settings; endpoint credentials stay in
the original operator config.

The initial live route is explicitly limited to `gpt-6.1-sol` over Responses.
Other model/protocol comparisons require their own reviewed rate/transport
configuration; the script never guesses credentials or silently substitutes a
model. All paid stages are operator-invoked. CI checks the offline admission,
stream parsing and scope contracts without using model credentials.

The evaluation design follows [official evaluation guidance](https://developers.openai.com/api/docs/guides/evaluation-best-practices):
define task-specific criteria, use reproducible graders, inspect failure traces,
and compare a candidate on the same cases. Infrastructure failures, transport
failures, budget stops and coding failures must remain distinct. Preserve failed
attempts and grader corrections instead of rewriting them as successful runs.
