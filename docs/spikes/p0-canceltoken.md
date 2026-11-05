# P0 spike 3: a CancelToken through the loop, one tool and the approval callback

Status: finished. Branch `p0/spike-canceltoken` is throwaway; only this document is meant to be merged. The code is on base `a055e5d` in two commits, the core `5b18fc2` and the extension `970cd16`. Question from the plan (section 7.2, P0): how big is it to thread a cancellation token through the loop, one tool and the synchronous approval callback, and what does that say about the size of P2c "Interrupt and terminal hygiene" (6-7 engineer-weeks)?

Provenance. Every number below comes from binaries and test executables built from the named commits in one private target directory (`--locked --no-default-features`, rustc 1.99.0, debug profile) on a 10-core Apple-silicon Mac that other builds and an endpoint-security agent shared. Each table states the one-minute load average before and after its runs; the run script waited for it to fall below 3 before each run and it stayed between 2.5 and 3.7 while the runs reported here were in progress (one short run ended at 7.6; it is timer-bound and says so), so strictly the "below 3" condition holds at the start of each run and not always at its end. Binary hashes are in section 3.7.

## 1. Result in numbers

- The token threads through with no new dependency and no `Cargo.toml` or `Cargo.lock` change. Core: 20 files, +2356 / -128 lines, of which production code is +471 / -81 in 14 files and tests plus the measurement harness are +1885 / -47. Extension: 5 files, +167 / -4, of which production +40 / -3 in 4 files (section 5).
- One token per turn now reaches the model call, the `before_model` hooks, every tool (`ToolCtx::cancel`), `execute` (which kills its whole process group), the approval prompt, `ask_user` and, with the extension, a foreground subagent and its shell. Ctrl-C now cancels from inside the approval and `ask_user` modals.
- Library-level cancel latency (clock starts at `request_cancel()`, N = 20 per row): the turn returns in at most 0.4 ms at p95 for `execute`, `sleep 60 & sleep 60`, a stalled model stream and a foreground subagent running a shell, and no process of the command is left at most 0.5 ms (p95) after the request. With the real `craftsman` binary under a pseudo-terminal (clock starts at the write of the Ctrl-C byte, N = 20 per row) the screen shows the cancelled call after 29-39 ms at p95 and no process is left after at most 39.4 ms at p95, for the plan's five scenarios; the streaming-reply scenario is measured as the moment the mock model server sees its connection close (3.0 ms at p50 and 4.9 ms at p95 after the Ctrl-C write).
- Where it is not fast: a tool that ignores the token is dropped after a fixed 200 ms grace (the foreground subagent without the extension: 203 ms in process, 249 ms at p95 in the real binary); with the production workspace-snapshot middleware installed the cancel returns after about 200 ms + 0.12 ms per file in the workspace (445 ms at 2,000 files, 1.4 s at 10,000, 2.7 s at 20,000); synchronous file tools cannot be interrupted at all (a cancel requested during a `grep` over 30,000 files returned after 1354 ms at p50, which is the whole run time of the grep).
- Of the 541 pre-existing tests, 538 pass on the new code as they were (16 `ToolCtx` literals in them only gained the new `cancel` field), 2 had to become `async` because a private function they call did, and 1 asserted the old cancel semantics and was rewritten (section 5.4). The workspace test command now reports 575 passed, 5 ignored (four of the ignored are the measurement tests).
- `process_group(0)` is acceptable for `execute` only if every front end that can receive a terminal signal turns it into a cancel. The prototype does it for SIGINT in `craftsman -m` (without it a Ctrl-C leaked all 3 processes of a 3-process command where the current code leaks 1). It does not do it for the TUI's SIGHUP: closing the terminal window while a command runs now leaves the command running (3 of 3 processes, where the current code leaves 0). That makes the SIGHUP, SIGTERM and SIGINT handler of P2c deliverable 2 a precondition of shipping the process group (section 4).
- Recommendation (section 9): the spike supports the plan's 6-7 engineer-weeks for P2c and does not lower them; the token threading is about a third of the phase, the stat-cache work is on the critical path of the latency gate, and the seams with P2b (process group), P2g (signals, synchronous tools) and P6a (async approvals) should be written into the plan.

## 2. What was built

### 2.1 Where the token is observed

| Place | Mechanism | Files |
|---|---|---|
| Turn | `TurnInputState.cancel`, a fresh `CancelToken` per `begin_turn`; `request_cancel()` fires it (the flag it replaces was read only at round boundaries and before each tool dispatch) | `deepagents-rs/src/agent.rs`, `cancel.rs` |
| Model call and `before_model` hooks | `tokio::select!` against the token; the losing future is dropped, which closes the SSE socket | `deepagents-rs/src/agent.rs` |
| Any tool | `ToolCtx::cancel`. The loop runs each call under `run_until_cancelled`: a tool that watches the token returns by itself; any other is dropped 200 ms (`CANCEL_GRACE`) after the token fires and the call gets the same result as a call that never ran | `deepagents-rs/src/agent.rs`, `tool.rs` |
| `execute` | `Backend::execute_cancellable`; `execute_impl` starts the shell as leader of its own process group (`process_group(0)`) and races the child against the token; `ProcessTree` SIGKILLs the group on cancel and also when the call is dropped | `craftsman-rs/src/tools.rs`, `backend_fs.rs`, `store_fs.rs`, `agent.rs` |
| Approval prompt, `ask_user` | the synchronous callback runs on the blocking pool through `CancelToken::run_blocking`, which stops waiting when the token fires | `hitl_middleware.rs`, `ask_user.rs` |
| TUI | Ctrl-C inside an approval or `ask_user` modal requests the cancel; the main loop fires the token first and then drops the modals (dropping their reply channels releases the parked threads); `finish_turn` closes tool blocks of calls the loop abandoned | `tui_repl.rs` |
| `craftsman -m` | SIGINT is turned into `request_cancel()` (a second press exits 130) | `main.rs` |
| Extension: foreground subagent | `AgentHandle::follow_cancel`: the child runs under the parent's token, the registry entry is marked `Failed`, and the `SubagentGate` prompt is raced like the main gate's | `deepagents-rs/src/subagent.rs`, `builtin/subagents.rs`, `agent.rs`, `craftsman-rs/src/subagent_gate.rs` |

