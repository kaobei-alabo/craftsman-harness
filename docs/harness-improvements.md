# Coding-harness improvement progress

The first implementation/evaluation cycle is complete. The broader goal of
competitive coding-agent quality remains ongoing. Current measurements come
from small synthetic repositories and selected paired cases; they do not
establish performance across large projects or other models.

| Work | Current status | Evidence / next step |
| --- | --- | --- |
| Terminal usability | Merged and locally installed | Gray/turquoise theme, filled hammer, grouped model → effort selection |
| Local services and configuration | Merged | Explicit model/MCP/search connections; local sessions, skills and data |
| Cancellation and saved work | Merged | Owned terminal lifecycle, durable turns/manual compaction and conversation-preserving model changes |
| Folder-aware checkpoints and long turns | Merged and locally installed | Mutation-driven bounded checkpoints, chat notices, configurable 200-round terminal default and saved limit pauses |
| First coding-quality cycle | Complete | 30-task baseline: 17/30 normal successful turns, 21/30 behavior-passing workspaces; six-case memory and controlled Codex comparisons |
| Task workflow and repeated tool errors | Implemented; validation and delivery gates apply | Advisory guidance; native checks and five matched instances preserve correctness with lower observed work/cost |
| Automatic verification and test integrity | Planned | Actual approved check receipts, bounded correction loop and protected-test change decisions |
| Context compilation | Planned | Pinned task state, file-aware context, stale-output handling and explainable `/context` |
| Broader quality evaluation | Planned | Larger repositories, longer sessions, more models and repeated paired comparisons |

The [first live cycle](live-coding-evaluation.md) used the approved $25 cap with
one conservative ledger. Its $20.678352 accounting and 40 unknown-usage
reservations remain retained. The task-workflow continuation uses the remainder
of that same allowance and records its current `xhigh` setting separately from
the first cycle's `max` setting. The [workflow comparison](live-workflow-evaluation.md)
records $23.000498 total accounting under that cap. Failed attempts and contract corrections keep
their original evidence.

[Task workflow](task-workflow.md) is advisory model guidance. It does not complete
the roadmap's verification gate, test-integrity guard, stuck escalation or
context compiler. Those implementations need their own behavior, permission,
recovery and quality validation. Native CI and live coding results remain
separate checks.

Public headless execution/gadget registration, broader native containment and
binary/container publication retain their independent gates and existing hold.
The detailed [harness plan](craftsman-harness-plan.md) keeps its historical baseline
and target architecture; [runtime status](runtime-status.md) describes current
implementation boundaries.
