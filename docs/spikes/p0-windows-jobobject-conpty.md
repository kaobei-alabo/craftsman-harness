# P0 spike 2: Windows Job Object and ConPTY hello-world

> **Status:** findings, 2026-10-16. Spike 2 of the four one-day spikes in P0 (`docs/craftsman-harness-plan.md`, section 7.2). The code is a separate cargo project, `spikes/windows-hello/`, run by `.github/workflows/spike-windows.yml` on the throwaway branch `p0/spike-windows`; only this document is meant to be merged.
> **Question (plan, P0 spike 2):** can Craftsman get reliable process-tree kill on Windows (P2b) and a pseudo-console for the P2d ConPTY smoke test from off-the-shelf pieces, or does it need a custom spawn path ("adds 1 week to P2b")?
> **Evidence:** three runs on GitHub-hosted `windows-2025` runners (three different VMs, three regions), 117 to 143 `RESULT` lines each. Everything below is measured unless it says "from the source" or "estimate".

## 1. Answer

**The off-the-shelf pieces are enough on x86_64. Craftsman does not need a custom spawn path, and the plan's "+1 week to P2b" threshold is not triggered.**

- **Tree kill (P2b).** `process-wrap` 10.0.1 with its `JobObject`, `KillOnDrop` and `CreationFlags` shims killed the whole tree in every scenario: `kill()`, dropping the child, and the parent dying without cleanup (`std::process::exit`, a crash, `TerminateProcess`). 0 of 200 grandchildren escaped in each of three runs, also with the CPU saturated. Today's `tokio::process::Command` with `kill_on_drop(true)` leaked every grandchild in 4 of 4 cases in 3 of 3 runs and left the output pipes open, so no partial output could be read after the kill. The integration is 13 lines of safe code; the same job written by hand is 86 lines, 53 of them in `unsafe`.
- **Four traps, each reproduced** (section 5.1): the job only kills on close if the `KillOnDrop` shim is registered; creation flags set directly on the `Command` are silently overwritten; `wait()` returns when the direct child exits and does not wait for the job; calling `try_wait()` several times and then `wait()` can block forever.
- **ConPTY (P2d).** `portable-pty` 0.9.0 hosted Windows PowerShell 5.1, PowerShell 7.6 and `cmd.exe` on the hosted runner, delivered Ctrl-C as a byte, resized, and hosted a raw-mode program whose console mode was restored on exit. One requirement: the host must answer the cursor-position query `ESC[6n`, or the child never starts.
- **32-bit.** `process-wrap` and the raw job code work in a 32-bit process on 64-bit Windows. `portable-pty` does not: its ConPTY setup faults with `0xc0000005` inside `openpty()` on `i686-pc-windows-msvc` (cause isolated in 3.5). Craftsman ships only `x86_64-pc-windows-msvc` today; keep it that way for the ConPTY test.
- **Not covered** (section 8): a session without a window station, Windows on ARM, Windows 10 or 11 client editions, real antivirus (Defender real-time protection is off on the hosted image), and `craftsman` itself.

## 2. What was run

### 2.1 Runs

| Run | Commit | URL | Wall time | Suite step | What it was |
|---|---|---|---|---|---|
| 1 | `a955b1e` | https://github.com/trend-mgai/craftsman-harness/actions/runs/38926424070 | 12 min 41 s | 468.7 s, 117 cases | First complete run (eastus). Its 32-bit step died without a message. |
| 2 | `a2a46b5` | https://github.com/trend-mgai/craftsman-harness/actions/runs/38929480509 | 14 min 9 s | 508.3 s, 141 cases | Added the atomic spawn (c2), the P2b prototype, sweeps and controls (westus3). 32-bit step failed with `0xc0000005`. |
| 3 | `3b13e75` | https://github.com/trend-mgai/craftsman-harness/actions/runs/38933246103 | 13 min 30 s | 522.1 s, 143 cases | Run 2 plus the 32-bit diagnosis and the thread-snapshot measurement (northcentralus). This is the final code. |

The final commit on the branch squashes these three and adds this document; its `spikes/` and `.github/` files are byte-identical to commit `3b13e75` (checked with `git diff`). It was pushed with `[skip ci]`, so it has no run of its own. Runs 2 and 3 are the same suite on different VMs; run 1 is a subset of it. Runs 1 and 2 each exposed a gap that the next run closed (the unexplained 32-bit failure, 3.5). Every case present in more than one run has the same verdict in all of them (checked by script: 0 differences; the 42 encoding cases are byte-identical). Numbers below are written `r1 / r2 / r3` where they differ.

### 2.2 Runner (identical in all three runs)

