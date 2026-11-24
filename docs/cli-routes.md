# How `craftsman` chooses and owns its route

This is local source on an unmerged, uninstalled branch. It has been checked
on macOS only. It is not validated on Windows, where the failure it answers
was seen; see [what is not established](#what-is-not-established).

## What `main` does

`main` parses the invocation, asks `route` for the invocation's route, and
awaits that route. All of it is in `crates/craftsman-rs/src/main.rs`.

| Invocation | Route | Where its future lives |
| --- | --- | --- |
| No subcommand, with `-m MESSAGE` | `run_once`, the legacy one-shot | An allocation of its own |
| Everything else | `rest`: the `privacy` commands, the owned private terminal, the other subcommands and the terminal | One other allocation |

`route` is an ordinary function. It moves the chosen route's future into its
allocation and returns before any of that route runs, so `main` holds one
pointer while a route runs.

The order of effects is the one `main` had:

- The `privacy` commands run before the loader and never load a model
  configuration.
- The eligible interactive invocation (no subcommand, no `-m`, no `--session`,
  a terminal on both ends) passes through `privacy_startup::normal_route`
  before the loader. [Private terminal startup](privacy-startup.md) describes
  that route.
- Every other invocation starts with `configured`: the loader, this
  invocation's permission mode, the note about ignored legacy login fields,
  and, in a build without the gadget runtime, the refusal of the gadget
  flags.
- A subcommand wins over `-m`, as before.

The one-shot's inputs, output owner, signal hook, exit codes and error text
are unchanged. `run_once` now owns its message and returns the exit code that
`main` used to supply for it.

## Why a route has its own allocation

`main` is polled on the process's main thread. The platform sizes that
thread's stack; the Windows default is 1 MiB. The one-shot's turn runs on
that thread, and a delegated agent's whole turn runs inside one of its tool
calls, so this route is deep there.

A route that `main` awaits in its own body is part of `main`'s future and of
the frame that polls it, whichever route actually runs. Before this change
`main` awaited every route that way: gadgets, setup, the ACP server and
bridge, the service agent, the one-shot and the terminal, and since the
privacy work also the `privacy` commands and the whole private terminal. A
`-m` run takes none of the others, and still ran with all of them under it.

No stack size, linker setting, thread or timer is changed. The routes inside
`rest` still share one future among themselves, as they did in `main`.

## The failed hosted run

Run 45464728443, attempt 1, at `9b63392` (checkout `0d2985c`) is retained as
failed. In the gadget mode on Windows, two cases of `tests/loop_test.rs`
failed at line 604:

- `migrated_gadget_agent_tools_stay_behind_the_approval_gate`
- `plugin_agent_tools_stay_behind_the_approval_gate`

In both, the real `craftsman -m` child printed the `task` tool line and then
exited with 0xC00000FD; its stderr ends with `thread 'main' has overflowed its
stack`. The core mode of the same job passed the plugin case, Linux and macOS
passed both, and the main baseline run 44985966590 passed both on Windows.

## What is not established

- **That `main` was the whole cause.** The source shows what `main` owned.
  How deep the delegated turn itself goes was not measured, and the same
  work changed other files on that path. The correction removes `main`'s
  share and nothing else.
- **That the two cases pass on Windows.** No Windows run was made. No stack
  limit was lowered to imitate Windows, and no stack depth was read at run
  time. A fresh native run of the final source has to show it.

## Checks on this change

- Type checks of the gadget mode, the core mode and the default build pass
  without a warning; formatting is unchanged.
- The gadget-mode unit-test binary's `__eh_frame` on macOS is 16,776,644
  bytes with this change and the two fixture corrections made with it, 216
  more than before them and 572 under the 16,777,216 limit. This is one
  binary of these changes alone. The integrated source has to be measured
  again.
- On macOS, in the gadget mode, the eleven test files that drive the real
  binary pass on the changed routes, 67 cases: `loop_test` (9, with both
  cases above), `shell_test` (9), `oneshot_contract` (6),
  `oneshot_observation` (5), `branding_contract` (4),
  `local_connections_test` (2), `golden_model_request` (6),
  `gadget_migration_test` (2), `gadget_test` (4), `gadget_authoring_test`
  (4) and `privacy_cli` (16). In the core mode the eight of them that have
  cases there pass, 42 cases. The three `privacy_cli` cases that need voice
  input were not run. A macOS pass says nothing about the Windows stack.