### 2.2 Core and extension

- Core (`5b18fc2`): everything above except the last row. Cancelling the parent already stops a foreground subagent here, through the backstop: the `task` call is dropped 200 ms after the token fires and `ProcessTree::drop` kills the shell's group.
- Extension (`970cd16`): the foreground subagent follows the parent's token. It buys the 200 ms, a correct registry entry (a dropped child never gets to set its status, so by construction it stays `Running` and counts against the delegated-agent cap) and a cancellable subagent approval prompt (that prompt blocks inside `poll`, so no timer can reach it).
- The TUI ordering fix is in the core, not in the extension, because it belongs to Ctrl-C from the main agent's modals: fire the token first, drop the modals second. In the other order the parked callback returns a denial that the turn can read before it sees the cancel, so the tool result says "rejected" instead of "cancelled". `ctrl_c_in_a_prompt_cancels_instead_of_typing_a_c` pins the order.

## 3. Measurements

### 3.1 What is timed, and what each valid run checked

- Library level: the clock starts at `DeepAgent::request_cancel()` (for the modal rows, at the Ctrl-C key event reaching `handle_key`, with the main loop's two follow-up calls made by the test) and stops when the `run_turn` future resolves, or, for "no process left", when `kill(pid, 0)` fails for every pid that `pgrep` listed before the cancel (the shell and its sleepers), polled every millisecond and then confirmed by one `pgrep`. It excludes the terminal read before the key event and the redraw after the turn.
- Every run asserted its outcome, so a row holds only valid runs: the tool result is exactly `Tool call cancelled by the user.` with `is_error`; for the approval modal and for the pending-approval test the command or tool never ran (a marker file or flag); for the model stream the server saw the socket close; for the subagent the registry has no `Running` entry (extension build).
- Real binary: `craftsman` (TUI, `--auto-approve` except for the approval and `ask_user` rows) runs under a pseudo-terminal against a local mock of the OpenAI chat-completions API that answers with one tool call and then plain text. The driver waits until the command's sleepers appear in `pgrep`, writes the byte 0x03 to the pty master (raw mode: a key event, not a signal) and records when the screen output first contains `cancelled by the user` and when every pid of the command is gone. For the streaming-reply scenario the mock sends one text delta and then waits; the clock stops when the mock server sees the model connection close. The screen time includes the TUI's 16 ms frame tick and the driver's own 1-5 ms polling. The plan's "composer is live" is not detected: there is no terminal emulator in the driver.
- p50, p95 and max use the nearest-rank rule over the runs shown. Runs per row: 20 unless stated.

### 3.2 Library level, final build (`970cd16`), load 2.98 before and after

| Scenario | Timed from the cancel request until | p50 ms | p95 ms | max ms |
|---|---|---|---|---|
| `execute` `sleep 30; echo` (craftsman, sh, sleep) | the turn returned | 0.4 | 0.4 | 0.7 |
| | no process of the command left | 0.4 | 0.4 | 0.7 |
| `execute` `sleep 60 & sleep 60` (a background and a foreground sleeper) | the turn returned | 0.4 | 0.4 | 0.6 |
| | no process of the command left | 0.4 | 0.5 | 3.3 |
| model stream stalled after one delta (real SSE client) | the turn returned | < 0.05 | < 0.05 | < 0.05 |
| | the server saw the socket close | 0.1 | 0.1 | 0.1 |
| foreground subagent running `sleep 30` | the parent turn returned | 0.4 | 0.4 | 0.6 |
| | no process of the command left | 0.4 | 0.4 | 0.9 |
| approval modal open (the TUI's own prompt closure, real middleware) | Ctrl-C reached `handle_key` until the turn returned | < 0.05 | < 0.05 | < 0.05 |
| | until the prompt's thread was free | < 0.05 | < 0.05 | < 0.05 |
| `ask_user` modal open (same) | until the turn returned | < 0.05 | < 0.05 | < 0.05 |
| | until the prompt's thread was free | < 0.05 | < 0.05 | < 0.05 |

The two modal rows print 0.0 at the harness's one-decimal precision; "< 0.05" is what that means. The streaming-reply row is the same mechanism as the model-call row in the real-binary table.

### 3.3 Core build (`5b18fc2`) without the extension, load 2.83 before and 7.59 after

| Scenario | Timed from the cancel request until | p50 ms | p95 ms | max ms |
|---|---|---|---|---|
| foreground subagent running `sleep 30` | the parent turn returned | 203.4 | 203.5 | 203.6 |
| | no process of the command left | 205.4 | 205.9 | 206.0 |

The other rows of the core build equal the final build's within 0.3 ms. The load rose after the run; the spread of 0.2 ms shows it did not matter for a timer-bound row.

### 3.4 Real binary under a pseudo-terminal, load 2.98 before and 3.22 after (`final` = `970cd16`) / 2.95 and 2.84 (`core` = `5b18fc2`)

| Build | Scenario | Timed from the Ctrl-C write until | p50 ms | p95 ms | max ms |
|---|---|---|---|---|---|
| final | `sleep 30; echo` through `execute` | screen shows `cancelled by the user` | 32.0 | 39.0 | 41.0 |
| | | no process of the command left | 32.5 | 39.4 | 40.9 |
| final | `sleep 60 & sleep 60` through `execute` | screen shows `cancelled by the user` | 32.0 | 38.0 | 39.0 |
| | | no process of the command left | 32.2 | 38.0 | 39.3 |
| final | approval modal open (`touch` marker, never created) | screen shows `cancelled by the user` | 28.0 | 33.0 | 40.0 |
| final | `ask_user` modal open | screen shows `cancelled by the user` | 26.0 | 29.0 | 40.0 |
| final | foreground subagent (`task`, plugin agent `worker`) running `sleep 30` | screen shows `cancelled by the user` | 31.0 | 38.0 | 39.0 |
| | | no process of the command left | 31.1 | 37.7 | 39.4 |
| core | the same foreground subagent | screen shows `cancelled by the user` | 241.0 | 249.0 | 250.0 |
| | | no process of the command left | 241.4 | 249.5 | 249.7 |
| final | streaming reply: the mock streams one delta and then waits (load 2.99 before and after) | the mock server saw the model connection close | 3.0 | 4.9 | 5.4 |

All 20 runs of every row were valid. These are debug builds; the numbers say the cancel work is far below the plan's gates (1 s in CI, 300 ms on a developer machine) and that what remains is the terminal path.

### 3.5 Control: the same scenarios on the unmodified code

One run each of the pristine `a055e5d` binary (an unmodified build: the source export it was built from is byte-identical to the commit, and the binary contains the old `stopping at the next safe boundary` text), Ctrl-C written the same way, observed for 15 s:

| Scenario | Result |
|---|---|
| `sleep 30; echo` through `execute` | no cancel on screen within 15 s; both processes of the command still alive 15 s later (the flag is read only after the tool returns) |
| approval modal open | nothing within 15 s (Ctrl-C does nothing in the picker: plan A7) |
| `ask_user` modal open | nothing within 15 s |
| model stream stalled | the server did not see the connection close within 5 s |

### 3.6 The production snapshot middleware and a synchronous tool (final build)

These rows use N = 5 because building and hashing the trees is slow.

| Scenario | Runs | Load | Timed from the cancel request until | p50 ms | p95 ms | max ms |
|---|---|---|---|---|---|---|
| `execute` `sleep 30` with `WorkspaceCheckpointManager`, 2,000 files | 5 | 2.48 / 2.48 | the turn returned | 445.3 | 460.5 | 460.5 |
| same, 10,000 files | 5 | 2.48 / 3.09 | the turn returned | 1434.2 | 1490.8 | 1490.8 |
| same, 20,000 files | 5 | 2.93 / 3.65 | the turn returned | 2732.1 | 2830.3 | 2830.3 |
| `grep` over 30,000 files, alone (no cancel) | 5 | 2.99 / 3.42 | the grep ran | 1357.6 | 1392.8 | 1392.8 |
| same, cancel requested 5 ms into the turn | 5 | 2.99 / 3.42 | the turn returned | 1354.0 | 1400.2 | 1400.2 |

The trees are synthetic (small files spread over 100 directories; a git repository for the snapshot rows) and the harness builds each once per size. With the middleware installed, the post-command snapshot in `finish_mutation` takes longer than the 200 ms grace, so the loop drops it, and the turn-end `after_agent` snapshot then hashes the tree again: the cancel returns after about 200 ms plus one full snapshot, 0.12-0.13 ms per file here (a full snapshot before the command, at turn start, took 289 ms, 1247 ms and 2490 ms for the three sizes). The dropped snapshot is not stopped, because it runs in `spawn_blocking`: it keeps hashing in the background. On this tree the 300 ms developer-machine budget is exceeded from about 800 files and the 1 s CI gate from about 6,500. This is the plan's reason to keep the snapshot off the cancel path (stat cache, P2c deliverable 3); the numbers make that work a precondition of the latency criterion, not an optimisation.

### 3.7 Binaries and how to rerun

| File | Commit | sha256 |
|---|---|---|
| `bin/core/craftsman`, `bin/core/craftsman-unit-tests` | `5b18fc2` | `5c197215...f04f6e`, `fb970a2c...e5cc` |
| `bin/final/craftsman`, `bin/final/craftsman-unit-tests` | `970cd16` | `509fa18d...3ed`, `3fb002e0...d3a` |
| `bin/baseline-craftsman` (control, sections 3.5 and 4) | `a055e5d` (unmodified; the source export it was built from is byte-identical to the commit) | `82f390dc...aee4` |

(under `/tmp/oc-p0-artifacts/wp5a/`). To rerun from the repository root on `p0/spike-canceltoken`, with a private `CARGO_TARGET_DIR`:

```text
cargo test --locked --no-default-features -p craftsman-rs measure_cancel_latency_loop -- --ignored --nocapture --test-threads=1
cargo test --locked --no-default-features -p craftsman-rs measure_cancel_latency_tui  -- --ignored --nocapture --test-threads=1
CANCEL_SPIKE_FILES=10000 CANCEL_SPIKE_RUNS=5 cargo test --locked --no-default-features -p craftsman-rs measure_cancel_latency_with_the_workspace_snapshot -- --ignored --nocapture --test-threads=1
CANCEL_SPIKE_FILES=30000 CANCEL_SPIKE_RUNS=5 cargo test --locked --no-default-features -p craftsman-rs measure_cancel_latency_of_a_synchronous_grep -- --ignored --nocapture --test-threads=1
```

`CANCEL_SPIKE_RUNS` (default 20) sets the runs per row; each test prints a markdown table with the load average before and after. Run them one at a time: the name filter is a substring match. The real-binary rows come from three small Python scripts outside the repository (`/tmp/oc-p0-artifacts/wp5a/pty/mock_llm.py`, `pty_run.py`, `pty_measure.py`; standard library only): `python3 -I pty_measure.py final=<path to craftsman> 20 fg,sleep,approval,ask,subagent,stream`. They start the mock API, give `craftsman` a temporary `HOME` and `CRAFTSMAN_CONFIG_DIR`, and drive it as described in 3.1. A rerun of the whole set takes about 10 minutes on a quiet machine.

## 4. `process_group(0)`: what happens to Ctrl-C

`execute` now starts the shell in a process group of its own, so that one `kill(-pgid)` reaches the shell and everything it forked. The cost is that the command leaves the terminal's foreground group, and the signals the terminal generates for the foreground group no longer reach it: SIGINT and SIGQUIT on a key, SIGTSTP on Ctrl-Z, and SIGHUP when the terminal goes away.

All runs below put `craftsman` under a pseudo-terminal as a job of an interactive `zsh -f -i` (so the shell, not craftsman, is the session leader, as in a real terminal) and count the processes of the command that are still alive 1.5 s after the event. `sleep 60 & sleep 60` is three processes (the shell and two sleepers); `sleep 30; echo` is two. "Pristine" is the unmodified `a055e5d` binary; "final" is `970cd16`; the two other Ctrl-C builds are `5b18fc2` with one thing taken out.

| Event | Build | `sleep 60 & sleep 60` | `sleep 30; echo` |
|---|---|---|---|
| Ctrl-C in `craftsman -m` (cooked mode: SIGINT to the foreground group) | pristine, no process group | 1 of 3 left (the background sleeper: a non-interactive shell starts asynchronous commands with SIGINT ignored) | 0 of 2 |
| | own process group, no handler (`5b18fc2` without its `main.rs` hunk) | 3 of 3 left | 2 of 2 left |
| | own process group and SIGINT turned into a cancel (`5b18fc2`) | 0 of 3 left | 0 of 2 left |
| Terminal closed while the TUI runs the command (hangup) | pristine | 0 of 3 left | 0 of 2 left |
| | `970cd16` | 3 of 3 left | 2 of 2 left |
| `kill -TERM` of craftsman (TUI) | pristine | 3 of 3 left | 2 of 2 left |
| | `970cd16` | 3 of 3 left | 2 of 2 left |

What follows from it:

- Ctrl-C in the TUI is unchanged: the terminal is in raw mode, Ctrl-C is the byte 0x03 and no signal is sent to anyone, with or without a separate group (the TUI rows of section 3.4).
- `craftsman -m` is where the earlier draft's worry is real: with a group of its own and nothing else, an interrupted run orphans the whole command. The core commit routes SIGINT into the token in 24 lines (`tokio::signal::ctrl_c()` calls `request_cancel()`, the second press exits 130) and `sigint_in_one_shot_mode_cancels_the_turn_and_kills_the_command` pins it. With it nothing is left, one process better than before.
- A closed terminal is a regression the prototype does not fix. Today the kernel's SIGHUP reaches the command through the shared foreground group and kills it with craftsman; with a group of its own the command keeps running after craftsman dies. The TUI has no SIGHUP handler (plan A8). The cure is the `shutdown()` of P2c deliverable 2 (SIGHUP, SIGTERM and SIGINT cancel the turn and kill the groups before the process exits), roughly one signal task and a bounded wait; it is not in the prototype.
- `kill -TERM` of craftsman leaks the command before and after; it is not a regression, and `shutdown()` fixes it too.
- A command that reads `/dev/tty` behaves differently (checked with a Python supervisor standing in for craftsman, not with craftsman): in the shared group it sleeps in a foreground read (`S+`) and would take keystrokes meant for the TUI; in its own group the kernel stops it with SIGTTIN (`T`, both the shell and `cat`). A stopped command ends only by SIGKILL, so the existing 120 s timeout, which kills the direct child only, would leave it behind; switching the timeout path to the group kill is one line (`tree.kill()` instead of `tree.disarm()`) and is P2b's item.

So `process_group(0)` is acceptable for `execute`, on the condition that the signal routing lands with it: SIGINT for every cooked-mode front end (done here for `-m`; `serve`, `acp-serve` and the desktop sidecar have no controlling terminal) and SIGHUP and SIGTERM for the TUI (not done). P2b owns the group and P2c owns the shutdown, so the plan has to order them: P2b's group cannot ship before P2c's `shutdown()` without turning a closed terminal into a leak (section 9).

Alternatives, with what each costs:

- Keep the own group and route the signals (chosen). Matches plan 5.3 and P2g; the cost is one hook per front end that can see a terminal signal.
- `setsid()` in the child (a new session). No controlling terminal at all, so `/dev/tty` opens fail at once instead of stopping the command, and the group is still killable; the signal story is the same as for the own group. It needs a `pre_exec` closure, which gives up the `posix_spawn` fast path on macOS.
- Stay in craftsman's group and walk the process table on cancel. The shared group keeps SIGINT and SIGHUP working for free, but the walk is racy (forks between the walk and the kill), linear in the number of processes and blind to a double-forked daemon; rejected.
- A pseudo-terminal per command (P2f's persistent sessions). The line discipline then delivers Ctrl-C and SIGHUP to the command's own foreground group; this is the end state for sessions and too heavy for a one-shot `execute`.

A limit that holds for every variant: a descendant that calls `setsid()` leaves the group and survives the kill (`a_descendant_in_its_own_session_survives_the_group_kill` shows it with `perl`); only containment (cgroups, Job Objects, a sandbox) catches it, which is P7a.

## 5. Size of the change

All counts are `git diff --numstat` of the commit; "production" is everything before the file's `#[cfg(test)] mod`, including comments and blank lines; "tests" includes the measurement harness (`tools::procs` and the `measure_cancel_latency_*` tests).

### 5.1 Core, `5b18fc2`

| File | + | - | production + | production - | tests + | tests - |
|---|---|---|---|---|---|---|
| `crates/deepagents-rs/src/agent.rs` | 297 | 62 | 75 | 31 | 222 | 31 |
| `crates/deepagents-rs/src/backend.rs` | 14 | 0 | 14 | 0 | 0 | 0 |
| `crates/deepagents-rs/src/builtin/budgets.rs` | 1 | 0 | 0 | 0 | 1 | 0 |
| `crates/deepagents-rs/src/builtin/reminders.rs` | 1 | 0 | 0 | 0 | 1 | 0 |
| `crates/deepagents-rs/src/cancel.rs` (new) | 154 | 0 | 79 | 0 | 75 | 0 |
| `crates/deepagents-rs/src/error.rs` | 4 | 0 | 4 | 0 | 0 | 0 |
| `crates/deepagents-rs/src/lib.rs` | 3 | 0 | 3 | 0 | 0 | 0 |
| `crates/deepagents-rs/src/middleware.rs` | 1 | 0 | 0 | 0 | 1 | 0 |
| `crates/deepagents-rs/src/tool.rs` | 7 | 2 | 7 | 2 | 0 | 0 |
| `crates/craftsman-rs/src/agent.rs` | 693 | 5 | 9 | 5 | 684 | 0 |
| `crates/craftsman-rs/src/ask_user.rs` | 75 | 2 | 11 | 2 | 64 | 0 |
| `crates/craftsman-rs/src/backend_fs.rs` | 16 | 5 | 16 | 5 | 0 | 0 |
| `crates/craftsman-rs/src/hitl_middleware.rs` | 167 | 34 | 59 | 18 | 108 | 16 |
| `crates/craftsman-rs/src/main.rs` | 24 | 0 | 24 | 0 | 0 | 0 |
| `crates/craftsman-rs/src/store_fs.rs` | 8 | 0 | 8 | 0 | 0 | 0 |
| `crates/craftsman-rs/src/subagent_gate.rs` | 10 | 0 | 0 | 0 | 10 | 0 |
| `crates/craftsman-rs/src/tools.rs` | 458 | 17 | 113 | 17 | 345 | 0 |
| `crates/craftsman-rs/src/tui_repl.rs` | 327 | 1 | 49 | 1 | 278 | 0 |
| `crates/craftsman-rs/src/web_tools.rs` | 1 | 0 | 0 | 0 | 1 | 0 |
| `crates/craftsman-rs/tests/loop_test.rs` | 95 | 0 | 0 | 0 | 95 | 0 |
| **Total (20 files)** | **2356** | **128** | **471** | **81** | **1885** | **47** |

### 5.2 Extension, `970cd16`

| File | + | - | production + | production - | tests + | tests - |
|---|---|---|---|---|---|---|
| `crates/deepagents-rs/src/agent.rs` | 11 | 1 | 11 | 1 | 0 | 0 |
| `crates/deepagents-rs/src/builtin/subagents.rs` | 75 | 1 | 10 | 1 | 65 | 0 |
| `crates/deepagents-rs/src/subagent.rs` | 9 | 0 | 9 | 0 | 0 | 0 |
| `crates/craftsman-rs/src/agent.rs` | 9 | 1 | 0 | 0 | 9 | 1 |
| `crates/craftsman-rs/src/subagent_gate.rs` | 63 | 1 | 10 | 1 | 53 | 0 |
| **Total (5 files)** | **167** | **4** | **40** | **3** | **127** | **1** |

Both commits together: 22 files, +2521 / -130.

### 5.3 Public signatures that changed

`deepagents-rs` (library API):

- New: `deepagents_rs::cancel` and `CancelToken` (`new`, `Default`, `Clone`, `cancel(&self)`, `is_cancelled(&self) -> bool`, `async cancelled(&self)`, `async run_blocking<T, F>(&self, f: F) -> Option<T>`).
- `ToolCtx<'a>` gained a public field `cancel: CancelToken`. Breaking for every struct-literal construction: 16 test sites in 6 files here, plus the one in the loop.
- `AgentError` gained the variant `Cancelled` (display text `Tool call cancelled by the user.`). Breaking for an exhaustive match; none exists in this repository.
- `Backend` gained a provided method `async fn execute_cancellable(&self, command: &str, cancel: &CancelToken) -> Result<ExecOutcome, AgentError>`, which defaults to `execute`.
- Extension: `AgentHandle` gained a provided no-op method `fn follow_cancel(&self, parent: CancelToken)`; `DeepAgent::follow_parent_cancel` is `pub(crate)`.
- Unchanged signature, changed meaning: `DeepAgent::request_cancel(&self) -> bool` now fires a token that interrupts the model call and tools, where it set a flag read at round boundaries. `clear_pending_input` no longer resets a cancel flag (there is none; a new token is installed per turn). The private `take_cancel_request` (which consumed the flag) became `cancel_requested` (the token stays fired for the rest of the turn).

`craftsman-rs` (a binary crate, so all internal):

- `tools::execute_impl(cmd: &str, cwd: &Path) -> Result<ExecRaw, String>` became `execute_impl(cmd: &str, cwd: &Path, cancel: &CancelToken) -> Result<ExecRaw, ExecError>`, with a new `pub(crate) enum ExecError { Failed(String), Cancelled }`.
- `RealFs` and `CompositeBackend` implement `Backend::execute_cancellable`; `HostTool::call` uses it for `execute`.
- `ApprovalMiddleware::approval_decision` (private) went from `fn(&self, &ToolCall, &str, &str) -> Option<Decision>` to `async fn(&self, &ToolCall, &str, &str, &CancelToken) -> Result<Option<Decision>, AgentError>`.
- `ApprovalPrompt` and `AskUserPrompt` keep their types, but the contract changed: the callback now runs on a blocking-pool thread instead of the agent task, and a front end must release it (drop the reply channel) when it cancels, or its thread stays parked. The doc comments say so.
- `AskUserTool::call` and, in the extension, `SubagentGate::call` race their prompt against `ctx.cancel`.
- TUI, private: `App::drop_agent_prompts`, `App::cancel_from_prompt`; `handle_key` sends Ctrl-C inside an approval or `ask_user` modal to the cancel path while a turn runs; `finish_turn` closes tool blocks that never reported back; the line `Cancellation requested; stopping at the next safe boundary.` became `Cancellation requested; stopping the turn.`
- `run_once` (`-m`) installs the SIGINT hook and exits 130 after an interrupted turn.

### 5.4 What the change broke, and what it required changing

The pre-existing test modules (`a055e5d`) were put back under the new production code to find out (a scratch experiment, not committed): 117 framework tests passed (112 baseline tests and the 5 of the new `cancel.rs`) and 1 failed, 422 `craftsman-rs` unit tests passed (418 baseline tests as they were plus the four tests of the changed `hitl_middleware` module: the two adapted ones and two new ones), all 7 end-to-end tests passed, and the dependency guard passed.

- Failed: `agent::tests::cancel_during_model_call_stops_before_tools_and_balances_tool_calls`. It lets the in-flight model request return after the cancel and expects a cancelled result for the tool call in that reply. The model call is now dropped, so no such reply exists. Rewritten as `cancel_during_model_call_drops_the_call` (the call is dropped, no tool call is left unanswered) plus `cancel_as_the_model_replies_balances_tool_calls` (the old assertion, with the cancel fired as the reply arrives).
- Compile breaks, mechanical: 16 `ToolCtx { .. }` literals in tests in 6 files (`builtin/budgets.rs`, `builtin/reminders.rs`, `middleware.rs`, `ask_user.rs`, `subagent_gate.rs`, `web_tools.rs`) needed `cancel: CancelToken::new()`.
- Compile breaks, signature: the two `hitl_middleware` tests that call `approval_decision` became `#[tokio::test]` and `.await.unwrap()` the new result.
- Behaviour that no test caught and that front-end owners should know: a cancelled `execute` now yields `Tool call cancelled by the user.` instead of the finished command's output; a cancelled model call leaves no partial assistant message in the transcript (the streamed words stay on screen, not in the thread); a pending approval or `ask_user` is answered with a cancelled result instead of waiting; the protocol v4 `cancel` frame and ACP `session/cancel` keep their frames but the turn ends sooner and with different tool-result text, which the golden-frame sign-off in P0 and P2c has to cover.
- Test counts: framework 113 -> 123, `craftsman-rs` unit 420 -> 443 passed and 1 -> 5 ignored (four measurement tests), end to end 7 -> 8; workspace 541 -> 575 passed.

## 6. What was awkward

- `ToolCtx` is built by struct literal in 17 places. Any new field breaks all of them; a constructor would have left the call sites untouched.
- The approval and `ask_user` callbacks are synchronous closures that park a thread on a standard-library channel (`block_in_place`). A task parked there never reaches an `await`, so no timer or `select!` can run. The bridge (`run_blocking` on the blocking pool) works and is fast, but it leaves an implicit protocol: the front end must drop the reply channel, in the right order, or the thread stays parked until the process ends. The ACP server already works around the same problem with its own ordering rule (it resolves the waiters first and then aborts the task).
- Three front ends cancel three ways: the TUI sets a flag, the desktop sidecar sets the flag and rejects the waiters (`cancel_pending`), and ACP aborts the task (`cancel_in_flight`) without going through `request_cancel`. This spike changed the TUI and `-m`; the desktop sidecar gets the new behaviour unchanged because it already calls `request_cancel` before it rejects the waiters; ACP does not use the token.
- `kill_on_drop` kills only the direct child. The group kill had to be an owned guard (`ProcessTree`) so that every way a call can be abandoned, including the loop's backstop and an aborted task, reaches the group.
- The foreground subagent shares the parent's token instead of deriving a child token. `request_cancel()` on the child's handle would therefore cancel the parent. Nothing can reach it today (the `task` tool owns the handle), but hierarchical tokens are the right shape.
- A call dropped by the backstop never reports `Finished`, so the TUI left its block `Running` forever; `finish_turn` now closes such blocks. The same would hold for any other consumer of tool events.
- `drop_agent_prompts` drops every approval and `ask_user` modal, because prompts carry no origin. A background subagent's `SubagentGate` prompt dropped that way is read by that agent as a denial.
- Tests that start processes are timing-sensitive on a busy machine. In one parallel run at load above 12 three of the process tests failed at a 5 s bound; they passed alone and in smaller runs. The functional tests now assert outcomes with 30 s upper bounds and never a latency, and the latencies live in the ignored measurement tests. `inbox_poller::tests::agent_initiated_requests_are_answered_so_the_turn_completes`, which the plan's Appendix A already lists as timing-sensitive and which does not touch this code, failed once at load 13 under the default test parallelism and passed 5 of 5 alone.
- Dropping a future closes its socket but keeps nothing of what it had read, so the partial reply of an aborted model call is lost, and so is its token usage.

## 7. Not done, and what it would take

| Item | State in the prototype | What it would take |
|---|---|---|
| Model-call abort that keeps the partial reply and the usage | The call is dropped and its socket closes (3.0 ms at p50 and 4.9 ms at p95 after the Ctrl-C write in the real binary, section 3.4); the transcript gets no assistant message; usage of the aborted call is not counted | A provider contract that streams deltas into the loop's state (P4b: `complete(req, CallCtx { cancel, progress })`), so the loop can commit what arrived. About 2 days once that contract exists |
| Async prompt contract | Synchronous callbacks bridged through `spawn_blocking` | Replace the six sink and callback types by the Session API (P6a "approvals designed fresh"); until then keep the bridge |
| Synchronous file tools | `grep`, `glob`, `ls`, `read_file` never yield; neither the token nor the backstop can interrupt them (a cancel during a `grep` over 30,000 files waited 1354 ms at p50) | Run them through `spawn_blocking` with a token check per directory or chunk. P2g already plans `spawn_blocking` for the headless runtime; the TUI needs it too. About 2-3 days |
| Hierarchical tokens | A foreground child follows the parent's token; a background child has its own | `child_token()`: a token that fires when its parent does and can also be fired alone; about 40 lines without a new dependency |
| Kill policy | SIGKILL of the group, immediately | SIGTERM, a short grace, then SIGKILL. SIGKILL left a stale `.git/index.lock` after a killed `git add` of a 4 GiB file in a check made for this document; SIGINT and SIGTERM did not. The timeout path should kill the group too (P2b) |
| Partial output of a killed command | Discarded with the reader futures | Readers that own their buffers (P2b: head and tail, spill file) |
| Windows | `ProcessTree` is empty on non-Unix; `execute_impl` falls back to `start_kill()` of the shell; not compiled here (only the host target is installed) | A Job Object with kill-on-close, assigned right after spawn (`windows-sys` is already a dependency); the console-event handler for `shutdown()`; P2b and P2d |
| Signals and terminal | SIGINT in `-m` only; the TUI has no handler, so a closed terminal now orphans the running command (section 4) | One idempotent `shutdown()` for SIGINT, SIGTERM, SIGHUP and the console events, plus a panic hook (P2c deliverable 2, P2g). Roughly one signal task per front end and a bounded wait for the groups to die; not estimated by running it |
| Snapshot off the cancel path | Not touched: section 3.6 | The stat-cache snapshot (P2c deliverable 3, spike 4); skip `after_agent` hashing when cancelling |
| Desktop sidecar, ACP | The sidecar already goes through `request_cancel`; ACP aborts the task | Route ACP `session/cancel` through the token so the transcript is balanced; re-record the golden frames |
| The `!cmd` escape, voice capture, selection, other modals | `!cmd` runs with a token that never fires; voice and selection keep their Ctrl-C meaning; only the approval and `ask_user` modals cancel | Decide per modal whether Ctrl-C cancels (the plan says "reachable from every modal") |

## 8. Risks

| Risk | Evidence | Mitigation |
|---|---|---|
| A group of its own orphans the command on a terminal SIGINT | 3 of 3 left in `craftsman -m` without a handler (section 4) | Ship the group (P2b) together with the signal hook, or the other way round; the core commit's hook and test show the size |
| Closing the terminal orphans the command | TUI as a job of a shell: 3 of 3 left on hangup with the spike, 0 of 3 with the unmodified code (section 4) | The SIGHUP and SIGTERM handler of P2c deliverable 2 must land with, or before, the process group; the prototype has none |
| SIGKILL gives the command no chance to clean up | A killed `git add` left `.git/index.lock`; SIGINT and SIGTERM did not | SIGTERM, a short grace, SIGKILL; costs the grace in the worst case |
| Commands that read `/dev/tty` are stopped, not blocked | State `T` (SIGTTIN) in its own group, `S+` in the shared group | Set `GIT_TERMINAL_PROMPT=0` and similar hints (P2b), kill the group on timeout, or use `setsid` so the open fails |
| A descendant that calls `setsid()` survives the group kill | `a_descendant_in_its_own_session_survives_the_group_kill` | Containment (P7a); do not claim more than "the process group" in the exit criteria |
| Cancel waits for the workspace snapshot | 445 ms at 2,000 files, 2.7 s at 20,000 (section 3.6) | Stat cache is a precondition of the latency gate; measure the gate with a large workspace |
| A tool that ignores the token pays the 200 ms grace | 203 ms in process, 241 ms (p50) in the real binary for the subagent without the extension | Shorten the grace once the main tools watch the token; keep it as the backstop |
| Synchronous tools cannot be interrupted | A cancel during `grep` over 30,000 files returned after 1354 ms at p50 and 1400 ms at p95, the run time of the grep (section 3.6) | `spawn_blocking` plus checks (section 7) |
| `-m` approval prompt on a plain stdin line is not cancellable | By reading: `prompt_decision` is a synchronous `read_line` inside the middleware and is the one prompt not routed through `run_blocking`; not run | The first SIGINT only fires the token and the second exits 130 (one press killed the process before); route it through `run_blocking` too |
| Abandoned prompt threads | Each cancelled prompt keeps a blocking-pool thread until the front end drops its reply channel; tokio's `Runtime::drop` waits, without a limit, for blocking closures that are running (the same hazard existed with `block_in_place` on a worker thread) | Release in every front end (done for the TUI, which drops the modals on cancel); async prompts in P6a |
| `drop_agent_prompts` also drops a background subagent's prompt | Reading: prompts carry no origin | Tag prompts with the agent path (P6a) or drop only the foreground turn's |
| Partial reply and usage lost on a model abort | Section 7 | P4b contract |
| `kill(-pgid)` with a pgid of 0 or 1 | `ProcessTree` does not guard it; tokio's `Child::id()` never returns those | Guard `pgid > 1` in production |
| Timing-sensitive tests | Three process tests failed at a 5 s bound in one parallel run at load above 12 | Outcome-only tests with generous bounds (done); latencies only in measurements |
| Windows build unverified | Not compiled; `ProcessTree` is a stub there | The first CI run on windows-2025 (P2d) will say |

## 9. Recommendation for P2c

Size: the spike supports the plan's 6-7 engineer-weeks and gives no ground to lower them. It removes most of the uncertainty about deliverable 1, and it adds about half a week to the expected value (to roughly 6.5-7.5) because two items that the plan's list does not mention turned out to be needed: the synchronous file tools and the kill escalation.

What the numbers say about deliverable 1 (the token through the loop, model call, `ToolCtx`, `Backend::execute`, approvals, `ask_user` and foreground subagents, reachable from every modal): the mechanism is small (about 510 production lines in 17 files, no dependency, one existing test rewritten) and the latency gates are met by a wide margin without the stat cache (p95 39 ms against 300 ms in the real binary under a pty; 0.5 ms at library level). My estimate for turning it into the shipped feature is 2-2.5 engineer-weeks, about a third of the phase: hardening and review of what exists, hierarchical tokens (1 day), ACP and desktop routing with the golden-frame sign-off (3 days), the SIGTERM-then-SIGKILL escalation (1-2 days), the model-abort semantics with the partial reply (2 days, after P4b's contract), the remaining modals and the per-modal Ctrl-C decision (2 days). This is an estimate from the work list above, not a measurement.

What stays outside the spike and decides the rest: deliverable 2 (signals, panic hook, terminal restore; I would put it at 1.5-2 weeks, untouched here), deliverable 3 (stat cache; 1.5-2 weeks, plus 2 if spike 4 exceeds 300 ms) and deliverable 4 (the pty rig and the 50-run gates; 1-1.5 weeks, of which the driver in section 3 is a small start). Added up, 6-8 engineer-weeks (midpoint 7): the plan's 6-7 holds if the stat-cache spike is under its threshold; if it is not, 8-10. I would keep the plan's range and flag the upper end.

Scope changes I recommend:

1. Keep the synchronous approval and `ask_user` callbacks in P2c with the blocking-pool bridge, and leave the async contract to P6a, which redesigns approvals anyway. The bridge costs nothing measurable (< 0.05 ms in the TUI rows) and saves about a week of three-front-end migration.
2. Write the seam with P2b into both phases: the process-tree kill (P2b) and the token (P2c) meet at `execute_impl`, and the group must not ship without the signal routing (section 4). If P2b lands first it must carry the `-m` SIGINT hook, or an interrupted `craftsman -m` leaks 3 processes where it leaks 1 today, and the P2c `shutdown()` (SIGHUP, SIGTERM) must land with it, or a closed terminal leaks the command where today it kills it.
3. Add the synchronous file tools to P2c (or state that an interactive Ctrl-C waits for an in-flight `grep`): P2g plans `spawn_blocking` for the headless runtime only.
4. State in the exit criteria that the five scenarios are measured with a workspace of the size the product will see, because the snapshot dominates the cancel path above about 800 files (section 3.6), and that "no process-group descendant remains" excludes a descendant that left the group.
5. Name the front-end convergence (ACP, desktop) and the SIGTERM-then-SIGKILL policy as deliverables; both are small, neither is in the plan's list.

Confidence: high on feasibility and on the library-level and pty latencies of the token path (N = 20, zero invalid runs, load 2.5-3.7); medium on the work estimates above, which are judgement; low on Windows, which was not built.
