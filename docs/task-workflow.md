# Task workflow and tool-error recovery

The terminal and ACP main agent receive task-workflow guidance that scales
planning, delegation and verification to the work actually found. Small changes
can proceed through relevant files, edits and checks directly. Independent work
can still use delegated agents. User requests and repository requirements keep
priority, including every mandatory check.

The guidance asks the model to read each check's output and exit status, preserve
existing tests, explain necessary test changes and finish promptly once the
requested work and verification are complete. Additional reads, reviews and
planning updates need a reason: changed files, new evidence or an unresolved
concern. An unavailable tool calls for an available check or a clear explanation
of the limitation. Documents and folders containing several projects are
supported; this policy performs no workspace scan or Git-based classification.

## Actual returned errors

A bounded observer inside the existing approval gate tracks allowed tool calls
that return a model-visible error result. When the same tool returns the same
error on successive observed attempts, the next model round receives a fixed
recovery hint: inspect the error and any partial effects, use new evidence or a
different permitted approach, and keep permission decisions in force.

Successful outcomes and changed error text reset this hint. Successful repeated
reads and expected nonzero exits are not reclassified as errors from harness-only
metadata. Typed approval, capability-denial and cancellation outcomes are
excluded. Denials from the outer approval middleware never reach the observer.
Errors propagated from a tool rather than returned as `ToolOutcome` are unchanged
and clear the observation. Model/transport errors retain their existing handling.

The observer hashes at most 512 bytes of tool name and 64 KiB of returned error.
Larger values clear the observation without altering the original result. It
retains two digests and one flag in memory, resets at turn start/end, and stores
no raw error, arguments, credentials, new checkpoint fields or diagnostics.
Only fixed guidance enters the system additions; original messages, tool results,
usage and native signed/opaque continuation items are retained.

`SessionExtras.task_workflow` defaults false. TUI/ACP explicitly select it for
the main agent. Framework, legacy, private headless, frozen and delegated-agent
defaults remain. The app-only observer adds no framework/SDK/WIT dependency,
tool definition, generation option, ACP method/frame or user configuration field.
Shell ownership, approvals, operation/shutdown limits and publication holds are
unchanged.

This is **model guidance**, the first advisory step toward the roadmap's stuck
governor. It does not execute verification, refuse otherwise allowed tool calls,
retry requests, switch models, create diagnosis agents or guarantee that the
model follows the workflow. The full verification gate, test-integrity guard,
broader progress detection and context compiler remain planned.

## Evidence and evaluation

The first [coding-quality cycle](live-coding-evaluation.md) exposed repeated
planning/reviews after checks and repeated unavailable LSP diagnostics in small
tasks. That observation motivates the change; it does not establish that it
caused the cycle's transport/execution failures or predict a pass-rate gain.

Agent-loop regressions check original outcomes/dispatch, successful iteration,
permission/cancellation exclusion, bounded observation, per-turn reset and genuine
model failures. Marked native fixtures exercise the shared builder, loopback HTTP,
actual file errors/listing, denied writes, local SQLite/reopen and native Responses
continuations. Unmarked or unknown fixture startup returns before config/path
inspection. The [coding evaluation runner](coding-evaluation.md) supplies separate
held-out grading and conservative accounting for a paired live comparison; native
tests are not live coding-quality evidence.

Full local verification, all four hosted jobs/every mandatory step/both native
modes, retained/new inventory and warning/tree review, plus independent-main
validation remain required before delivery. Existing live accounting and failed
attempts stay retained; more model expense cannot exceed the approved cap.
