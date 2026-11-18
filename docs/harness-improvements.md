# Coding-harness improvement progress

Updated November 18, 2026. The initial implementation/evaluation cycle and the
subsequent terminal fixes are merged and installed. The broader goal of
competitive coding-agent quality remains ongoing. Small synthetic repositories
and selected paired cases do not establish performance across large projects
or additional models.

| Work | Current status | Evidence / next step |
| --- | --- | --- |
| Terminal usability | Merged and locally installed | Gray/turquoise 匠 splash, miniature hammer/label, grouped model → effort selection |
| Local services and configuration | Merged | Explicit models/MCP/search, local sessions/skills/data; platform services retired |
| Cancellation and saved work | Merged | Owned lifecycle, durable turns/manual compaction and conversation-preserving model changes |
| Folder-aware checkpoints and long turns | Merged and locally installed | Mutation-driven bounded snapshots, chat notices, configurable 200-round default and saved pauses |
| Terminal status monitoring | Merged and locally installed | OSC 7501 default-on, actual approval/question/edit/save proof in Orca; native host chat registration remains separate |
| First coding-quality cycle | Complete | Original 30-task baseline: 17/30 completed successful turns, 21/30 behavior passes; selected memory/Codex comparisons |
| Task workflow and error hints | Merged and locally installed | Advisory guidance; native proof and five matched instances preserve correctness with lower observed work/cost |
| Declared verification | Merged and locally installed | Reviewed scoped checks, actual results, bounded repair and content-bound protected review; one matched live composition check |
| Bounded task context | Implemented, explicit opt-in | Actual input/file observations, stale/uncertain labels and local `/context`; original messages/native items preserved |
| Exact failure budgets and structural test review | Implemented, explicit opt-in | Actual owned sampling pause after saved results; protected-source notes preserve legitimate progress/approvals |
| Fresh and broader quality evaluation | Local synthetic v3 evidence; live run pending | Frozen-source corpus prerequisite, three production-TUI synthetic scenarios, private SQLite snapshots and independently requested fixed child; combined-source/native/hosted/live-budget gates remain |

The original $25 evaluation ledger accounts for **$24.285934**, including
**40 retained unknown-usage reservations**. Its stages remain separately recorded:
[first cycle](live-coding-evaluation.md) $20.678352,
[workflow comparison](live-workflow-evaluation.md) $23.000498 cumulative,
[verifier comparison](live-verification-evaluation.md) $23.153274 cumulative,
[context/failure comparison](live-context-evaluation.md) $24.285934 cumulative.
No unknown reservation was reset/refunded, and failed attempts/contract
corrections retain their original evidence. These are conservative admission
amounts, not a provider invoice or an additional allowance.

[Fresh evaluation preparation](coding-evaluation-preparation.md) proposes $25
additional / $50 total on this same ledger for the next bounded cycle. It makes
no model calls or cap change. The remaining $0.714066 cannot fund a full fresh
run; budget approval, reviewed future combined-source/image rebinding and
explicit live admission/cohort selection remain required. The separate scoped
synthetic capture evidence is described below.

[Owned-terminal v3](owned-terminal-evaluation.md) supplies local synthetic
evidence from the production TUI at frozen source `33a80d9`: real one-call
approvals, stored rows, cancellation, manual compaction, quit/reopen and an
independently requested fixed native child. Retained whole SQLite bytes are
queried only in a private copy; duplicate/read negatives bind the actual owned
return and new saved condition, and unbound live writer hold is quarantined.
Historical v2 capture gaps and first repair failures remain. It distinguishes
passed original behavior and completed external observations from unverified
private callbacks, typed returns/live recovery/outer invocation/ToolMeta and
whole-activity absence. Its
synthetic fixture ledger does not extend the real allowance or create a full
latest-runtime model score. Coordinator integration and full local/native/
hosted/independent-main validation remain separate gates.

The unchanged full v5 corpus prerequisite passed in the same frozen
source `33a80d9` image: 30 originals failed held-out grading and 30 references
passed, with complete Rust grade/doctest output retained. That credential-free
fixture validation is separate from a full live run or any future combined
runtime/source-image rebinding.

`scripts/verify.sh` discovers 17 checks: every original 14 check plus latest
preflight, long-session and owned-TUI tests. The additive discovery preserves
the original required CI contexts, setup/default builds and both native modes;
discovery alone does not establish those checks passed.

The first full 30-task score is historical. Subsequent selected comparisons do
not create a full score for the current runtime. [Task workflow](task-workflow.md)
is advisory; [declared verification](verification.md) implements actual scoped
completion checks and protected-content review. The bounded [context](context.md)
and [failure controls](failure-controls.md) implementation keeps default behavior
unchanged and has its own native/live delivery checks. Full typed context/cache,
semantic test adjudication, broader effectful-loop controls and public receipts
remain work with separate validation.

Public `exec`, headless gadget registration, broader native containment and the
full Session API/event contracts retain separate roadmap gates. Binary/container
publication remains intentionally disabled, pending owner approval and reviewed
release work. The [harness plan](craftsman-harness-plan.md) preserves its historical
baseline and target architecture; [runtime status](runtime-status.md) describes
maintained implementation boundaries and current validation.
