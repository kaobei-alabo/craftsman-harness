# Terminal work cancellation and shutdown

The terminal session now owns cancellation through model requests, automatic
and manual compaction, foreground helpers and native commands. Ctrl-C asks
running native commands to stop through their existing `ProcessTree` owner;
the tool future remains alive while that owner kills, reaps and drains. Inline
`!command` and manual `/compact` work runs on owned tasks so input remains
responsive. Finished operations clear their UI state only after actual return.

## Turn and session owners

The opt-in framework scope has a separate turn stop and session shutdown owner.
Foreground helper turns share the parent's turn. Background helpers receive a
fresh turn under the same session; cancelling the main turn leaves them running.
An explicit background stop signals its scope and keeps its registry entry
running while cleanup finishes. Session shutdown stops all of those scopes,
including work queued before its first poll. Session generations have separate
owners under the terminal shutdown root, so rebuilding closes the old session
without closing the new one. Parent depth is bounded at 32.

The model opt-in defaults false. Legacy/private/frozen agents explicitly mask
ambient turn scopes. Private prepared construction never selects the terminal
session owner. The framework uses only its existing Tokio/standard-library
dependencies; HTTP, terminal, database and process implementations stay in the
application. Providers and model middleware in the opted-in terminal path must
be safe to drop. Filesystem/mutation hooks and tools retain their own work.

Compaction cancellation discards the incomplete summary and retains the original
conversation. Manual compaction also refuses to overwrite a conversation that
changed while the summary was being generated. A foreground helper's already
committed writes still merge back on cancellation. Cancellation promises no
rollback. Missing model usage remains unknown; a stop is not proof that remote
generation or billing ended.
Turn-end work is attempted once. If shutdown interrupts an end hook, the
framework does not replay its effects or append a fabricated replacement answer.

## Native commands and signals

Signal receivers are installed before raw terminal entry and agent construction.
Unix SIGINT, SIGTERM and SIGHUP cancel the terminal shutdown root; Windows uses
the console interrupt/break/close/logoff/shutdown receivers. Keyboard Ctrl-C in
raw mode remains a turn stop. `/quit`, Ctrl-D and terminal input closure use the
same cleanup path. The TUI returns its exit code after cleanup, allowing the main
runtime to finish normally. Unix signal codes remain 128 plus the signal.

An opted-in native call owns a Unix process group. Windows retains its existing
Job Object rules. Default `shell.process_group=false` remains unchanged for
unscoped/legacy execution; the terminal scope selects an owned group because it
now has real signal routing. All commands still use `platform_os::ProcessTree`.
The existing 5-second kill/reap and 2-second pipe-drain bounds remain unchanged.
Cancellation is observed before spawn, during pipe reads and during the direct
child wait. Completed commands retain their actual exit and byte facts. A
cancelled operation has typed `Cancelled` metadata; unknown exit status is not
reported as zero, and a pre-spawn stop has no invented observed-byte count.

Shutdown closes prompt delivery, refuses pending/late approvals and questions,
and waits for foreground, background, retired-session, voice and LSP owners.
Scoped synchronous prompt callbacks have an owned cancellation waiter, so a
busy or closing UI cannot strand them. Cancellation reaches the actual turn
before a denial wakes a tool; a highlighted Always choice and partial question
answer cannot become a grant after a stop.

Overall terminal cleanup has a 30-second bound. Exceeding it returns an error
that explicitly says work may still be completing. Process groups and Job
Objects are not a native OS sandbox: an escaping process or uninterruptible
filesystem worker is not proved quiescent. Builds retain their normal native
permissions and atomic component publication; completed effects are retained.

The terminal owns raw-mode restoration on normal return, draw errors, signals
and panic. Its panic hook cancels the shutdown root and emits fixed text rather
than printing a potentially secret/control-bearing panic payload. Clean exit
restores the prior hook. Diagnostic retention and live console-event behavior
still need evaluation; no complete OS-containment claim follows from these
changes.

## Verification and remaining gates

Additive tests exercise actual foreground/background framework work, compaction,
owned cleanup status, legacy scope isolation, newly owned native descendants,
pre-spawn metadata and real Cargo gadget-build cancellation. Unix tests use
newly allocated PTYs, the actual TUI, actual Ctrl-C/SIGTERM/SIGHUP and terminal
settings recorded while the child still owns the terminal. macOS revokes a PTY
after its controlling process exits, so exit alone is not restoration evidence.
Darwin may set `PENDIN` for canonical-input reprocessing after restoration. The
new PTY comparison permits only that kernel-managed transient bit; all configured
mode flags remain exact and raw terminal values are retained. It consumes or
flushes no input to manufacture equality. The original tests are unchanged.
No parent environment, user config, installed gadget or model account is used.
Native helper markers select only registered cfg-test cases; unmarked helpers
return before configuration or terminal inspection.

Full local and all-four hosted verification remain required before merge, with
independent final-main verification before local fast-forwards. Keep failed
source checks and the first PTY ABI/state failures separate from later results.
SDK/WIT, dependency pins,
schemas, retained ACP frames and goldens remain unchanged. Live task/gadget
quality, crash diagnostics, native containment and durable recovery still need
their separate validation. Binary and container publication remains disabled.

The subsequent [terminal recovery](terminal-recovery.md) continuation adds required
per-tool checkpoints and unknown-completion notices. Its verification is separate
from the lifecycle PR/main results, and native/diagnostic/live-quality limits remain.
