# Owned-terminal synthetic evaluation v2 — November 18, 2026

The isolated driver now runs the production owned TUI, with real manual
one-call approvals, checkpoint reads, cancellation, `/compact`, quit and
same-session reopen. Its result is a **scoped external-observation result**;
it is not a live coding score or verification of every private runtime fact.
The original preparation commit `0ad1e1a8fc8ba908ada569a9d848b4fd73f8be86`,
original v5 corpus, historical grades and failed review evidence remain intact.

The [v2 scenarios](../evals/coding/owned-terminal-v2/scenarios.json) separately
version the corrected capture/scoring boundary and native-child wrapper.
The [original v1 scenarios](../evals/coding/latest-runtime-v1/long-session.json)
are unchanged. v2 preserves the original task and held-out grader assertions;
only the newly proposed scenario/capture criteria and fixture change.

## Exact source and isolation

Runtime source is `33a80d99431fd707b148485cb714f365b1946803`, tree
`ddee5694698835c48c9c7bbf37b4140b64d721dc`. It was built with actual
`cargo build --locked --release --no-default-features -p craftsman-rs --bin craftsman`
from a tracked Git archive mounted read-only. The unique artifact target ends
`cargo-targets/evaluation`. The working tree is never the compiler input;
untracked automatic `build.rs`/`.cargo` inputs cannot gain build attestation.

The immutable local image is
`sha256:6123321bf37c5641bc6ff4b4f79848f8b8e752fbc78a2f4361406162943cb38b`,
with binary SHA-256
`08542be11e7d207b43c4e0622a6f24180eaa2de9d65b247599f7f70660b56585`.
The independently frozen original v5 corpus SHA-256 is
`5d45038dca136245f090d2e2ed83a3e99f7a7c18eff19f2c4ba2d750d71d89f7`.
Caller-selected runtime records cannot retarget this original-corpus anchor.

The agent mounts only its disposable task and one read-only client bundle.
The bundle contains capture functions, not graders, references or signing keys.
Provider/gateway processes, driver roots and signing authority stay outside
that mount. Actual Docker inspection verifies image, two mounts, read-only
root, dropped capabilities, no-new-privileges, 128 processes, 768 MiB memory,
two CPUs and an internal network. No host config, key or Docker socket is
mounted into the agent.

The driver accepts only the fixed synthetic provider route and fake keys.
The real ledger is read before/after and is never mounted or written. A separate
zero-price fixture ledger uses the unchanged **$25 cap and 20-request gateway
limit**. The v1 long-run proposal's 40-request upper limit is not enabled.
Actual emitted configuration/settings and received synthetic request model,
effort and output ceiling are checked: Sol 6.1 Responses, `xhigh`, 8,192 tokens,
12,288-byte compiled context, repeat limit five, review signals off, manual
permissions, `auto_approve=false`, voice/gadgets/MCP/LSP disabled.

## Observed and unverified channels

| Channel | What v2 records |
| --- | --- |
| Input | Actual per-submission saved context pin/stamp, expected submitted digest, observed appended position, monotonic event and first observed stored-row sequence; captured before lawful eviction |
| Stored conversation | Typed messages/groups, exact ordered message digests, group-local ids and actual error booleans; first-seen owner generation/checkpoint/index/call ordinal is historical stored evidence |
| TUI lifecycle | Fixed actual OSC 7501 states, own PTY/keeper wait returns and terminal-mode restoration; no rendered prompt/body export |
| Storage | Retained scoped SQLite directory/file identity and consistent row queries; raw stored rows are kept distinct from unobserved live reconciliation |
| Gateway | Actual validated arrival attempts, persisted admissions, actual refusals, received headers, genuine read EOF and settled/unknown usage; no fabricated historical 21st attempt |
| Files | Independent finite no-follow reads through a directory retained before execution; source, effects, current probe input and final stopped-task snapshot are bound to the receipt |
| Native probe child | One immutable marked child executed from a retained source descriptor; actual bounded captured streams and status after real wait, then forwarded once |

Direct `AgentBoundaryPolicy` callback objects, typed `TurnResult`/`AgentError`
and `checkpoint_failed` at return, live `RecoveryInfo`, outer `ToolMeta`/
native-truncation facts, complete activity/quiescence and absence of arbitrary
shell rereads are **unobserved/unverified**. Reports enumerate these criteria
and set `all_stronger_private_criteria_verified=false`. They never predict
UNKNOWN and call it observed live reconciliation. A root error or unfinished
row is not converted into positive saved success.

