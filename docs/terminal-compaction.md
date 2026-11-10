# Saving manual terminal compaction

The owned terminal's `/compact` command saves the original committed state
before requesting a summary, then saves the replacement before reporting
`messages (saved)`. Previously, it changed only the in-memory conversation;
exiting before another turn could reopen the older transcript.

The operation uses the existing `durable_turn_checkpoints` opt-in, checkpointer,
thread identity and monotonic sequence. Legacy manual compaction and private or
frozen frontend defaults retain their existing paths. No CLI flag, storage
table, checkpoint marker shape or model request definition changes.

## Ownership and failures

An active turn or unresolved approval refuses compaction. The operation holds
the same activity guard as model turns, so another turn or conversation transfer
cannot begin while its summary or save is pending. The terminal queues new user
input rather than consuming it as steering. No turn-start, tool, review or
turn-end hook runs, and approvals are never bypassed.

Unknown calls left by a failed turn are balanced with the same recovery logic
as a reopened session, retaining real results and labelling unrecorded effects
as unknown. The original state is saved with the unchanged incomplete marker
before sampling. An exit during summarization reopens that state with a warning;
it makes no model request or tool dispatch and never retries the summary.

Cancellation before replacement discards the summary and retains the original
conversation. A concurrent message-list change also refuses replacement. Files,
todos and custom state committed during sampling survive the message-only
replacement and are included in the final save. Only an actual replacement
resets the usage anchor; missing model usage is not fabricated.

Once replacement begins, the final save remains owned through actual return,
including a late cancellation. Successful return marks the checkpoint complete.
A failed save reports the existing fixed diagnostic and checkpoint-failure
status. The summary remains in memory and the prior durable state remains on
disk; the terminal cannot report successful compaction or `Session saved`.
Further sampling requires another successful readiness save. A short
conversation is saved as complete without requesting a summary.

The existing SQLite transaction, WAL/FULL synchronization, five-second busy
bound, 50-checkpoint retention and terminal cleanup bounds remain. There is no
power-loss, exactly-once, cross-process lease, OS-containment or billing-stop
claim. Use one agent owner per conversation. Background recovery and retained
crash diagnostics remain separate work.

## Verification

Additive framework tests cover save ordering, readiness/final failures, no-replay
restore, exclusive activity/steering/transfer, concurrent state changes,
cancellation followed by a usable turn, parked approvals, legacy refusal and a
late stop during an owned final save.

Marked subprocess fixtures use the actual terminal builder, Responses loopback
HTTP, a physical `write_file` effect and SQLite. Actual SQL triggers refuse
readiness and final saves; the former sends no request and the latter retains
the original durable state. Explicit reopen/retry saves the summary and
continues without forwarding stale native items or repeating the write. Another
fixture exits during an observed pending summary request. Fresh owners reopen
both checkpoints without replay, requiring their distinct actual exit codes.
Unmarked/unknown-mode helpers return before path/config access.

All credentials, providers, config and files are synthetic and isolated. No
parent environment, user config, installed gadget or paid account is changed.
Initial new-test import/usage-type failures and the parent fixture's noncanonical
workspace mismatch are retained separately; only that fixture's explicit scope
was corrected to the actual builder's canonical workspace. Fresh full local and
all-four/every-step/both-native-mode CI validation, then independent main
verification, gate delivery. Live quality, containment, inherited shell issue
#138, public headless execution and publication retain their separate gates.

The first complete local command passes core but fails one original macOS
unread-pipe diagnostic in gadget mode: the real stdout descriptor is temporarily
unavailable while stderr still reports its actual 17 buffered bytes. Its ready
file was created during a shell stdout redirection, so file existence could
precede descriptor restoration. The fixture now renames a separately written
ready file after the redirect closes; a new isolated barrier case proves the
final ready name stays absent until publication and both original streams retain
their exact bytes. The old byte/native assertions, 20-second readiness and
two-second drain bounds remain. No production reader, ProcessTree or native
query changes, retries, extra PID queries or historical #138 root-fix claim
follow. The failed full command remains a separate record.
