# Recovering interrupted terminal sessions

Terminal sessions require successful checkpoint writes before starting a turn,
before each model round, at the reviewed model response and after each completed
tool result. Turn-end hooks run once; completion is recorded after they return.
A storage failure stops subsequent model/tool work with a fixed diagnostic.
Completed effects remain in memory and on disk; this does not promise rollback.
A missing store refuses agent requests and is reported at terminal startup.

The framework's `durable_turn_checkpoints` option defaults false. The application
selects it only for the explicitly owned terminal session. Legacy frontends and
private prepared runtimes retain their persistence policy. The framework adds no
database, HTTP or terminal dependencies. CLI flags, model definitions and
transport frames are unchanged.

## Reopening a session

Use `craftsman --session SESSION_ID` or `/session` to reopen the latest checkpoint.
Durable tool results, todos and conversation are retained. The terminal warns
when a turn was incomplete or calls lack recorded results. Missing results are
balanced with an error message saying the operation **may have had effects**,
before the next conversation turn. Calls match within each assistant/tool group;
an id reused in an older round is not the current call's result.

Reopening performs no tool dispatch or model request. It never replays pending
operations automatically. Inspect file changes, Git diffs, build outputs and
external effects before asking to retry. The gap between an effect and its save
cannot establish whether that effect occurred. A parked approval requires an
explicit resolution; its unrecorded siblings are not resumed. Approval is
durably consumed before the approved tool starts, so a subsequent interruption
does not resurrect a stale approval. Starting a new turn cannot bypass a parked
approval.

The saved state's reserved `custom._craftsman_durable_turn_v1` marker records
whether the turn was incomplete. Terminal restore removes it from live state.
Malformed/unknown-version markers, live custom-key collisions and sequence
overflow refuse with fixed diagnostics. Old checkpoints without the marker
remain readable; unbalanced calls still receive unknown-completion messages.
The database layout/version remain unchanged.

## Atomic local storage

Manual `/compact` saves the original state before summary sampling and the
replacement before confirming success. Active turns and pending approvals
refuse; cancellation or concurrent message changes discard an uncommitted
summary. Final saves retain ownership through return. Reopening retains actual
results without repeating compaction or tools. See
[terminal compaction](terminal-compaction.md) for save/failure ordering and
isolated actual-builder/HTTP/SQLite/native-exit verification.

Thread metadata, project context, state/suspension and old-checkpoint pruning
share one SQLite transaction. Failed insertion or pruning rolls back the whole
save. A dedicated connection and async owner keep concurrent catalog/workspace
operations outside that transaction. Checkpoint connections request
`synchronous=FULL` under existing WAL mode and the five-second busy timeout.
The existing 50-checkpoint retention bound remains. Sequences outside SQLite's
signed range and negative stored sequences refuse without wraparound.

Submitted input stays consistent in memory and its checkpoint after provider
errors. A save failure retains submitted input and completed results in memory. The TUI
checks the last checkpoint outcome after owned shutdown returns and does not
print “Session saved” after a failed attempt. JSONL
journaling remains advisory and is not a replay log or replacement checkpoint.

## Verification and limits

Additive framework tests cover user/intent/result saves, failures before/after
effects, fixed diagnostics, old-checkpoint ordering, parked approvals,
metadata/sequence refusal and one turn-end invocation. Database tests inject
actual SQL insertion/pruning errors and check rollback, connection isolation,
signed sequences and synchronization settings.

A marked native subprocess uses the actual agent and SQLite store, writes a
physical file, commits its first result, then exits inside the second tool
without destructors or turn-end hooks. A fresh owner reopens the database and
checks the real file, result/todo and unknown second call without replay.
Unmarked helpers return before path/config inspection. Providers and configuration
are isolated and synthetic. The first new pruning test guessed an empty
conversation's title incorrectly; that failed result is retained. Its corrected
assertion compares the actual prior title and preserves the rollback check.

Full local and fresh all-four/every-step/both-mode CI validation remain required
before merge, with independent merged-main verification. These checks do not
prove power-loss behavior, trustworthy disks, native containment or live model
quality. This is not an exactly-once execution or cross-process session lease:
use one agent owner per conversation. Background task state,
in-progress native work and retained crash diagnostics have
separate recovery limits. Unknown usage/billing stays unknown. The inherited
shell issue remains open. Public `craftsman exec`, headless persistence/usage/
producers, SDK/WIT, protocol/schema/goldens and binary/image publication retain
their existing gates.

Stable local verification passed every `scripts/verify.sh` check: core
**1947 passed / 0 failed / 13 ignored**, gadgets **2103 / 0 / 16**, with no
compiler warnings. Every original named/ignored/doctest entry is retained;
20 additive cases and one marked helper appear in both modes. The initial
complete WIP pass is retained separately because approval-consumption review
changed source while it ran. The final source hashes remained stable throughout
verification. Fresh hosted and independent merged-main checks are still required.
