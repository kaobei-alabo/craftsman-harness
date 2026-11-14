# Workspace file checkpoints and rewind

Craftsman can work in a project, a document folder, or a folder containing many
projects. Git is optional. The folder you start it in remains the working folder;
checkpoint selection does not change filesystem permissions or shell execution.

Conversation checkpoints save the agent's input, messages and tool results.
Workspace file checkpoints provide file undo through `/checkpoints` and `/rewind`.
These are separate services: read-only turns still save their conversation while
performing no automatic workspace scan for file undo.

## Capture follows mutations

Turn start records the checkpoint identity and user-request summary in memory.
It does not enumerate or hash workspace files. Reads and directory listings add
bounded directory hints without taking a workspace snapshot.

Before `write_file` or `edit_file`, the service captures that specific target.
The first observed bytes or confirmed absence are retained across later edits to
the same path. Additional files can join the same turn checkpoint, including
files in unrelated folders. Explicit file operations include their chosen paths;
automatic shell enumeration retains Git's ignored-file exclusions.

Shell commands can affect more than one file. Before an `execute` call, the
service uses directories established by successful file reads/listings and
requested file writes/edits. A broad initial listing does not override narrower
directory context. Later tool activity can add another directory. With no
narrower context, shell checkpointing uses a bounded scan of the launch folder.
Selection never guesses permissions or interprets a command string as a reliable
list of affected files.

Automatic scans exclude the selected application settings/state directory. The
live database and its WAL/SHM/journal companions are excluded from all file undo,
including older stored records. Explicit edits to ordinary configuration files
remain governed by their normal permissions; a folder is never rejected merely
because it contains application settings.

After a mutating tool returns, and at turn end when mutations were enrolled,
the service snapshots the selected paths and persists their observed changes.
Earlier observations remain recorded when a later partial scan cannot observe
those files, even when another file receives a newly observed edit.
Background mutation tickets retain the shared original observations after the
main turn returns. One checkpoint per turn, thread switches and the existing
20-checkpoint retention policy remain.

Approved gadget filesystem writes enroll their exact target after the existing
capability and per-write approval checks. Checkpoint work shares the gadget's
existing call clock; read-only gadget calls take no file snapshots. The app-only
runtime option defaults absent in standalone/frozen callers. SDK/WIT and host
operation limits remain unchanged.

MCP calls with an explicit standard `readOnlyHint: true` skip file snapshots.
Other MCP calls use bounded local checkpoint scopes because they may change
files. This advisory hint affects undo collection, never invocation permission;
remote effects and local effects outside captured scopes are not restorable.

## Partial coverage stays explicit

macOS can refuse directory access with `Operation not permitted`; other systems
can return permission denied. Those paths are marked unavailable. Readable files
remain eligible for checkpoints. A missing observation never becomes a claimed
file creation or deletion: both snapshots must cover a path before recording its
change. A later incomplete observation cannot delete an earlier recorded checkpoint.

The existing `partial` flag also describes a selected scope smaller than the
whole working folder. `/checkpoints` exposes it. A notice about unreadable paths
or scan limits appears once per session when encountered. Only recorded changes
are eligible for `/rewind`; arbitrary shell effects outside the selected scope
remain unrecorded. A newly enrolled scope starts from its actual pre-operation
observation, not a fabricated turn-start image.

Snapshots admit at most 4,096 directory entries or Git path records, 1 MiB of
Git listing output and 1,024 files per scan. Up to 64 directory scopes and 1,024
explicit file targets may be enrolled per turn. File reads share a 25 MiB hash
budget; a one-byte growth probe refuses an overrun without publishing a partial
hash. The existing limits of 5 MiB retained content per file and 25 MiB retained
pre-images per turn remain. Hash-only files remain unrestorable.

These are selection/data bounds, not an OS sandbox or a hard IO deadline. Native
blocking IO and Git's internal work keep their existing limitations. The existing
ShellRunner/ProcessTree cleanup and permission contracts are unchanged. Unix file
capture/rewind retains no-follow parent walks; Windows retains its documented
best-effort checks. Rewind still checks actual post-operation hashes/modes and
preserves conflicts unless the user explicitly chooses `--force`.

Nonregular entries, including symlinks, pipes and devices, are unavailable for
file snapshots. Unix capture checks the final entry through its retained parent
before opening it and uses a nonblocking, no-follow open against a type-change race.
This does not change which operations the ordinary file tools can perform.

## Notices stay in the terminal UI

The session builder passes the frontend's existing activity sink to the file
checkpoint service. The TUI receives notices through its event queue, stores them
in chat activity and renders them through SafeBackend. The service does not fall
back to stderr when an explicit frontend sink is present, including during
shutdown. Standalone frontends without a sink retain stderr diagnostics.

This fixes the direct stderr writes that appeared inside the composer. Draft
text and cursor position are preserved. Existing copy, selection, voice, approval
and cancellation behavior is unchanged.

## Verification

Local-file regressions cover read-only turns, multiple non-Git folders, evolving
shell scopes, literal Git pathspecs, byte/file/entry/output limits, permission
changes, false creation/deletion prevention and retention of earlier checkpoints.
Original rewind, conflict, binary, mode, symlink-parent, pruning, thread-switch
and background-ticket assertions remain, with mutation fixtures explicitly
enrolling before their file operations.

Two marked actual-builder cases use loopback HTTP and isolated local SQLite
configuration. They check a read-only turn with an unavailable workspace and an
actual approved file operation whose checkpoint warning reaches chat without
stderr output or draft corruption. The same ignored helper exercises the bounded
Git-output reader; unmarked/unknown startup returns before config/path access.
No user configuration, API credentials, SDK/WIT, dependency pins, SQL layout,
retained request/frame goldens or publication settings are changed. Full local,
all four hosted jobs/every mandatory step/both native modes and independent-main
validation remain required before delivery.