The stored context permits 32 KiB, first input plus four recent update pins,
4 KiB whole pinned text and lawful omission/eviction. Ten 8 KiB updates can be
accepted without retaining their plaintext or all ten pins. v2 retains separate
per-input witnesses as those saved pins occur; event numbers include runtime
activity and are not dense human-turn numbers.

Manual `/compact` keeps up to **four** recent messages, expanding the cut to
preserve a tool group. It differs from automatic message-count compaction's
120-message trigger/20-message tail. v2 verifies the actual recent tail and
replacement row/context count; it permits summarized old prefixes and does
not demand strict message-count reduction. Earlier group witnesses remain
historical after compaction. Noncompaction scenarios demonstrate a selected
no-compaction interval before checking exact prefix/group positions.

## Native wrapper and receipt authority

[native_probe_wrapper.py](../scripts/coding-eval/native_probe_wrapper.py)
rejects absent, unknown and duplicate markers before path/config/import effects.
Its marked mode opens and hashes the immutable v1 probe through a retained
descriptor, reserves a create-only witness slot, starts exactly one child,
collects bounded actual streams/status and writes complete metadata only after
the wait returns. It forwards the real bytes once and preserves exit 7. An
incomplete reserved witness is not success and a second invocation cannot
rerun the child. This is child-process evidence, not outer ShellRunner metadata.

The wrapper source/call/witness and actual physical input before/after the
operator change are bound to the external root. Its original observation is
labelled historical after that change. A direct observed `read_file` of the
changed probe or an explicit duplicate marked invocation rejects the positive
criterion. This does not prove the absence of arbitrary shell/hidden activity;
that broader no-reread instruction remains unverified.

The driver retains actual process/container/workspace authority. A seal can
only be issued after matching real producer history, capture stages, input
witnesses, waits, actual container removal and retained stopped file data.
HMAC covers source/image/scenario/settings/client/code bindings and observation
digests. The key remains outside the agent. The original grader is invoked
only after this driver-held authority verifies those bindings. Caller JSON
alone is not a runtime receipt; standalone data grading requires explicit
`--synthetic-metadata-selfcheck` and reports no runtime result.

The operator and Docker daemon remain trusted. This mechanism does not protect
against an operator who rewrites the driver or reads its private signing key,
and does not establish OS containment, remote billing termination or a session
execution lease. Overflow/incomplete capture refuses positive grading; required
evidence is not clipped to manufacture a pass.

## Repeating the checks

The trusted build record is produced from the actual archive build, with
source/tree/image/binary/archive identities; it cannot be an arbitrary runtime
substitution. From this worktree:

```sh
python3 -I -B scripts/coding-eval/run_owned_tui.py \
  --image-build-record /path/to/owned/image-build-record.json \
  --output /path/to/fresh-v2-output
python3 -I -B scripts/coding-eval/run_owned_tui.py \
  --image-build-record /path/to/owned/image-build-record.json \
  --scenario interruption-reopen-v1 --negative-case save-refusal \
  --output /path/to/fresh-negative-output
python3 -I -B scripts/coding-eval/test_preflight_latest.py
python3 -I -B scripts/coding-eval/test_long_session.py
python3 -I -B scripts/coding-eval/test_owned_tui.py
```

Other negative cases are `provider-eof`, `duplicate-create` and
`forbidden-reread`. A held `BEGIN IMMEDIATE` writer transaction exercises the
existing five-second busy limit without schema/table changes; actual root error
and unchanged prior row stay failure observations. EOF records actual no-terminal
stream return and unknown usage; an in-progress row remains in-progress. Explicit
duplicate controls are fresh requested actions, never automatic recovery replay.

Local/CI wiring adds these three test entrypoints while retaining every original
14 local check, all four contexts, mandatory setup/default builds/both native
modes, conditions and publication hold. The eleven frozen data-only false-accept
counterexamples are rejection regressions. Earlier failed driver/bootstrap/
fixture/unit attempts and the immutable preparation/review evidence remain
preserved separately from the corrected runs.

No paid evaluation, cap-extension migration, real ledger write, global install,
PR or publication is performed. The original $24.285934/$25 ledger and 40
unknown reservations remain exact; the additional $25/$50-total request is
still unanswered. Coordinator full local/native/hosted/inventory/warning/tree/
independent-main integration gates and any stronger private assertion work
remain separate from these synthetic external observations.
