# Memory during coding tasks

The TUI and ACP explicitly select task-focused memory guidance. The harness
already loads `/memories/AGENTS.md` at turn start and supplies that snapshot to
each model round. The guidance asks the model to use those facts when relevant,
begin the current task, and read additional memory only when a missing durable
fact matters. Current user requests, repository instructions and permissions
take priority over remembered preferences.

Task requirements, test results and repeated boilerplate are not lasting user
preferences. When the user establishes a preference for future work, the model
can update `/memories/AGENTS.md`, preserving unrelated entries and using the
existing read/write permission checks. This file is loaded on the next turn;
the current turn keeps its original snapshot. Credentials must never be saved
as memory.

`MemoryMiddleware::new` retains its established guidance. The additive
`task_focused()` method selects the new text, and the app's
`SessionExtras::task_focused_memory` defaults to false. Only TUI/ACP callers
set it. Legacy callers keep the old behavior; the private prepared runtime
continues to omit memory entirely. No storage migration, grant change, tool
definition change, checkpoint replay or new ACP method is introduced.

This is model guidance, not filesystem authorization or a guarantee that a
model will follow the requested workflow. Coding-quality and cost effects are
measured independently using the [coding evaluations](coding-evaluation.md).

Framework regressions exercise the actual model request, retained preference
content, turn-start refresh and default legacy behavior. A marked native app
fixture exercises the shared builder, actual HTTP request and local SQLite
memory in both policy modes. Its unmarked or unknown invocation returns before
environment/config access. Existing tests, native limits, SDK/WIT/pins and
goldens remain unchanged.
