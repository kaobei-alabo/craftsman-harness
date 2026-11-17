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

Additional owned-session regressions in
[`session_bridge/reliability_tests.rs`](../crates/craftsman-rs/src/tui/session_bridge/reliability_tests.rs)
exercise a later effect that reuses an earlier call id. Two separate app approvals
permit actual file creation, writing and synchronization; the child exits after
the second effect, before its tool result returns. A fresh owner retains the
first committed result and labels both the second call and its unstarted sibling
UNKNOWN. The physical second file does not turn an unrecorded result into a
completed result. Reopen and an explicit inspection turn repeat neither write.

The strengthened reused-id follow-up first reopens that abrupt checkpoint
through a distinct owned app builder with the production registered read/write
tools. Actual tool-activity and approval witnesses remain zero during load;
the separately requested inspection executes two real file reads with no write
invocation or approval. The original results, UNKNOWN/sibling group, accepted
input, file bytes, modification times and checkpoint sequence remain checked.
A separately requested write to a different fixture file then proves the same
witnesses observe one actual approved write without resetting their counters.
The earlier scripted final text remains only a state/sequence comparison and
is not counted as an actual read or a registered-tool no-dispatch witness.

The input-preservation cases explicitly enable the app context controls.
Another case rejects the final cancellation checkpoint with an actual SQLite
trigger. The completed write stays durable, while two identical accepted steering
inputs remain in the current owner's memory. They are absent from the earlier
checkpoint: reopening cannot recover a save that failed. The UI reports the fixed
checkpoint error, and a separately refused readiness save starts no model request.
After storage is restored, an explicit follow-up saves the retained originals
without repeating the write. These cases preserve the difference between current
memory, committed results and unknown effects.

The first new-case attempts failed at a Status import, overlapping fixture
borrows, an existing transcript-selection reset and a transcript prefix too short
to reduce during compaction. Those failures remain separate from corrected
results. The corrected long-session fixture produces its prior history through
actual completed turns and crosses the existing 50-checkpoint retention window;
no historical transcript or sequence is planted. Original tests and runtime
contracts remain unchanged.

Two additional owned-session cases use synthetic Claude Messages and Gemini
GenerateContent after 26 actual completed turns and the existing 50-checkpoint
retention window. Claude's original thinking/signature and redacted opaque blocks
remain exact through question cancellation, the final durable save and a fresh
owner's request on the same connection. The accepted input and committed write
result remain portable; the cancelled question keeps its actual non-error
no-answer result while the unstarted sibling keeps its cancellation error.

The Gemini case refuses the final cancellation save with an actual SQLite
trigger, distinguishes retained memory from the earlier incomplete checkpoint,
and explicitly saves the retained originals after restoring storage. A fresh
owner then continues on the original connection with the exact signed parts.
API-returned call ids remain exact, while generated framework ids for id-less
native calls are not invented in function responses. Both cases inspect the
physical file and preserve the write without replay; reopened aggregate usage
is not fabricated. Synthetic signatures test preservation, not authenticity or
live provider compatibility. The first run's new fixture incorrectly expected
the declined question to be an error; that failure is retained separately and
the correction checks the actual existing tool outcome in portable and native
history without changing the provider codec.

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