| Item | Value |
|---|---|
| Image | `windows-2025-vs2026`, image 20261007.264.1; Windows Server 2025 Datacenter 10.0.26100.33438 |
| CPUs | 2 (`available_parallelism`); about 1,312 to 1,366 threads system-wide |
| Session | id 2, window station `WinSta0`, desktop `Default`, user elevated. This is an interactive-style session, not session 0, so the spike brief's premise "no interactive desktop" does not hold for this image |
| Step console | a console without a window, shared by the step shell, the spike and `Runner.Worker.exe` (`GetConsoleProcessList` returned all three); stdin, stdout and stderr are pipes |
| Job | the step runs inside a Job Object with limit flags `0x0` and 9 processes (agent, listener, worker, shell, spike, `vctip.exe` and others) |
| Code pages | OEM 437, ANSI 1252; the step's console 65001; a console created with `CREATE_NO_WINDOW` starts at 437 |
| Shells | Windows PowerShell 5.1.26100.33438, PowerShell 7.6.6, cmd |
| Defender | service on, real-time protection off, behavior monitor off, exclusions `C:\` and `D:\` |
| Toolchain | rustc 1.99.1 (58cf8f9c3 2026-10-13), cargo 1.99.1 (installed with `rustup toolchain install stable --profile minimal --no-self-update`) |

### 2.3 Crate versions (`spikes/windows-hello/Cargo.lock`)

| Crate | Version | Used for |
|---|---|---|
| `process-wrap` | 10.0.1 (features `tokio1`, `job-object`, `creation-flags`, `kill-on-drop`) | Job Object around a tokio command |
| `portable-pty` | 0.9.0 | ConPTY |
| `tokio` | 1.53.3 | async runtime, the status-quo spawn |
| `windows-sys` | 0.61.2 | raw approach and measurement helpers |
| `windows` | 0.62.2 | only because `process-wrap`'s `CreationFlags` takes its `PROCESS_CREATION_FLAGS` |
| `shared_library` 0.1.9, `winapi` 0.3.9, `filedescriptor` 0.8.3 | transitive, used by `portable-pty` | |

### 2.4 Method

- Each experiment is a subcommand run as its own child process by `windows-hello all --isolate`, with a private hidden console (`CREATE_NO_WINDOW`) and inside its own `KILL_ON_JOB_CLOSE` job, so nesting is exercised by construction and a crash or hang in one experiment cannot affect another. Every case prints one line, `RESULT id=<case> verdict=<V> key=value ...`.
- Liveness is checked by pid plus process creation time (immune to pid reuse) with `WaitForSingleObject`, and cross-checked with `tasklist`. A tree-kill case is only scored after the full tree was observed to exist (otherwise it would pass vacuously).
- Race and signal tests have positive controls (a deliberately late job assignment; a Rust program instead of PowerShell) so that "0 escapes" and "still running" can be interpreted.
- ConPTY latencies run to the moment the read that completed the marker returned in the reader thread, starting at the write or at `spawn_command`.
- Size of the code, as non-blank, non-comment lines (tests excluded): see section 2.5.

### 2.5 Lines of code

| Experiment | Code | Lines | Lines mentioning `unsafe` |
|---|---|---|---|
| (a) baseline | `exp_a.rs` (plus shared `scenario.rs` 188, `tree.rs` 87) | 33 | 0 |
| (b) process-wrap | `exp_b.rs`; the integration itself is `spawn_wrapped` | 453; 13 | 0 |
| (c) raw | `exp_c.rs` 377, `raw_job.rs` 160; counted regions below | 537 | 17 |
| (c2) atomic spawn | `exp_c2.rs` | 254 | 5 |
| (d) console events | `exp_d.rs` | 331 | 10 |
| (e) ConPTY | `exp_e.rs` 589, `conpty.rs` 247 (reader thread, cursor-position responder, bounded waits), `conpty_abi.rs` 128 | 964 | 7 |
| (f) encoding | `exp_f.rs` plus `emit-utf8` in `helpers.rs` | 285 | 2 |

The program counts its own production-relevant slices from its source (`c-loc`, `c2-loc`, markers `LOC-BEGIN`/`LOC-END`): `raw_job` 86 code lines, 7 `unsafe` blocks, 53 lines inside `unsafe`; the `NtResumeProcess` alternative 13 lines, 2 blocks, 4 lines; the atomic `CreateProcessW` job part (`raw_job_list`) 62 lines, 2 blocks, 52 lines; the `process-wrap` integration (`spawn_wrapped`) 13 lines, 0 blocks.

## 3. Results

### 3.1 (a) Baseline: today's spawn path

Spawn exactly as `platform_os::shell_command` does (tokio `Command`, `kill_on_drop(true)`, stdin null, stdout and stderr piped, `CREATE_NO_WINDOW`). Two trees: `cmd /c "start /b ping -n 60 127.0.0.1 > nul & ping -n 60 127.0.0.1 > nul"` (the tree from the spike brief) and `powershell.exe -NoLogo -NoProfile -NonInteractive -Command "ping -n 60 127.0.0.1"` (Craftsman's real command line). The case waits until the pings exist, then kills or drops, then looks 3 s later. Identical verdicts in all three runs.

| Case | Tree | Action | Grandchildren alive after 3 s | Pipes at EOF within 4 s | `tasklist` ping.exe |
|---|---|---|---|---|---|
| a1 | cmd | `child.kill().await` | 2 of 2 | no | 2 |
| a2 | cmd | drop (`kill_on_drop`) | 2 of 2 | no | 2 |
| a3 | PowerShell | `child.kill().await` | 1 of 1 | no | 1 |
| a4 | PowerShell | drop | 1 of 1 | no | 1 |

The direct child dies at once (`kill()` returned in 1.2 to 1.5 ms for cmd, 7.7 to 8.4 ms for PowerShell). The survivors inherited the pipe handles, so the pipes stay open as long as they live: after a timeout the current code cannot read partial output (it drops its read futures today; any attempt to collect the rest would block). This matches Appendix A row A1.

### 3.2 (b) `process-wrap` JobObject

The integration, as tested (`spawn_wrapped` in `exp_b.rs`; the spike's version has a switch that omits `KillOnDrop` for the negative control):

```rust
let mut cmd = CommandWrap::with_new(tree.program(), |c| {
    c.args(tree.args())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
});
cmd.wrap(CreationFlags(CREATE_NO_WINDOW)).wrap(JobObject);
cmd.wrap(KillOnDrop);              // required, see 5.1
let child: Box<dyn ChildWrapper> = cmd.spawn()?;
```

**Kill and drop.** Same two trees as (a), with the shims above.

| Case | Action | Descendants left | Time to return | All descendants dead within | Pipes at EOF |
|---|---|---|---|---|---|
| b1 cmd | `kill()` | 0 of 2 | 4.6 / 2.8 / 2.8 ms | 5.1 / 5.4 / 5.1 ms | 4.4 / 2.8 / 5.8 ms |
| b2 cmd | drop | 0 of 2 | 0.2 / 0.3 / 0.3 ms | 5.1 / 5.6 / 5.6 ms | 3.6 / 2.4 / 2.4 ms |
| b1 PowerShell | `kill()` | 0 of 1 | 9.7 / 10.2 / 8.8 ms | 0 / 0 / 0 ms | 3.0 / 3.9 / 3.5 ms |
| b2 PowerShell | drop | 0 of 1 | 0.3 / 0.5 / 0.3 ms | 15.8 / 21.2 / 10.7 ms | 5.8 / 18.9 / 3.0 ms |

("Dead within" has the 5 ms granularity of the poll.) Without the `KillOnDrop` shim (`b2n`): dropping leaked both grandchildren and even the direct child (`root_alive=true`); `kill()` without it (`b2k`) still killed the tree.

**Parent dies without cleanup** (`b3`): a helper process builds the tree under the job, announces it, waits 1.5 s so the orchestrator can confirm the tree is alive, then dies. Only the kernel can close the job handle.

| Parent death | Tree | Result | All dead within |
|---|---|---|---|
| `std::process::exit(0)` | cmd (3 processes) | killed | 0.0 / 5.6 / 5.1 ms |
| `std::process::abort()` (exit `0xc0000409`) | cmd | killed | 5.6 / 5.1 / 5.3 ms |
| `TerminateProcess` by the orchestrator | PowerShell (2 processes) | killed | 10.4 / 10.2 / 10.8 ms |
| control: no job (`kill`) | cmd | 3 of 3 alive after 8 s | not killed |

**Race: grandchild spawned before the job assignment** (`b4`). `process-wrap` starts the child with `CREATE_SUSPENDED`, assigns it to the job, then resumes it (from the source: `pre_spawn` adds the flag, `wrap_child` assigns, then `resume_threads` resumes every thread of the pid found with a Toolhelp thread snapshot). The stress loop spawns a program that starts a grandchild as its very first action, waits until the grandchild exists, kills the job and checks the grandchild: 200 iterations.

| | Escapes | Spawn p50 / p95 / max | `kill()` p50 / p95 |
|---|---|---|---|
| idle machine | 0 of 200 in each run | 10.3 / 11.7 / 10.7 ms; 14.7 / 13.0 / 20.2 ms; 16.6 / 51.0 / 68.6 ms | 2.1 / 2.2 / 2.0 ms; 3.7 / 3.5 / 5.1 ms |
| CPU saturated (4 busy threads on 2 vCPUs) | 0 of 200 in each run | 19.7 / 15.3 / 16.5 ms; 24.2 / 16.9 / 19.4 ms; 265.7 / 97.0 / 86.2 ms | 100.6 / 99.2 / 100.6 ms; 779 / 172 / 174 ms |

A plain `std::process::Command::spawn` of the same child takes 1.3 ms p50 (r2, r3; 2.0 in r1), so the wrapper adds about 9 to 10 ms per spawn. The cause is measured in 3.3.

**Nesting.** Experiment b ran inside the driver's `KILL_ON_JOB_CLOSE` job, itself inside the runner's job (flags `0x0`); the child job was created and assigned without error in every case. `cargo run` placed the process in cargo's own job (`0x2000`, two processes: `cargo.exe` and the spike) and all cases passed there too (`b-smoke`, the cargo-run step of every run). Cargo creates that job for what it runs, so `cargo test` binaries should be in the same situation; I did not run a Job Object test under `cargo test` itself.

**P2b contract prototype** (`b-p2b-prototype-*`): `Write-Output 'partial-output'; ping -n 60 127.0.0.1` with a 6 s limit, then kill, then read what was captured.

| | Timed out | Partial output returned | Pipes closed after kill | Survivors |
|---|---|---|---|---|
| `process-wrap` JobObject | yes (6.0 s) | yes (`partial-output` plus the ping replies printed before the kill) | 10 to 23 ms | 0 |
| plain tokio (status quo) | yes | no | not within 5 s | 1 |

### 3.3 (c) Raw approach and (c2) atomic spawn

(c) `std::process::Command` plus `windows-sys`: create the job with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, spawn with `CREATE_SUSPENDED`, `AssignProcessToJobObject`, resume. **86 code lines, 7 `unsafe` blocks, 53 lines inside `unsafe`.** Results match (b): `TerminateJobObject`, closing the handle, and all three abrupt parent deaths killed the tree (c1, c2, c3: all dead at the first poll or within 10.6 ms); 0 of 200 escaped idle and 0 of 200 saturated in each run.

What `std` does not expose: the handle of the suspended main thread. `std::process::Child` has `id()` and `as_raw_handle()` (process only). `std::os::windows::process::ChildExt::main_thread_handle` is unstable on rustc 1.99.0 (error E0658, feature `windows_process_extensions_main_thread_handle`, rust-lang/rust#96723). So the thread is found again with a system-wide Toolhelp thread snapshot (what `process-wrap` does), or the whole process is resumed with `ntdll!NtResumeProcess`, which `windows-sys` does not declare (13 lines, 2 blocks, 4 unsafe lines for a hand-written declaration). I also found no stable `std` way to pass a process attribute list: a `CommandExt::raw_attribute` method does not resolve on stable 1.99.0 (E0599).

| Resume path | Spawn p50 / p95 (idle) |
|---|---|
| Toolhelp thread snapshot (raw) | 10.3 / 8.5 / 8.7 ms; 12.4 / 10.0 / 10.8 ms |
| `NtResumeProcess` | 2.3 / 1.2 / 1.3 ms; 3.3 / 1.4 / 1.6 ms |
| plain spawn, no job | 2.0 / 1.3 / 1.3 ms |

The snapshot walks every thread in the system and costs 7.0 to 7.2 ms at p50 on this runner (1,312 to 1,366 threads; `env-thread-snapshot`, p95 7.3 to 7.9 ms). That accounts for nearly all of the difference above. It grows with the number of threads on the machine (a developer machine will have more; not measured).

**Race controls** (raw path, child started without `CREATE_SUSPENDED`, assignment delayed on purpose by `delay`). They show that the harness can see an escape and how wide the window is. Escapes / iterations, r1 / r2 / r3:

| Delay between `CreateProcess` and assignment | Escapes |
|---|---|
| 0 ms (natural), idle | 0/100 in all runs |
| 0 ms, CPU saturated | 0/100 in all runs |
| 1, 2, 5 ms | 0/40 in all runs |
| 8, 10 ms | not run / 0/40 / 0/40 |
| 15 ms | not run / 18/40 / 22/40 |
| 20 ms | 25/40 / 39/40 / 40/40 |

I could not provoke the race without an artificial delay of 15 ms or more. With `CREATE_SUSPENDED` (toolhelp or ntdll resume) there were no escapes in 200 + 200 saturated + 100 spawns per run, which is what the construction promises: the child cannot run before it is in the job.

**(c2) Atomic spawn with `PROC_THREAD_ATTRIBUTE_JOB_LIST`** (`CreateProcessW` creates the process inside the job; no suspended start, no resume). **62 code lines, 2 `unsafe` blocks, 52 lines inside `unsafe`, job part only.** Kill, close and the 200-spawn loop: tree killed in all cases (dead within 5.1 to 10.8 ms), 0 of 200 escaped in runs 2 and 3, spawn p50 1.3 / 1.1 ms (the same as a plain spawn). It also worked nested in the runner's and the driver's jobs. The "pipes at EOF: none" in those lines only means this minimal path has no pipes. A real custom spawn path would still need stdio pipes usable from tokio, argument quoting, environment and cwd handling, and a `Child` with `wait` and `kill`; none of that was built or measured here.

### 3.4 (d) Console control events

All run in a private hidden console created by the driver (`CREATE_NO_WINDOW`): `CTRL_C_EVENT` cannot be aimed at one process group, and the real step console is shared with `Runner.Worker.exe`, so a `CTRL_C_EVENT` to group 0 from a step goes to every process on that console, including the runner's worker and the step shell (all three were in the console's process list). The tests refuse to send it unless they are alone on their console.

| Case | Setup | Result (r1 / r2 / r3 where they differ) |
|---|---|---|
| d1 | PowerShell child, `CREATE_NEW_PROCESS_GROUP`, `CTRL_BREAK_EVENT` to the group | call succeeds, **PowerShell still running after 10 s** in all runs |
| d1b | control: a Rust program, same flags and event (runs 2 and 3) | exits in 5.1 / 5.3 ms with `0xc000013a` (default handler). The event is delivered; PowerShell (`-NonInteractive -Command`) ignores Ctrl-Break |
| d2 | PowerShell child with `CREATE_NEW_PROCESS_GROUP`, `CTRL_C_EVENT` group 0 | still running after 5 s: the flag disables Ctrl-C for the group |
| d3 | PowerShell child, shared console, `CTRL_C_EVENT` group 0 | exits with code 1 after 162 / 534 / 52 ms |
| d3b | control: Rust program, same (runs 2 and 3) | exits in 5.1 ms with `0xc000013a` |
| d4 | Craftsman's flags (`CREATE_NO_WINDOW`, plus new process group), `CTRL_BREAK_EVENT` | `GenerateConsoleCtrlEvent` returns success, **child unaffected** (it has its own console) |
| d5 | same child, `FreeConsole` + `AttachConsole(child)` + `CTRL_C_EVENT` | child exits with code 1 (already gone at the first poll after 150 ms) |
| d6 | no console at all | `GenerateConsoleCtrlEvent` fails with error 6 (invalid handle) |

Consequence: a graceful interrupt of Craftsman's shell child is not available with today's flags. `CTRL_BREAK` does nothing to PowerShell even where it is delivered, and `CTRL_C` needs the `AttachConsole` dance, which detaches the calling process from its own console. That would take away a raw-mode TUI's console handles (not tested), so it would have to run in a small helper process. The plan's cancel path is "kill the tree" (section 5.3), which the job provides without any of this.

### 3.5 (e) ConPTY via `portable-pty` 0.9.0

**Availability.** Works on the hosted runner: `openpty` plus `spawn_command` of `cmd.exe /c echo` produced the marker 27 / 51 / 32 ms after spawn (130 bytes). It also works from the non-isolated step console that `Runner.Worker.exe` shares (34 / 33 ms, exit code 0). The initial stream is `ESC[6n ESC[?9001h ESC[?1004h ESC[m ESC]0;<exe> BEL ESC[?25h ESC[?25l`.

**The host must answer the cursor-position query.** `portable-pty` passes `PSEUDOCONSOLE_INHERIT_CURSOR` (from its source). With no reply, `cmd.exe` produced 4 bytes (`ESC[6n`) and no prompt in 8 s in all three runs (`e-cpr-no-reply`, `BLOCKS_UNTIL_REPLY`); after the reply the prompt came in 16 / 16 / 212 ms. The test rig (P2c lists `portable-pty` plus `vt100`) must answer `ESC[6n` with `ESC[<row>;<col>R`. I did not test whether the `vt100` crate does.

| Measurement | Windows PowerShell 5.1 | PowerShell 7.6 | cmd |
|---|---|---|---|
| spawn to first byte, p50 | 18 / 14 / 13 ms | 26 / 17 / 15 ms | 26 / 13 / 11 ms |
| spawn to custom prompt, p50 (5 sessions) | 416 / 255 / 247 ms | 820 / 411 / 389 ms | 46 / 25 / 25 ms |
| first command in a fresh session | 1,402 / 436 ms (r2 / r3) | 1,263 / 127 ms (r2 / r3; r1 max 5,098) | 8 to 9 ms |
| later command round trips | 15 to 16 ms | 16 to 474 ms | 8 to 23 ms |
| Ctrl-C byte `0x03` during a long command, warm session, p50 (6 samples) | 3 / 3 / 2 ms | 3 / 3 / 2 ms | 27 / 10 / 15 ms |
| same, worst sample | 8 / 6 / 5 ms | 2,291 / 388 / 20 ms | 28 / 17 / 18 ms |
| same, first interrupt of 5 fresh sessions (r2 / r3) | 5 / 5 to 7 ms | 5 to 13 / 12 ms | 9 to 11 / 9 to 24 ms |
| shell usable after the interrupt | 6 of 6 | 6 of 6 | 6 of 6 |
| resize 100x30 to 132x40, child reports the new size | on the first query after the call (floor of the measurement 100 ms): 101 / 120 / 101 ms | 101 ms | 101 ms |
| `exit`, then drop the pseudo-console | 0 to 1 ms, never hung | 0 to 1 ms | 0 ms |

Many values cluster at 16 ms, which is consistent with one render frame of the pseudo-console (the cause was not verified). The slow samples are the first command or first interrupt of a PowerShell session (the PowerShell 7 outlier of 2.3 s was not reproduced in 10 fresh sessions); budget a few seconds for the first command in a CI test.

**VT sequences.** The shells emitted 14 to 16 distinct sequences: SGR (`CSI #m`, `CSI m`; about 200 to 320 per PowerShell session; cmd emitted only one reset), `CSI K`, cursor positioning (`CSI #;#H`, `CSI H`, `CSI #C`), `CSI ?25h/l`, `OSC 0` (title, repeated per prompt in cmd), `CSI ?9001h` (win32-input-mode), `CSI ?1004h` (focus events), `CSI 6n`, and after a resize `ESC[8;40;132t`; PowerShell 7 also used `CSI #X`. No `CSI ?2004h` (bracketed paste) and no `?1049h` (alternate screen) from any shell; `?1049h/l` passed through from the test program. The first examples are logged as `vt-examples-*` lines.

**Raw-mode program** (`tui-probe`, a stand-in for `craftsman`; the P2d exit criterion asks for type, stream, interrupt, restore): ready 33 / 20 / 20 ms after spawn; `a`, `0x03`, `ESC[A` and CR each arrived as exactly those bytes within 9 to 22 ms; the process stayed alive after `0x03` (raw mode delivers Ctrl-C as data); console modes before and after exit were equal (`0x1f7` input, `0x7` output); the alternate screen was entered and left. Contrast: the same byte to a program left in cooked mode ended it with `0xc000013a` within 5 ms (conhost turned `0x03` into `CTRL_C_EVENT`).

**Teardown** (cmd with `ping -n 60` running):

| Event | Result |
|---|---|
| host calls `child.kill()` (`portable-pty`: `TerminateProcess` on the direct child) | `ping.exe` survives, 1 of 2 left, in all runs |
| host drops the master (`ClosePseudoConsole`) | cmd and ping dead, 0 of 2 left, within 1 ms |
| host process exits, aborts or is terminated without cleanup (`e-abrupt-host-*`, runs 2 and 3) | cmd and ping dead within 5.1 to 5.6 ms in all three cases |

A pseudo-console therefore takes its attached processes down by itself when it is closed or its host dies. A process that detaches from the console would not be covered.

**32-bit.** Run 1: the 32-bit smoke step ended with exit code 1 and no output. Run 2 (with breadcrumbs and a panic hook): `0xc0000005` before `PtySession::spawn` (the `portable-pty` setup) returned. Run 3 isolated it:

| Probe (32-bit process on 64-bit Windows) | Result |
|---|---|
| `CreatePseudoConsole`, `ResizePseudoConsole`, `ClosePseudoConsole` through `windows-sys` (correct `extern "system"`) | HRESULT `0x0` and `0x0`: kernel32's API works under WoW64 |
| the same calls through `extern "C"` pointers from `GetProcAddress` | create returned `0x0`, resize returned a nonsense HRESULT (`0xafedd0`), then the process died with `0xc0000005` |
| `portable-pty` `openpty` | last breadcrumb before the crash: `pty: openpty (CreatePseudoConsole via portable-pty)`; `0xc0000005` |
| the same two probes in a 64-bit process | all fine (the `extern "C"` pointer call survives: one calling convention on x64) |

From the source: `portable-pty` loads the three functions with `shared_library!`, whose generated function pointers are `unsafe extern fn` (the C convention, `shared_library-0.1.9/src/lib.rs`), while kernel32 exports them `__stdcall` on x86. I did not find which instruction faults inside `openpty`; the pieces above (correct binding works, wrong convention reproduces the symptom) are the evidence. `process-wrap` and the raw job code pass every WoW64 case (`b1`, `b2`, abrupt death, the P2b prototype, `cs4` stress with 50 spawns, and the 20 ms race control).

### 3.6 (f) Output encoding

Command: `[Console]::OutputEncoding.WebName; Write-Output ('<e-acute><CJK middle>')` (U+00E9 U+4E2D), with `-NoLogo -NoProfile -NonInteractive -Command`. Bytes were identical in all three runs.

| Setup | Windows PowerShell 5.1 | PowerShell 7.6 |
|---|---|---|
| **Pipe, Craftsman's flags** (`CREATE_NO_WINDOW`), default: `WebName` | `IBM437` | `ibm437` |
| same, bytes of the text | `82 3f` (e-acute as CP437; the CJK character is lost as `?`); `String::from_utf8_lossy` gives U+FFFD | same |
| same, prefixed with `[Console]::OutputEncoding = [Text.Encoding]::UTF8;` | `utf-8`, `c3 a9 e4 b8 ad`, **no BOM** | same |
| same, prefixed with `... = New-Object System.Text.UTF8Encoding $false;` | `c3 a9 e4 b8 ad`, no BOM | same |
| **Pipe**, a native program writing raw UTF-8 (`& exe`, and `& exe 2>&1`), default | unchanged `c3 a9 e4 b8 ad` | same |
| **ConPTY**, cmdlet text, default | UTF-8 `c3 a9 e4 b8 ad` (conhost converts) | same |
| ConPTY, native program writing raw UTF-8, default | mojibake: the bytes are read as CP437 (`e2 94 9c e2 8c 90 ce a3 e2 95 95 c2 a1`) | same |
| ConPTY, native program, with the UTF-8 prefix | correct | correct |
| Pipe, console **inherited** from the step (code page 65001), default | `utf-8`, correct without any fix | same |

So the P2b fix works as planned: put the assignment at the front of the same `-Command` string, before anything is printed, because the default loses the CJK character irrecoverably. In the pipe case it matters only for text PowerShell itself produces; native programs' bytes pass through unchanged (either untouched or round-tripped through CP437, which maps every byte). The prefix also fixed native output under ConPTY, which is consistent with the property setter changing the console's output code page as well (I did not read the page back). The result depends on the console the child gets: Craftsman's `CREATE_NO_WINDOW` child always starts at the OEM page (437 here), whatever the user's terminal uses, whereas a child that inherits the CI step's console starts at 65001; a test that passes in a CI step does not show the bug. Not measured: stderr, and non-ASCII input beyond the command line (which reached PowerShell intact as UTF-16).

## 4. Variance between runs

- **Verdicts:** none differ (143 distinct cases across three runs; 26 exist only in later runs because they were added).
- **Stable numbers:** kill and drop latencies (within about 2x), 0 escapes, byte-level encoding results, the ConPTY CPR requirement, close and teardown times, console-event outcomes.
- **Moves with the VM or the moment:** PowerShell start-up (416 / 255 / 247 ms and 820 / 411 / 389 ms to the custom prompt), first-command latency (127 ms to 5.1 s for PowerShell 7), PowerShell's own response to Ctrl-C on a shared console (162 / 534 / 52 ms), and the race window (20 ms delay: 25/40, 39/40, 40/40 escapes; 15 ms: 18/40, 22/40). Medians of 5 to 6 samples are given where there are 5 to 6 samples; "p95" over so few samples equals the maximum.
- Each run was on a different worker in a different region (eastus, westus3, northcentralus), same image.

## 5. Recommendation

### 5.1 P2b: use `process-wrap`, behind one small type

Adopt `process-wrap` (pin the exact version, `=10.0.1`) with features `tokio1`, `job-object`, `creation-flags`, `kill-on-drop`, and hide it behind a single `ProcessTree`-style type in `platform_os.rs` that enforces these rules; each rule is a reproduced trap, and the `b` series in the spike is the regression test for them:

1. **Register all three shims** (`CreationFlags`, `JobObject`, `KillOnDrop`) and never call `kill_on_drop` or `creation_flags` on the `Command`. Without `KillOnDrop` no kill-on-close flag is set and dropping leaks everything, even the direct child (`b2n`). `JobObject`'s `pre_spawn` overwrites the creation flags, so `CREATE_NO_WINDOW` set on the command was lost and the child landed on the parent's console (`b-flags-direct-on-command`); through the shim it is kept (`b-flags-shim`).
2. **On timeout or cancel call `kill()` (it terminates the job), then read the pipes with a short deadline.** The pipes reach EOF 2 to 19 ms later and the partial output is intact (`b-p2b-prototype-jobobject`).
3. **Do not rely on `wait()` to wait for the job.** The type's doc says it "waits on all processes within the job"; it returned after 17 to 30 ms while a 20 s orphan was still running (`b-wait-orphan`), the same as a plain tokio child.
4. **Do not poll `try_wait()` and then call `wait()`.** After 1 to 3 `try_wait()` calls on an exited child `wait()` returned at once; after 5, 10 and 30 it did not return within 3 to 5 s (`b-trywait-n*`; 4 calls were not tried). From the source: each `try_wait()` removes one packet from the job's completion port, and `wait()` then blocks in a `spawn_blocking` task on an empty port with no timeout. `kill()` after such polling still returned immediately. Use `wait()` alone under `select!` or a timeout, and never call it after a successful `try_wait()`.

From the source, not measured: `JobObjectChild::into_inner` leaves the job handle open; fine for a registry that holds the whole wrapper (P2f), a leak otherwise.

Cost: about 9 to 10 ms more per spawn than a plain spawn, from the Toolhelp thread snapshot (3.3), small next to a PowerShell start (about 200 to 900 ms measured here). If that ever matters, the `NtResumeProcess` path costs nothing measurable (1.2 ms) but needs a hand-written FFI declaration.

Estimate (judgment, not measured): the Windows tree-kill work item is 1.5 to 2 engineer-days inside P2b's three weeks (13-line integration, the four rules as tests, wiring the timeout), so **P2b keeps its 3 weeks**.

### 5.2 P2d: `portable-pty` is enough for the ConPTY smoke test, on x86_64

Use `portable-pty` 0.9.0 for the Windows PTY rig. Build the rig with these rules, each measured here:

- Answer `ESC[6n` immediately (section 3.5). `conpty.rs` (247 lines, no `unsafe`) is a working reference: reader thread, timestamps, bounded `wait_for`, responder, close with hang detection.
- Close the pseudo-console to clean up; do not rely on `child.kill()` (it kills only the direct child).
- Never send `CTRL_C_EVENT` or `CTRL_BREAK_EVENT` to group 0 from a CI test: the step console is shared with `Runner.Worker.exe`. Drive interrupts through the pseudo-console (a `0x03` byte).
- Budget seconds for the first command in a fresh PowerShell (0.1 to 1.4 s here, 5.1 s once) and expect output latencies around 16 ms.
- Run the test as `x86_64`. Do not build or test the ConPTY path as `i686`.
- Expect the CI environment to differ from users': the step console has code page 65001, a Craftsman child console 437.

Estimate (judgment): the Windows-specific part of the ConPTY smoke test is about one engineer-day on top of the P2c rig. The rest of P2d's size is unaffected by this spike.

### 5.3 Plan impact

- P0 spike 2 threshold ("a custom spawn path adds 1 week to P2b"): **not triggered.**
- A custom spawn path, if it were ever needed, is shown by (c2): the job part is 62 lines; the missing parts (async stdio pipes, argument quoting, environment, a `Child` with `wait` and `kill`) are unmeasured. My estimate is about the week the plan assumes, so there is no case for building it now.

## 6. What would change the answer

| If | Then |
|---|---|
| Craftsman must send a graceful interrupt (Ctrl-C or Ctrl-Break) to the shell child before killing it | not a spawn-path problem but a design change: Ctrl-Break does nothing to PowerShell, and Ctrl-C needs the child on a console shared with the sender or a helper process that attaches to the child's console (d4, d5). `process-wrap` neither helps nor hurts. |
| A 32-bit Windows target must host ConPTY | `portable-pty` 0.9.0 cannot be used as is (3.5). Fix its calling convention upstream or in a fork, or call `CreatePseudoConsole` directly (the `windows-sys` binding worked in the same process). |
| Another security product blocks or flags a process created suspended and resumed from a thread snapshot (real-time protection was off here) | switch to the atomic `PROC_THREAD_ATTRIBUTE_JOB_LIST` spawn (c2: no suspended start, 1.1 to 1.3 ms spawn), which does need the custom `CreateProcessW` path. Cheapest check: run this spike's `b` and `c` experiments on a machine with the product. |
| A target where `AssignProcessToJobObject` fails (an enclosing job that forbids nesting, Windows older than 8, some container setups) | fall back to a Toolhelp parent-pid walk at timeout. Not observed: three nested levels worked here. |
| `process-wrap` changes how `wait`/`try_wait` use the completion port, or a field bug appears | re-run the `b` series (about 2.5 minutes) on the new version before adopting it; it is why the version should be pinned. |
| P2f background jobs must outlive the call that started them | design the registry to own the whole `Box<dyn ChildWrapper>`; do not use `into_inner` (leaks the job handle). |
| The plan moves the shell tool itself onto ConPTY (persistent sessions) | the teardown results above make that safe for tree cleanup (closing the console or host death kills the tree), but `child.kill()` alone is not enough, and the CPR responder becomes production code. |

## 7. Risks

| Risk | Evidence | Mitigation |
|---|---|---|
| **`CREATE_SUSPENDED` race** (a fast child spawns before it is in the job) | closed by construction; 0 of 200 idle, 0 of 200 saturated, per run; a positive control shows the harness detects escapes (15 ms delay: 18/40 and 22/40; 20 ms: 25/40 to 40/40); no escape was ever provoked at 10 ms or less | keep `CREATE_SUSPENDED`; rule 1 of 5.1 |
| **Job Object nesting on hosted runners** | the runner puts the step in a job (flags `0x0`); nesting under it and under cargo's `KILL_ON_JOB_CLOSE` job worked in every case | none needed; keep the b-smoke step under `cargo run` in the Windows job |
| **ConPTY availability without a desktop** | works in `WinSta0` session 2; this image does not offer a session without a window station | see section 8 |
| **ConPTY startup handshake** | child never starts without the `ESC[6n` reply | responder in the rig |
| **32-bit vs 64-bit** | `portable-pty` crashes on i686 (3.5); all Job Object code works in WoW64 | x86_64 only for ConPTY |
| **Antivirus** | not measured: Defender real-time protection and behavior monitor are off, `C:\` and `D:\` excluded; spawn numbers here are best case | see section 6, row 3 |
| **Runner console is shared with the worker** | `GetConsoleProcessList` listed `Runner.Worker.exe` | rule in 5.2 |
| **Encoding differs by console** | a CI step inherits 65001; Craftsman's child starts at 437 | test with `CREATE_NO_WINDOW`, apply the UTF-8 prefix |
| **Thread-snapshot cost grows with the machine** | 7 ms at 1.3k threads | acceptable; `NtResumeProcess` if needed |
| **First-command and PowerShell 7 latency spikes** | the first command in a fresh PowerShell session took 0.1 to 5.1 s | generous timeouts in CI tests |

## 8. Not measured

- A service session or any session without a window station (`WinSta0`); the hosted image does not provide one. ConPTY needs none by design, but this spike cannot show it.
- Windows on ARM, Windows 10 or 11 client editions, older server builds, Windows containers.
- Antivirus or EDR beyond Defender with real-time protection off.
- `craftsman` itself in a pseudo-console against the mock server (P2d's exit criterion); the stand-in is the `tui-probe` raw-mode program.
- Whether `vt100` answers `ESC[6n`; PowerShell-specific bracketed-paste behavior (no `?2004h` was emitted by either shell).
- stderr encoding; Git Bash and WSL shells (excluded by decision 3).
- More than one concurrent job tree, jobs with resource limits, long-running stability.
- A hand-written full spawn path (only its job part was built).

## 9. Reproduce

On Windows with Rust stable:

```
cd spikes/windows-hello
cargo build --release --locked
./target/release/windows-hello.exe all --isolate     # every experiment, 8 to 9 minutes
./target/release/windows-hello.exe b                 # or: a, c, c2, d, e, f
cargo run --release --locked -- b-smoke              # inside cargo's own job
./target/release/windows-hello.exe e0-stdcall        # 32-bit probes: e0-stdcall, e0-cabi, e0
```

The OS-independent helpers (VT summarizer, unsafe-line counter, percentile code) have unit tests that also run on other hosts: `cargo test --locked`. The workflow runs the same commands, plus a 32-bit build (`i686-pc-windows-msvc`, step marked informational). Raw logs of the three runs are retained by GitHub Actions for the repository's retention period; the URLs are in 2.1.

## 10. Suggested edits to the plan (for the coordinator; this document does not change it)

- Section 7.2, P0 spike 2: record "threshold not triggered; `process-wrap` Job Object and `portable-pty` ConPTY are sufficient on x86_64; P2b stays at 3 weeks."
- P2b deliverables: add the four `process-wrap` rules of 5.1 as acceptance tests, and the UTF-8 prefix for PowerShell (section 3.6) with the note that it must precede all output.
- P2d: add the cursor-position responder, the no-i686 rule, and "no console control events to group 0 in CI".
- Appendix A: add rows for the Windows facts (baseline leak and open pipes, a step shares its console with `Runner.Worker.exe`, the encoding table).
- Risks table, "Windows process handling with the async runtime": the mitigation named there ("Job Object owned by the foreground call and killed on drop") is confirmed; add the `wait`/`try_wait` trap.
