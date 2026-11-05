# Desktop connection ownership and reconnect

The desktop bridge permits one active client. A client that closes an idle
WebSocket can reconnect immediately, without receiving the live-client
`client_already_connected` error for the reader that is still finishing cleanup.
Application frames remain the frozen desktop protocol v4.

## Actual ownership and cleanup

`DesktopConnection::attach` selects the existing bridge generation and holds an
owned asynchronous mutex lease for that reader. A competing connection waits
once, for at most **250 ms**, on that same mutex. It can enter only when the
original owner actually releases the lease. A live client retains ownership and
the competing request receives the existing error. There is no incumbent
replacement, retry loop, sleep, fabricated close or extra permission grant.

The reader treats an actual WebSocket Close as its end, instead of ignoring the
frame and relying on a subsequent read poll. Its connection guard requests
voice/main-turn cancellation before detaching the old generation and releasing
pending human-input waiters. The ownership lease stays held through those calls;
a new client cannot be admitted and then cancelled by old-reader teardown.
Guard Drop also performs cleanup if the reader task is aborted.

Cancellation is cooperative. The original turn context remains until its runner
actually returns; the existing generation filters keep it out of a successor's
snapshot. Removing a connection is not evidence that a model, native command or
foreground subagent has stopped.

A private writer-stop channel ends the old socket writer even if background
tasks retain frame senders. A reader-owned abort guard also cancels an in-flight
writer when that reader is dropped/aborted. The writer sends the normal WebSocket close control
frame, with a **1 s** close bound. Joining the writer has a **1 s** bound, followed
by abort and a separately bounded **1 s** join when needed. A refused connection
first drains its existing operation-error frame. These are connection-cleanup
bounds, not OS/native-work deadlines, delivered-byte receipts or hard scheduling
guarantees. Existing application schemas, frames, host limits and process behavior
are unchanged.

## Failure and validation record

PR #2's first source `e00bfa8` exposed the ORIGINAL
`desktop_serve::tests::idle_disconnect_allows_immediate_reconnect` assertion on
Ubuntu in gadget mode: **1987 passed / 1 failed / 12 ignored**. Core mode passed
**1833 / 0 / 10**. All nine terminal cancellation cases and all 30 request-budget
cases passed in both modes. The unexpected first reconnect frame was
`operation_error` instead of `hello`; the old assertion and its **1 s** connection
bound remain exact. This failed run is retained separately:
[42082095608](https://github.com/yumingchangsabodota/craftsman-harness/actions/runs/42082095608).

Eight additive cases verify ordered cleanup while the actual driver callback is
paused, real approval/question waiter refusal, retained original turn context,
single-owner admission under contention, live-client rejection, reader abort,
writer abort/resource Drop and real native WebSocket close/reconnect behavior.
The first focused run had **35 / 1 / 0**: a new test incorrectly assumed that
connection cleanup completed the original turn. That new assertion now verifies
retained original context; no original assertion or deadline is weakened.
First and corrected focused/full evidence stays under `target/verification/`.
Fresh complete local and hosted checks are required before merge.

Final focused verification passed **38 / 0 / 0**, including all original desktop
tests and the eight additions, with no compiler warnings. This focused result is
not a substitute for the failing whole hosted run or corrected full-mode checks.
The first hosted run also passed macOS **1887 / 0 / 10**, **2042 / 0 / 12** and
Windows **1811 / 0 / 10**, **1955 / 0 / 12** (core, gadgets); the Ubuntu gadget
failure remains a failure. No unchanged whole run was retried.

The independently audited request-budget main run
[42078817023](https://github.com/yumingchangsabodota/craftsman-harness/actions/runs/42078817023)
remains a separate passing checkpoint; it does not replace this failing PR run.
Earlier cold `/stats` and deferred shell failures also remain separate. No user
configuration, installed gadget, live model, microphone, SDK/WIT, schema, golden,
privileged trace or old native process was used or changed for this correction.
