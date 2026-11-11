# Trace schema v1

> **Identity migration (November 4, 2026):** names/paths in this document use
> current Craftsman identifiers. Historical dates, hashes, counts, accepted
> semantics and failure outcomes remain original evidence, not a new run or
> proof of release. The schema/ACP/WIT namespace change is breaking; see
> [repository-name.md](repository-name.md).

> **Status:** P0 deliverable, for review · schema id `craftsman-harness/trace@1` · adopted unchanged by P2a · plan reference: sections 5.5, 5.9 and 7.2 of `docs/craftsman-harness-plan.md`
> **Files:** this document; `docs/schemas/trace-v1.schema.json` (JSON Schema draft 2020-12, validates one line); `docs/schemas/check_trace.py` (validates a whole file: the schema plus the file-level rules of 5.6); `docs/schemas/trace-v1.vectors.json` (test vectors for 6.2 and 6.8 that every writer must reproduce)
> **Writers:** the instrumented baseline R0 (`baseline-0.4.0-instrumented`, driven with `craftsman -m`) and, from P2a, `craftsman exec`.
> **What the file is for:** it records what the harness can observe about one run. One collector derives from it the metrics that need only that observation (section 8), whichever build wrote it. Some inputs are not in it and come from the runner: the grader's verdict (pass rate), the identity of the task and the repeat, the task snapshot and reference solution (wrong-place edits), and prices (dollars). Section 8.0 lists them and says how a trace is joined to them; section 8.8 says which metric each build can supply.

## 1. What the trace is

A trace is the measurement record of one run: which model calls and tool calls happened, how long they took, what they cost in tokens, and what they did to the files. It is not a transcript and not the event log of section 5.4. It holds no prompt, no model text, no tool argument, no tool output and no file content: only names, counts, times, line numbers and SHA-256 hashes. That keeps it small, safe to store next to scoreboard artifacts, and identical in shape for every build.

## 2. File format

- JSON Lines, UTF-8 without a byte order mark. One JSON object per line, ended by `\n`, no blank lines. The order of keys inside a line carries no meaning.
- One file per run. The writer creates the file when it writes the first line. If a file already exists at the path it is replaced only when it is empty or begins with a line of a previous trace (`{"schema":"craftsman-harness/trace@1"`); any other existing file is left alone and the run is not traced.
- Each line is handed to the file as one write, in order, so a run that is killed at any moment leaves a readable prefix.
- Writing is best effort and never makes the run wait. A writer must not open the file in a way that can block (a FIFO, a device and a directory are refused, and so is a path whose last component is a symbolic link), and must not let a slow or stuck file hold up the run. A failure to open or to write disables the trace for the rest of the run and never changes the run's output, exit code or behavior. A run whose trace was disabled has a file that ends early, or none.
- A path that lies inside the working directory is not used: the model could list the file, and the run would differ from an untraced one. Point the writer at a file outside the directory the agent works in.
- Lines are in `seq` order. Consumers order by `seq`; `ts` is informational because wall clocks can step.
- A file with no `run_end` line is incomplete: the process crashed or was killed, or the trace was disabled. The collector records such a run as crashed.
- The trace holds no tool arguments, tool output, model text or file content, only hashes, counts, times and a few short names, so it is far less sensitive than a transcript. The free text it does hold comes from the model or the repository: the model id, tool names, `path` and the exit reason. P2a's redactor covers the trace like every other output channel. A path outside the working directory is written as an absolute path and can contain a user name; scoreboard runs mount only the task directory, and a local trace should be treated as private.

## 3. Evolution

Evolution is additive only. Within `craftsman-harness/trace@1` a later revision may add optional fields to a kind, add new kinds, and add new values to the open strings (`entry`, `harness_profile`, `exit_reason`). A consumer must ignore fields and kinds it does not know, and must treat an unknown value of any enumerated field (`role`, `agent`, `status`, `usage_source`, `tree_hash_skipped`) as unknown rather than as an error. Removing or renaming a field, changing a type, a unit or a hashing rule, or making an optional field required is a new schema id (`craftsman-harness/trace@2`), and the collector reads both.

The JSON Schema is the writer-side contract, and it is closed where this document is closed: it lists the enumerations, so a writer's tests fail on a value this document does not define. A revision that adds an enumeration value therefore also updates the schema file. A collector does not validate with the schema to decide whether to read a line; it reads tolerantly as described above and may use the schema for diagnostics. No object in the schema sets `additionalProperties` to false, and a line of an unknown kind passes the envelope check.

## 4. Envelope

Every line has these fields.

| Field | Type | Meaning |
|---|---|---|
| `schema` | string | Always `craftsman-harness/trace@1`. |
| `seq` | integer | Position of the line in the file, counting from 0 with no gaps. |
| `ts` | string | RFC 3339 UTC with exactly three fractional digits, such as `2026-10-16T09:14:03.120Z`; the wall clock when the event completed, truncated (not rounded) to milliseconds. |
| `run_id` | string | Opaque id of the run, 1 to 128 characters from `A-Za-z0-9._-`. The same on every line of a file. |
| `thread_id` | string or null | The conversation thread of the run. Null when the run has none, for example under `--ephemeral`. The same on every line of a file. |
| `kind` | string | `run_start`, `model_call`, `tool_call`, `round` or `run_end`. |

`started_ms`, `ended_ms`, `duration_ms` and `wall_ms` are integer milliseconds. The first two and `wall_ms` are read on a monotonic clock with one zero, the *run clock*. Every writer starts the run clock at the same point, the one R0 uses: the start of the run, after the command line has been parsed and the configuration read, and before the session (providers, tools, middleware) is built and before the start tree hash. In R0 that is the first statement of the one-shot run of `craftsman -m`, where the writer is created. A `craftsman exec` writer (P2a) must start the run clock at the same point of its own run, and its tests pin it; until a P2a build is shown to keep that point, wall time is not compared across builds (8.8). `wall_ms` therefore includes building the session and the start tree hash (`run_start.tree_hash_ms` says how long that took) and does not include the end tree hash. `ts` is the wall clock.

## 5. Kinds

### 5.1 `run_start`

The first line of the file, written before any other event.

| Field | Type | Meaning |
|---|---|---|
| `model` | string | The model id the run was configured with. |
| `harness_profile` | string | The active harness profile key: `general`, `openai`, `claude` or `nemotron`. |
| `cwd_sha256` | hash | SHA-256 of the working directory path (6.5). The path is never written. |
| `tools` | array of strings | The names of the tools offered to the main agent's model, sorted ascending, as the model saw them. An empty list means the start is unknown: the first line of the file was a compaction call, made before the main agent's first request existed (the collector treats `tools == []` as "start unknown" and does not conclude that no tools were offered). |
| `entry` | string | How the run was started: `one-shot` for `craftsman -m`, `exec` for `craftsman exec`. |
| `producer` | object | The program that wrote the file: `name` (string), `version` (string) and, when the build knows it, `commit` (7 to 40 lowercase hex characters). The baseline R0 reads `commit` from the compile-time variable `CRAFTSMAN_BUILD_COMMIT`, which its documented build command sets. |
| `tree_hash` | hash, optional | The tree hash (6.6) when the run started, computed before the first model call of any kind. |
| `tree_hash_ms` | integer, optional | Milliseconds spent on that hash, whether or not it succeeded. Absent only when no attempt was made. |
| `tree_hash_skipped` | string | Present exactly when `tree_hash` is absent: `git_unavailable` (git could not be started), `git_failed` (git failed; this includes a directory that is not a git work tree), `timeout` (the 20 second budget ran out) or `not_attempted` (the writer had already stopped). |

### 5.2 `model_call`

One line per call to a model provider, written when the call has finished. A provider that retries internally still makes one call, so the tokens of attempts that failed inside it are not counted. The main agent, a subagent and compaction each use their own provider, which is what `role` records. A line has no round index: a compaction call is made while the next round is being prepared, so it appears after the `round` line of the previous round and before the `main` call of the round it prepares.

| Field | Type | Meaning |
|---|---|---|
| `role` | `main`, `subagent` or `compaction` | Which provider made the call. |
| `model` | string, optional | The model id this provider was configured with. R0 writes it on every line; when absent, `run_start.model` applies. |
| `input_tokens`, `output_tokens` | integer or null | Prompt and completion tokens. Null exactly when `usage_source` is `missing`. |
| `cached_input_tokens`, `reasoning_tokens` | integer, optional | The part of `input_tokens` served from a prompt cache, and the part of `output_tokens` spent on reasoning. Absent when unknown; R0 never writes them (its client does not read them from the provider), so tokens by role in R0 are totals only. |
| `usage_source` | `reported`, `estimated` or `missing` | `reported`: the provider sent usage. `estimated`: the harness counted the tokens itself because the provider sent none. `missing`: neither, and both counts are null. |
| `finish_reason` | string or null | The provider's finish reason (`stop`, `tool_calls`, `length`, ...). Null when the provider gave none, or the call failed. |
| `duration_ms` | integer | Wall time of the call from the request to the end of the stream, including retries inside the provider. |
| `error` | `true`, optional | Present, with the value `true`, when the call failed. Then `finish_reason` is null and `usage_source` is `missing`. Absent when the call succeeded; `false` is never written, and the schema rejects it. |

### 5.3 `tool_call`

One line per tool call that an agent dispatched, written when the call is over, whether it ran, failed or was refused. Tool calls run one at a time today. When a later phase runs reads in parallel, the lines are still written as calls finish, and `started_ms` and `ended_ms` show the overlap.

| Field | Type | Meaning |
|---|---|---|
| `tool` | string | The tool name as the model called it. |
| `agent` | `main` or `subagent` | Which agent made the call. A subagent's calls are traced too, so edits it makes count in every metric. A line does not say which subagent instance made it. |
| `round` | integer | The index of the main-agent round during which the call ran. |
| `call_id` | string, optional | The model's tool-call id, when the writer knows it. |
| `argument_sha256` | hash | Hash of the arguments the tool received (6.2). |
| `output_sha256` | hash | Hash of the exact text the model saw as the result (6.3). |
| `output_body_sha256` | hash, optional | Present only when the result ends with a block the harness appended (today the stale-todo reminder): the hash of the text without that block (6.3). |
| `redacted` | boolean | `true` when a redactor changed the result before the model saw it. |
| `started_ms`, `ended_ms` | integer | Run-clock milliseconds when the call entered and left the tool pipeline. The interval includes any wait for an approval. |
| `status` | `ok`, `error`, `denied` or `cancelled` | `ok`: the result is not an error. `error`: the tool returned or raised an error. `denied`: the approval gate or the permission mode refused the call, so the tool did not run. `cancelled`: the call was skipped or aborted because the run was cancelled. Every status other than `ok` is an error result (`is_error`) in the transcript. |
| `bytes` | integer | Length in bytes of the text hashed in `output_sha256`. |
| `output_truncated` | boolean | `true` when the tool cut its result at a size limit, so the end of its output (an `[exit code N]` line, the last lines of a test run) is not in the text. R0 detects the two markers the harness appends (`\n… [output truncated]` and `… [result truncated: N of M bytes shown]`) at the end of the text, before any harness-appended block. |
| `exit_code` | integer, optional | `execute` only: the exit status of the command. Absent means unknown; `null` is not used. A command that exits non-zero still has `status` `ok`, because the tool delivered its output; the exit code is the only place the trace shows the command failed. `-1` means the process ended without an exit status: it was killed by a signal. That includes the harness's own read cap: the shell tool reads at most 100,000 bytes from each of stdout and stderr and then closes that pipe, and a process that writes to the closed pipe dies of `SIGPIPE`. When that process is the command's shell itself (for example an `echo` that runs after a long output), the command gets `-1`, whatever status it would have exited with; when only a process inside it dies (`yes \| head -c 120000; exit 3`), the shell's own status (3) is kept. R0 also writes `-1` if waiting for the process failed. On Unix a command cannot exit with `-1` itself; on Windows an exit status of `0xFFFFFFFF` also reads as `-1`. |
| `path` | string | `write_file` and `edit_file`, required there: the target file (6.7). Present on every such line, whatever the status. |
| `sha256_before` | hash or null | `write_file` and `edit_file`, required there: SHA-256 of the file bytes just before the call (6.4). Null when the file did not exist. |
| `sha256_after` | hash or null | `write_file` and `edit_file`, required there: SHA-256 of the file bytes just after the call. Null when the file does not exist. |
| `hunks` | array of `[old_start, old_len, new_start, new_len]` | `write_file` and `edit_file`, required there: what the call changed, from the old file to the new file (6.8). Empty when the bytes are equal. |

The fields `path`, `sha256_before`, `sha256_after` and `hunks` appear on `write_file` and `edit_file` lines only, and `exit_code` on `execute` lines only.

### 5.4 `round`

A round is one main-agent model call plus the tool calls it asked for. The `round` line is written when the round is over, so it comes after the `main` `model_call` and the `tool_call` lines of that round. A run that stops with a failed model call or at the round limit still closes its last round before `run_end`.

| Field | Type | Meaning |
|---|---|---|
| `index` | integer | The round, counting from 0. |
| `tree_hash` | hash, optional | The tree hash (6.6) at the end of the round. Optional; the instrumented baseline does not write it. |
| `tree_hash_ms` | integer, optional | Milliseconds spent on that hash. |

### 5.5 `run_end`

The last line of a complete file.

| Field | Type | Meaning |
|---|---|---|
| `exit_reason` | string | Why the run ended. Values written so far: `end_turn` (the model finished), `max_turn_requests` (the round limit; the framework default of 50 in R0), `provider_failure` (a model call failed and ended the run), `error` (any other failure). Later builds add values such as `cancelled`, `interrupted`, `verification_failed`, `approval_required` and `max_tokens` (plan 5.4 and 5.5); the collector treats an unknown value as an opaque label. |
| `rounds` | integer | The number of rounds started, which is the number of `round` lines. |
| `wall_ms` | integer | Run-clock milliseconds when the turn ended (section 4: includes the start hash, excludes the end hash). |
| `tree_hash` | hash, optional | The tree hash (6.6) when the run ended. |
| `tree_hash_ms` | integer, optional | Milliseconds spent on that hash. Not part of `wall_ms`. |
| `tree_hash_skipped` | string | Present exactly when `tree_hash` is absent; same values as in `run_start`. |

### 5.6 File-level rules

JSON Schema checks one line. These rules need the whole file. `docs/schemas/check_trace.py` checks them together with the schema, and the writers' tests check them:

1. The first line is the only `run_start`, and its `seq` is 0.
2. `seq` increases by 1 on every line.
3. `schema`, `run_id` and `thread_id` are the same on every line.
4. There is at most one `run_end`, and it is the last line.
5. The `round` lines have `index` 0, 1, 2, and so on, and `run_end.rounds` equals their number.
6. In every `tool_call`, `ended_ms >= started_ms` and `round` is below the number of `round` lines.
7. In every edit line, `sha256_before` equals `sha256_after` exactly when `hunks` is empty. The hunks are ascending and separated: for consecutive hunks `h` and `g`, the number of old lines between them, `g.old_start - (h.old_start + h.old_len)`, equals the number of new lines between them, `g.new_start - (h.new_start + h.new_len)`, and is at least 1.
8. In every edit line, a file that did not exist before (`sha256_before` null) and exists after has no hunk or exactly one, `[1, 0, 1, n]` with `n >= 1`; a file that existed and does not exist after has exactly one, `[1, n, 1, 0]`.
9. A line whose `path` is `.` has `status` `error` or `denied`.

## 6. Hashing rules

### 6.1 General

Every hash is SHA-256 (FIPS 180-4) written as 64 lowercase hexadecimal characters. The text below says which bytes go in.

### 6.2 `argument_sha256`

The SHA-256 of the UTF-8 bytes of the canonical JSON of the arguments object the tool received, after any argument repair and before anything else touches it. If the model's argument text does not parse as JSON, the loop hands the tool the object `{"_raw": <the text>}`, and that object is what is hashed. Text that follows the JSON grammar still counts as text that does not parse when it holds a number whose magnitude overflows an IEEE 754 double (`1e400`, `-1e400`) or a string with an unpaired surrogate escape (`"\ud800"`, or `"\udc00\ud800"`, where neither is paired); R0's parser rejects both, so its arguments are hashed as `{"_raw": <the text>}`, and every writer must do the same even when its own parser would accept the text (some read `1e400` as infinity). The vectors file lists these cases under `unparsed_arguments` with the expected hash, which matches what the published R0 binary writes. Canonical JSON means:

- no whitespace outside strings;
- object keys sorted ascending by UTF-16 code unit at every depth; array order kept;
- `null`, `true` and `false` as literals;
- a number that is an integer in the range of a signed or unsigned 64-bit integer: exactly its decimal digits, with a leading `-` when negative (`-0` is the number zero and is written `0`);
- every other number (a fraction, an exponent, or an integer beyond 64 bits): read as the nearest IEEE 754 double and written by the ECMAScript algorithm `Number::toString(x)` (ECMA-262): the shortest digits that read back to the same double, in plain decimal notation when the decimal exponent allows (up to 21 integer digits, down to 1e-6), otherwise with an exponent as in `1e+21`, `1e-7` and `1.5e+300`. Two spellings of the same value hash the same: `1e2`, `1.0e2` and `100.0` all become `100`;
- strings in double quotes with only these escapes: `\"`, `\\`, `\b`, `\f`, `\n`, `\r`, `\t`, and `\u00xx` in lowercase hex for the other characters below U+0020; every other character, DEL and non-ASCII included, as its UTF-8 bytes.

This is a project rule, not a standard. It agrees with RFC 8785 (JCS) on the values tool arguments really contain, and differs from it where JCS rounds a 64-bit integer through a double. `docs/schemas/trace-v1.vectors.json` holds input and expected canonical text and hash for the edge cases (`canonical_json`) and for argument texts that do not parse (`unparsed_arguments`); a writer must reproduce every entry.

### 6.3 `output_sha256`, `output_body_sha256` and `bytes`

`output_sha256` is the SHA-256 of the UTF-8 bytes of the result text exactly as it is placed in the tool message the model receives next: after truncation, result budgets, reminders and, from P2a, redaction. `bytes` is the length of those bytes. When the call raised an error instead of returning a result, the text is the error message the loop puts into the tool message. The instrumented baseline has no redactor, so the text the model saw is the unredacted result: `output_sha256` covers it and `redacted` is `false`. From P2a the hash is of the redacted text and `redacted` is `true` when the redactor changed anything.

The harness appends a reminder block to a result once the todo list has gone stale. The block is a blank line, `<system-reminder>`, a line of text and `</system-reminder>`, at the very end. It makes the same tool output hash differently depending on when it happened, so when a result ends with such a block, `output_body_sha256` is the SHA-256 of the text without it (the blank line included). It is absent when there is no block. Comparisons of results ("the same output") use `output_body_sha256` when present and `output_sha256` otherwise. A tool whose own output ends in a block of the same shape is read as having a reminder; that is the only case in which the field misleads.

### 6.4 `sha256_before` and `sha256_after`

The writer reads the target file itself, immediately before the call starts and immediately after it returns, and hashes the bytes on disk. The read follows symbolic links (it is a `stat`, not an `lstat`): the hash is of the bytes of the file the link leads to. The value is null when the file does not exist, is a directory, cannot be read, or is a link that leads nowhere. The two are equal for a call that did not change the file, so a failed or denied edit has equal hashes and no hunks. The path `/memories` and every path under `/memories/` is a store entry, not a file: both hashes are null and `hunks` is empty.

### 6.5 `cwd_sha256`

The SHA-256 of the UTF-8 bytes of the working directory path exactly as the operating system reports it (the absolute path, not otherwise cleaned). R0 takes it from the process's current directory (`getcwd`), which on Unix is the real path with symbolic links resolved: a run started in `/tmp/w` on macOS hashes `/private/tmp/w`.

### 6.6 `tree_hash`

A hash of the content of the working tree that does not depend on any store of Craftsman's own. It is defined only inside a git working tree.

1. List the files with `git ls-files -z -co --exclude-standard` run in the working directory (tracked files plus untracked files that are not ignored). The writer runs git with `core.fsmonitor` and `core.untrackedCache` off and without `GIT_DIR`, `GIT_WORK_TREE`, `GIT_INDEX_FILE` and `GIT_COMMON_DIR` in its environment, so that no repository-configured helper runs and the listing is of the directory the agent works in. Remove duplicates and sort the paths by their bytes, ascending.
2. Keep each listed path that is a regular file on disk (an `lstat`: symbolic links are skipped, and so are directories, submodules among them, listed files that no longer exist and files that cannot be read).
3. Feed one record per kept file into one SHA-256: the path bytes as git printed them, one `0x00` byte, the 64-character file hash of 6.1 over the file's bytes, and one `\n`.
4. The result is the tree hash. A tree with no kept file hashes the empty input.
5. The trace file itself is left out when it lies inside the tree.

The hash is not computed when git is missing or fails, or when the computation takes more than 20 seconds; the field is then absent and `tree_hash_skipped` says why. On a very large tree the hash is slow (13 to 15 seconds for 100,000 files were measured); `tree_hash_ms` records the cost, and `wall_ms` is to be read with it (section 4).

### 6.7 `path`

The path the call targeted, taken from its `file_path` argument, resolved against the working directory when relative, and cleaned lexically (`.` removed, `..` resolved). When the result lies inside the working directory it is written relative to it with `/` separators (`src/lib.rs`); the working directory itself is written `.`. Otherwise it is written as the cleaned absolute path in the platform's own form (a Windows path keeps its backslashes). A missing, empty or non-string `file_path` is written `.` too: such a call never edits a file, so its line has `status` `error` or `denied`. Names that are not valid UTF-8 are converted with U+FFFD in place of the bad bytes.

### 6.8 `hunks`

The change a call made to a file, as hunks from the old bytes to the new bytes. The definition uses only the old and new bytes, so it does not depend on the tool, and stays valid when the edit engine changes. A file that did not exist counts as empty, in either position.

1. Split both versions into lines, each line keeping its `\n`. A missing final newline therefore belongs to the last line, and adding one changes that line.
2. Remove the longest common prefix of lines, then the longest common suffix that does not overlap it. Nothing left on either side means no change: an empty list.
3. Find a shortest edit script for the rest with Myers' O(ND) greedy algorithm exactly as published (Myers, "An O(ND) Difference Algorithm and Its Variations", 1986: the forward search over the array `V`). A shortest script is not unique and the choice changes the hunks, so the rule is part of the definition: in round `d`, the furthest-reaching path on diagonal `k` comes down from diagonal `k+1`, an insertion, when `k = -d` or (`k != d` and `V[k-1] < V[k+1]`), and otherwise comes right from diagonal `k-1`, a deletion; a deletion therefore wins when `V[k-1] >= V[k+1]`. The path is read back from the end with the same test, applied to the values `V` had at the start of each round. If the edit distance exceeds 2000, stop and treat the rest (everything between the common prefix and suffix of step 2) as one hunk. The vectors file has both sides of the limit: a distance of 2000 gives two hunks, a distance of 2001 one.
4. A hunk is a maximal run of consecutive deleted or inserted lines. It is written `[old_start, old_len, new_start, new_len]`: `old_len` lines of the old file starting at line `old_start` were replaced by `new_len` lines of the new file starting at line `new_start`. Line numbers are 1-based and include the removed common prefix. For a side of length 0 the start is the number of the line that follows the gap (one more than the number of lines before it, so it can be one past the last line). The two lengths are never both 0.
5. Hunks are in file order and are separated by at least one common line, the same number on both sides.

For a reader who wants the lines of the new file that changed, as `[start, end]` ranges: a hunk with `new_len > 0` gives `[new_start, new_start + new_len - 1]`; a hunk with `new_len = 0` (a deletion) gives the single line `[m, m]` where `m` is `new_start` or the last line of the new file if that is smaller, and nothing if the new file is empty; then merge ranges that overlap or touch. The ranges are derived; the hunks are the record.

| Old | New | `hunks` |
|---|---|---|
| `a\nb\nc\n` | `a\nB\nc\n` | `[[2,1,2,1]]` |
| `a\nb\n` | `a\nx\ny\nb\n` | `[[2,0,2,2]]` |
| `a\nb\nc\n` | `a\nc\n` | `[[2,1,2,0]]` |
| `a\nb\nc\n` | `a\nb\n` | `[[3,1,3,0]]` |
| `1\n2\n3\n4\n5\n6\n7\n8\n9\n` | `1\nX\n3\n4\n5\n6\n7\nY\n9\n` | `[[2,1,2,1],[8,1,8,1]]` |
| (no file) | `x\ny\n` | `[[1,0,1,2]]` |
| `a\nb` | `a\nb\n` | `[[2,1,2,1]]` |
| `a\n` | (empty) | `[[1,1,1,0]]` |
| `a\nb\n` | `a\nb\n` | `[]` |
| `a\nb\n` | `b\na\na\n` | `[[1,1,1,0],[3,0,2,2]]` |
| `a\nb\n` | `c\nb\na\n` | `[[1,1,1,1],[3,0,3,1]]` |

The last two rows have two shortest scripts each (keep the `a` or keep the `b`); the rule above picks the script that gives these hunks. As new-file ranges they are `[[1,3]]` and `[[1,1],[3,3]]`. The complete set of vectors is `docs/schemas/trace-v1.vectors.json`.

The reference writer is `crates/craftsman-rs/src/trace.rs` in the instrumented baseline. P2a reuses its helpers (canonical JSON, file and tree hashes, `hunks`) rather than writing them again, and its unit tests run the vectors file.

## 7. What R0 and P2a write

P2a adopts the schema unchanged. The two builds differ only in which optional or conditional parts they can fill in, because some need things R0 does not have: the redactor and the usage meter of P2a, and its per-round tree hash.

| Item | R0, `baseline-0.4.0-instrumented` | `craftsman exec` (P2a) |
|---|---|---|
| `entry` | `one-shot` | `exec` |
| `producer` | `craftsman`, the crate version, the commit given at build time | the same |
| `thread_id` | the session's thread id | null under `--ephemeral` |
| `model_call.model` | written | written |
| `model_call.usage_source` | `reported`, or `missing` with null counts when the provider sent no usage. Never `estimated`: R0 has no usage meter | `reported`, or `estimated` when the provider sent none and the harness counted the tokens (`TiktokenCounter`); `missing` only if counting is impossible |
| `model_call.cached_input_tokens`, `reasoning_tokens` | never written | optional |
| `tool_call.output_sha256`, `redacted` | hash of the raw result, which is what the model saw; `redacted` always `false`: R0 has no redactor | hash of the redacted result, which is what the model saw; `redacted` is `true` when the redactor changed it |
| `tool_call.output_body_sha256` | written when a todo reminder was appended | the same |
| `tool_call.output_truncated` | read from the two truncation markers at the end of the text | from the tool's own metadata |
| `round.tree_hash`, `round.tree_hash_ms` | absent | present on every round |
| `run_start.tree_hash`, `run_end.tree_hash` | present inside a git working tree, else `tree_hash_skipped`; the start hash is computed before the first model call, compaction included | the same |
| `tool_call.status` `denied` | recognised from the refusal text that the gates return as an error result (`Tool call denied by the user.`, `Permission mode '…`, and `… requires user approval, but this delegated run has no interactive approval channel`); a tool whose own error text began the same way would be misread | from the permission decision itself |
| `tool_call.status` `cancelled` | never written: `-m` has no cancel source | written (P2c, P2g) |
| `tool_call.exit_code` | the real exit status, which the shell implementation hands to the writer, so it survives truncation; `-1` when the process ended without one (killed by a signal, including a shell that got `SIGPIPE` writing to a pipe the 100,000-byte per-pipe read cap had closed; 5.3); if the hand-over is missing, read from the result text (leaving out the reminder block: a final `[exit code N]` line gives N, no such line gives 0, and a truncated result gives none); absent when the call failed to run | from the tool result's metadata once P2b provides it, otherwise by the same rule; `-1` keeps the meaning of 5.3 |
| `tool_call.call_id` | present for the main agent's calls, absent for a subagent's | present for all |
| `run_end.exit_reason` | `end_turn`, `max_turn_requests`, `provider_failure`, `error` | the same, plus the values of plan 5.5 |
| a path in the working directory, or a symbolic link, or a file that is not a trace | no tracing (silently) | defined in `docs/headless-mode.md` |

Activation is not part of the schema. R0 is switched on by the environment variable `CRAFTSMAN_TRACE_FILE=<path>`, read for tracing by `craftsman -m` only, and does nothing when it is unset or empty. Whenever the variable is set and non-empty, R0 removes it from the environment of the commands the agent runs, of language servers, of MCP servers and of the checkpoint `git` call, so a traced run does not differ from an untraced one. It does so in every mode, not only while a trace is being written: in `craftsman -m` also when the path was refused, and in every other mode (the TUI, `acp-serve`, `acp-bridge`, `service-agent`), which ignore the variable for tracing but still hide it from any such child process they start. That is harmless: the variable has no effect outside `-m`, and it is absent in normal use. `craftsman exec` takes its own option, defined in `docs/headless-mode.md`.

## 8. Scoreboard metrics from the trace

Plan 5.9 names the metrics; this section says how each one is computed from the lines above, and what else it needs. The collector (P3a) implements it once for every build. The definitions are normative: two collectors written from this text must give the same numbers.

### 8.0 What the trace does not contain

| Needed | Comes from | Notes |
|---|---|---|
| Pass or fail | the task's grader, run by the runner after the process ends | pass rate is not computed from the trace |
| Task, suite, model and repeat | the runner | The runner chooses the path of every trace file (`CRAFTSMAN_TRACE_FILE` for R0, the `exec` option for P2a), for example `<suite>/<task>/<model>/<repeat>/trace.jsonl`. **That path is the join key.** `run_id` is generated by the writer and cannot serve |
| Build under test | `run_start.producer` (name, version, commit) | the runner maps a commit to R0, R1 or a candidate |
| Task snapshot and reference solution | the corpus (P3a) | needed by wrong-place edits (8.3) |
| Prices | the runner's price table | the trace has tokens, not dollars (8.6) |
| Prompt-registry versions | not in schema v1 | R0's prompts are fixed by `producer.commit`; a registry field is a later optional addition |

Two cross-checks tie a trace to its run. A check that applies and fails marks the run invalid.

- `cwd_sha256` equals the SHA-256 of the directory the runner started the process in, taken as the operating system resolves it: the runner hashes the real path of that directory (`realpath` of it, or `getcwd` inside the started process), because that is what the writer hashes (6.5). Hashing the path as the runner spelled it would mark valid runs invalid whenever a symbolic link or a bind mount is on the way (macOS `/tmp` is `/private/tmp`).
- `run_start.tree_hash` equals the tree hash (6.6) of the task snapshot. This check applies only when both sides have a hash. When `run_start` has `tree_hash_skipped` instead (for example `git_failed`, because the snapshot is not a git work tree), or the runner has no tree hash of the snapshot, the check is *not applicable*: the report says so, and the run is not invalid for it.

### 8.1 Conventions

- The *call sequence of an agent* is its `tool_call` lines (`agent` equal to `main`, or to `subagent`) in `seq` order, with positions 0, 1, 2, and so on. Every tool counts for a position, edits and reads alike, and so do the lines of edit calls that are left out below (store entries). The order is the order of `seq`; with the serial dispatch of today it is also the order of `started_ms`. A line does not say which subagent instance wrote it, so when several subagents run at once their lines share one sequence and the rules below are approximate for them.
- An *edit call* is a `tool_call` of `write_file` or `edit_file` whose `path` is not `/memories` or under `/memories/` (those are store entries, 6.4, and are left out of every rule below).
- An edit call is *applied* when `status` is `ok` and `sha256_after` differs from `sha256_before`. A `write_file` that creates a file is applied (null before, a hash after).
- A result's *comparison hash* is `output_body_sha256` when present, else `output_sha256`.
- Several numbers below are *collector parameters*. The defaults in this table are proposals; P3a pins them before the first live run and every report records the values it used. Because the collector works offline on stored traces, changing one does not need a new model run.

| Parameter | Default | Used by |
|---|---|---|
| `N_REPEAT` | 3 | stuck, rule 1 |
| `W_FAIL` | 10 | stuck, rule 2 |
| `N_RETURN` | 2 | stuck, rule 3 |
| `N_INERT` | 8 | stuck, rule 4 |
| read-only tools | `read_file`, `ls`, `glob`, `grep`, `web_fetch`, `web_search`, `write_todos`, `ask_user` and the `lsp_*` tools | stuck, rule 4 |
| `SLACK` | 0 lines | wrong-place edits |
| `K_RETRY` | 3 positions | first-try apply |

The read-only list is pinned over the union of the tool names of the builds being compared (every name in their traces' `run_start.tools` and `tool` fields), so that the same name is classified the same way for every build. Each report records the list it used and every tool name that it has not classified (a name that is in none of the read-only list, the edit tools, `execute` and `task`); such a name counts as changing state (rule 4).

### 8.2 First-try edit-apply rate

1. An *attempt* is an edit call whose `status` is `ok` or `error`. Denied and cancelled edits never reached the file tools and are not attempts.
2. An attempt is *applied* (8.1), a *no-op* (`ok` with equal hashes: the call succeeded and the file did not change) or *failed* (`error`). A failed attempt and a no-op are both *unsuccessful*.
3. An attempt B is a *retry* of an unsuccessful attempt A when both are in the same agent's call sequence, have the same `path`, B is the earliest later attempt on that path in the same agent's call sequence (attempts by the other agent are ignored), and B's position is at most `K_RETRY` after A's. In v1 every subagent instance writes to the one `subagent` sequence (8.1), so instances that run one after the other share it as well: a failed edit by one instance can be retried by the next.
4. An attempt that is not a retry is an *initial attempt*. A *chain* is an initial attempt and the retries that follow it, each the retry of the one before. A chain is *first-try* when it has length 1 and its attempt is applied. A retry that is applied does not make its chain first-try: a model that needed a second attempt did not apply on the first.
5. The rate is the number of first-try chains divided by the number of chains (both agents pooled; the collector also reports each agent alone), and is undefined when there are no chains.

A `write_file` refused because the file exists is a failed attempt, and the `edit_file` that follows it on the same path within `K_RETRY` positions is its retry. Plan 5.9 defines a first-try apply as "an edit that is not followed by a retry". Read literally that counts the successful retry of a failed edit as a first-try apply (the sequence error, ok gives 1 of 2); this section counts chains instead (0 of 1).

*Worked example: another agent in between.* In R0 a subagent's lines fall inside the main agent's `task` call, which is written when it is over. Take these lines, all on the path `f.rs`:

| `seq` | `agent` | Call | Position in its agent's sequence |
|---|---|---|---|
| 1 | `main` | `edit_file`, `error` | main 0 |
| 2 | `subagent` | `edit_file`, `ok`, applied | subagent 0 |
| 3 | `main` | `task`, `ok` | main 1 |
| 4 | `main` | `edit_file`, `ok`, applied | main 2 |

The subagent's attempt at `seq` 2 is ignored when looking for the retry of the main agent's failed attempt, so `seq` 4 (main position 2, two positions after it) is that retry. There are two chains: the main chain `seq` 1, 4 (length 2, not first-try) and the subagent chain `seq` 2 (length 1, applied, first-try). The rate is 1 of 2. Reading "earliest" across both agents would make `seq` 2 the next attempt, `seq` 4 an initial attempt, and give 2 of 3; that reading is wrong.

### 8.3 Wrong-place edits

Each task ships a snapshot and a reference solution. The metric asks whether an applied edit changed lines that the reference solution also changed, with both measured on the *snapshot* file. Hunks (6.8) describe each edit from the file before that edit, so an edit after others has to be mapped back.

*Touched lines.* For a hunk `[os, ol, ns, nl]` the *old side* is the set of integers `os … os+ol-1` when `ol > 0`, and `{os-1, os}` when `ol = 0` (a pure insertion touches the two lines next to the gap). The values 0 and one past the last line are allowed: they stand for the top and the bottom of the file.

*Mapping back.* For an edit with hunks `H`, a line `x` of the file after the edit goes to the file before it as follows. If some hunk has `ns ≤ x < ns+nl`, the result is that hunk's old side. Otherwise the result is the single line `x − Σ(nl − ol)`, summed over the hunks of `H` with `ns + nl ≤ x`. A set of lines maps to the union.

*Chain.* For a path `p`, take its applied edit calls from both agents in `seq` order, `e1 … ek`. The chain is *valid* for `e1` when `e1.sha256_before` is the SHA-256 of the snapshot file (null when the snapshot has no such file), and for `e(j+1)` when it is valid for `ej` and `e(j+1).sha256_before = ej.sha256_after`. The first edit that breaks this, and every later applied edit on `p`, is *unmapped*: something outside the trace (a shell command) changed the file in between.

*Snapshot touched set.* For a mapped edit `ej`, take the union of the old sides of its hunks and map it back through `e(j-1)`, then `e(j-2)`, down to `e1`. The result is in the lines of the snapshot file.

*Reference set.* Compute the hunks of the snapshot file against the solution file with the helper of 6.8 (a missing file is empty), take the union of their old sides, and add every integer within `SLACK` of a member. A path the solution does not change has an empty reference set. The solution is a complete tree: a file that the snapshot has and the solution lacks is a deletion, and the reference set is the whole file.

An applied, mapped edit is a *wrong-place edit* when its snapshot touched set and the reference set of the same path have no line in common. The metric is the number of wrong-place edits divided by the number of mapped applied edits, times 100. The collector reports the count of unmapped edits next to it, and the metric is marked partial when that count is not zero.

*Edits through `execute`.* A shell command can change files (`sed -i`, `patch`, a script, a redirect) without an edit line. Such a change is invisible to this metric in R0: it adds no edit to the count, and if it touches a file that an edit line also touches, the hash chain breaks and later edits of that file are unmapped. The per-round `tree_hash` of P2a lets the collector see more: a round whose tree hash differs from the previous one, with no applied edit line in the round, is an *unattributed state change*. The collector counts those rounds and reports them with the metric. Attributing them to files is not part of schema v1; P2a may add an optional field for it.

### 8.4 Tool-error rate

The number of `tool_call` lines whose `status` is not `ok`, divided by the number of `tool_call` lines, over both agents. This is the plan's definition, a call whose result is an error. The collector also reports the count of each status and the rate of each agent, because a denial depends on the permission setup and a cancellation on an interruption.

### 8.5 Stuck

A run is *stuck* when any of the four rules of plan 5.6 fires on the main agent's call sequence. The collector reports which rule fired and where. A call *fails* when its `status` is `error`, `denied` or `cancelled`, or when it is an `execute` call whose `exit_code` is present and not 0. An `execute` call with no `exit_code` is not counted as failing, and the collector reports how many there are. An `exit_code` of `-1` (no exit status, 5.3) counts as failing; because R0 also writes it for a command that printed more than the 100,000-byte read cap, the collector reports how many failing calls have `-1`.

1. **The same call repeated.** `N_REPEAT` consecutive calls with the same `tool` and the same `argument_sha256`.
2. **The same failing output twice.** Any two failing calls (not only neighbours among the failing calls), at most `W_FAIL` positions apart, with the same `tool` and the same comparison hash (8.1). A result cut at a size limit (`output_truncated`) compares by the text that is left.
3. **Oscillating file hashes.** For one `path`, list the file states that the applied edits of the main agent on it recorded, in order: `sha256_before` then `sha256_after` of each edit, with repeats of the previous state dropped. Count the positions whose state already appeared two or more positions earlier (an A, B, A has one such return). The rule fires when the count reaches `N_RETURN`.
4. **Rounds with no state change.** `N_INERT` consecutive rounds, each with at least one main-agent call and none that changes state. A call changes state when its `status` is `ok` and it is either an applied edit, or a call to a tool that is not a read-only tool and not an edit tool (`execute`, `task` and unknown tools such as MCP tools count; the read-only list is pinned as 8.1 says). Failed, denied and read-only calls do not, and neither does an `ok` call to an edit tool that is not an applied edit (a no-op, or a store entry).

Rules 3 and 4 are *call-based* here: they see only what the trace lines of tools say. That is the form every build supports, so it is the stuck metric for every build. It is a proxy: an `execute` that changes nothing counts as a state change, and a shell edit that no edit line shows is not seen by rule 3. Where every round of a window carries a `tree_hash` (P2a), the collector also reports tree-based variants, next to the metric and never in its place: rule 3 on the sequence of tree hashes (`run_start.tree_hash`, then each round's, with repeats of the previous dropped, counted as above), and rule 4 with a round counted as inert when its `tree_hash` equals the previous round's (or `run_start`'s for round 0).

### 8.6 Tokens by role

For each `role`, the sums of `input_tokens` and `output_tokens` over the `model_call` lines whose `usage_source` is `reported` or `estimated`, the number of calls, and the number of lines of each `usage_source`. Estimated counts are reported apart from reported ones. A `missing` line adds no tokens and is counted. The total of a run is the sum over the three roles. A provider that retries inside one call reports only the call that succeeded, so tokens spent on failed attempts are not counted. Cached and reasoning tokens are summed only where the fields exist (not in R0). Dollars are not in the trace: the runner applies its own price table to the tokens by role and model.

### 8.7 The rest

The number of rounds and the exit reason are `rounds` and `exit_reason` of `run_end`; a run without `run_end` is reported as crashed. *Wall time* is `run_end.wall_ms` minus `run_start.tree_hash_ms` minus the `tree_hash_ms` of the round lines that have one: the time of the run without the tree hashes, which is comparable between builds that hash differently, provided they start the run clock at the same point (section 4). Pass rate comes from the task's grader, not from the trace.

### 8.8 What each build can supply

*Yes*: computed from the trace as defined. *Partial*: computed, with the limit named. *No*: not derivable from the trace. The *common* column is what a comparison between R0 and a P2a build may use; the collector does not compare the other forms across builds.

| Metric | R0 | P2a (`craftsman exec`) | Common subset | Also needs |
|---|---|---|---|---|
| Pass rate | no | no | — | grader verdict |
| First-try apply | yes | yes | all of it | — |
| Wrong-place edits | partial: edits by edit tools only; unmapped edits excluded | partial: the same, plus a count of unattributed state changes | mapped edits | snapshot, solution |
| Tool-error rate | yes (`denied` recognised from text) | yes | all of it | — |
| Stuck, rule 1 | yes | yes | all of it | — |
| Stuck, rule 2 | yes (exit status from the shell implementation; non-`execute` tools fail by `status`) | yes | all of it | — |
| Stuck, rule 3 | partial: edit tools only | partial, plus the tree-based variant | the edit-hash form | — |
| Stuck, rule 4 | partial: call-based form | call-based form, plus the tree-based variant | the call-based form | — |
| Tokens by role | partial: no estimated counts, no cached or reasoning split, provider retries not counted | yes (estimated counts marked) | reported counts | prices (for dollars) |
| Exit reason | yes (four values) | yes (plan 5.5 values) | the four values | — |
| Wall time | yes (start hash subtracted; clock zero of section 4) | yes, once its run clock starts at the point of section 4 | all of it, but only once a P2a build is shown to start the run clock at the point of section 4; until then none | — |

## 9. Examples

The hashes in examples 1 to 3 are real: each is the SHA-256 of the text or file named in the description, computed with the rules of section 6 (the `producer` commit is invented). Examples 1 and 2 run in `/work/addlib`, a crate whose `src/lib.rs` has a bug in `add`; the task is to fix it. The hunk `[2,1,2,1]` of the edit says: one line of the old file, starting at line 2, was replaced by one line starting at line 2 of the new file.

### Example 1: read, edit, answer (R0)

The model reads the file, replaces `a - b` with `a + b`, and answers. The provider sent usage every time. The tree hash changed because the file did.

```jsonl
{"schema":"craftsman-harness/trace@1","seq":0,"ts":"2026-10-16T09:14:03.120Z","run_id":"run-0199f3a2c7d1-8e41b6a05c93d2f7","thread_id":"1792142042-1f4c-0000","kind":"run_start","model":"example-model","harness_profile":"general","cwd_sha256":"db45314eb66a36f744b4173bd01e8831021e00e4ec652ee73eb3edc0a05a6dfd","tools":["edit_file","execute","glob","grep","ls","read_file","write_file"],"entry":"one-shot","producer":{"name":"craftsman","version":"0.4.0","commit":"3f9c2e7a41d0"},"tree_hash":"a4ad120b6f9c67b2bbc1026c5943d9efa9f14bb864990411990e2c3810dfdffd","tree_hash_ms":7}
{"schema":"craftsman-harness/trace@1","seq":1,"ts":"2026-10-16T09:14:04.037Z","run_id":"run-0199f3a2c7d1-8e41b6a05c93d2f7","thread_id":"1792142042-1f4c-0000","kind":"model_call","role":"main","model":"example-model","input_tokens":1840,"output_tokens":31,"usage_source":"reported","finish_reason":"tool_calls","duration_ms":912}
{"schema":"craftsman-harness/trace@1","seq":2,"ts":"2026-10-16T09:14:04.043Z","run_id":"run-0199f3a2c7d1-8e41b6a05c93d2f7","thread_id":"1792142042-1f4c-0000","kind":"tool_call","tool":"read_file","agent":"main","round":0,"call_id":"call_1","argument_sha256":"047d38e5a5eb4eee54d646129be09e973fbe996a9dbfa32de14610869ebe6f39","output_sha256":"140a43b53b7177c9aecb4f555acfe9bce09b0f6ea47248350d7e008522aaf222","redacted":false,"started_ms":1140,"ended_ms":1143,"status":"ok","bytes":68,"output_truncated":false}
{"schema":"craftsman-harness/trace@1","seq":3,"ts":"2026-10-16T09:14:04.044Z","run_id":"run-0199f3a2c7d1-8e41b6a05c93d2f7","thread_id":"1792142042-1f4c-0000","kind":"round","index":0}
{"schema":"craftsman-harness/trace@1","seq":4,"ts":"2026-10-16T09:14:04.690Z","run_id":"run-0199f3a2c7d1-8e41b6a05c93d2f7","thread_id":"1792142042-1f4c-0000","kind":"model_call","role":"main","model":"example-model","input_tokens":1962,"output_tokens":58,"usage_source":"reported","finish_reason":"tool_calls","duration_ms":640}
{"schema":"craftsman-harness/trace@1","seq":5,"ts":"2026-10-16T09:14:04.701Z","run_id":"run-0199f3a2c7d1-8e41b6a05c93d2f7","thread_id":"1792142042-1f4c-0000","kind":"tool_call","tool":"edit_file","agent":"main","round":1,"call_id":"call_2","argument_sha256":"6d0e1014aa33b9f9e8a9ad0da2e39a99cd7b4e7d7a6813c1d4db274116c49d4b","output_sha256":"5d2b53e74462d1d3d2272581f3d494deac2025d4b83273bad2668362751954ac","redacted":false,"started_ms":1795,"ended_ms":1801,"status":"ok","bytes":23,"output_truncated":false,"path":"src/lib.rs","sha256_before":"f8306ec92a207d3b1b584d5216371fcf4d3b5a727d212e4180c3c6c10f06478f","sha256_after":"821d282d75c051d9a2a445ad8ef1551004aba5b3322d7a354c8a2abcd15af1e6","hunks":[[2,1,2,1]]}
{"schema":"craftsman-harness/trace@1","seq":6,"ts":"2026-10-16T09:14:04.703Z","run_id":"run-0199f3a2c7d1-8e41b6a05c93d2f7","thread_id":"1792142042-1f4c-0000","kind":"round","index":1}
{"schema":"craftsman-harness/trace@1","seq":7,"ts":"2026-10-16T09:14:05.210Z","run_id":"run-0199f3a2c7d1-8e41b6a05c93d2f7","thread_id":"1792142042-1f4c-0000","kind":"model_call","role":"main","model":"example-model","input_tokens":2051,"output_tokens":14,"usage_source":"reported","finish_reason":"stop","duration_ms":505}
{"schema":"craftsman-harness/trace@1","seq":8,"ts":"2026-10-16T09:14:05.211Z","run_id":"run-0199f3a2c7d1-8e41b6a05c93d2f7","thread_id":"1792142042-1f4c-0000","kind":"round","index":2}
{"schema":"craftsman-harness/trace@1","seq":9,"ts":"2026-10-16T09:14:05.212Z","run_id":"run-0199f3a2c7d1-8e41b6a05c93d2f7","thread_id":"1792142042-1f4c-0000","kind":"run_end","exit_reason":"end_turn","rounds":3,"wall_ms":2312,"tree_hash":"cb230a1cdbf9b4a6e44add6e889711f6df59704290879be46dd1ffdb0dd20d10","tree_hash_ms":6}
```

The inputs behind the hashes are these. The working directory is `/work/addlib`. The tree covers two files, `Cargo.toml` and `src/lib.rs`:

```text
Cargo.toml
[package]
name = "addlib"
version = "0.1.0"
edition = "2024"

src/lib.rs before the edit (after it, line 2 is "    a + b")
pub fn add(a: i32, b: i32) -> i32 {
    a - b
}
```

The first `tool_call` has the arguments `{"file_path":"src/lib.rs"}` and the result text below. The second has the arguments `{"file_path":"src/lib.rs","new_string":"a + b","old_string":"a - b","replace_all":false}` (keys sorted, as 6.2 says) and the result text `Updated file src/lib.rs`.

```text
     1	pub fn add(a: i32, b: i32) -> i32 {
     2	    a - b
     3	}
```

From this trace the collector reads: first-try edit-apply 1 of 1, tool-error rate 0 of 2, not stuck, main-agent tokens 5,853 in and 103 out over 3 calls, 3 rounds, exit `end_turn`.

### Example 2: a failed edit and its retry (R0)

The first `edit_file` uses `a-b`, which is not in the file, so the tool returns an error and the file is unchanged: equal hashes and no ranges. The model corrects it in the next round. The last model call came back with no usage and no finish reason, so its counts are null.

```jsonl
{"schema":"craftsman-harness/trace@1","seq":0,"ts":"2026-10-16T09:18:10.610Z","run_id":"run-0199f3a9e0b4-52c7d1e9a84f3b06","thread_id":"1792142290-1f4c-0000","kind":"run_start","model":"example-model","harness_profile":"general","cwd_sha256":"db45314eb66a36f744b4173bd01e8831021e00e4ec652ee73eb3edc0a05a6dfd","tools":["edit_file","execute","glob","grep","ls","read_file","write_file"],"entry":"one-shot","producer":{"name":"craftsman","version":"0.4.0","commit":"3f9c2e7a41d0"},"tree_hash":"a4ad120b6f9c67b2bbc1026c5943d9efa9f14bb864990411990e2c3810dfdffd","tree_hash_ms":7}
{"schema":"craftsman-harness/trace@1","seq":1,"ts":"2026-10-16T09:18:11.420Z","run_id":"run-0199f3a9e0b4-52c7d1e9a84f3b06","thread_id":"1792142290-1f4c-0000","kind":"model_call","role":"main","model":"example-model","input_tokens":1790,"output_tokens":47,"usage_source":"reported","finish_reason":"tool_calls","duration_ms":801}
{"schema":"craftsman-harness/trace@1","seq":2,"ts":"2026-10-16T09:18:11.434Z","run_id":"run-0199f3a9e0b4-52c7d1e9a84f3b06","thread_id":"1792142290-1f4c-0000","kind":"tool_call","tool":"edit_file","agent":"main","round":0,"call_id":"call_1","argument_sha256":"d0a0cf79f8170e9800a30b55ac142826a0e21ea8caccfedc2a9bc650428f9ad2","output_sha256":"9cc535163d08646dd30813c4731fd961efda9f834c576250cf39983f818177cd","redacted":false,"started_ms":1026,"ended_ms":1034,"status":"error","bytes":67,"output_truncated":false,"path":"src/lib.rs","sha256_before":"f8306ec92a207d3b1b584d5216371fcf4d3b5a727d212e4180c3c6c10f06478f","sha256_after":"f8306ec92a207d3b1b584d5216371fcf4d3b5a727d212e4180c3c6c10f06478f","hunks":[]}
{"schema":"craftsman-harness/trace@1","seq":3,"ts":"2026-10-16T09:18:11.435Z","run_id":"run-0199f3a9e0b4-52c7d1e9a84f3b06","thread_id":"1792142290-1f4c-0000","kind":"round","index":0}
{"schema":"craftsman-harness/trace@1","seq":4,"ts":"2026-10-16T09:18:12.160Z","run_id":"run-0199f3a9e0b4-52c7d1e9a84f3b06","thread_id":"1792142290-1f4c-0000","kind":"model_call","role":"main","model":"example-model","input_tokens":1901,"output_tokens":52,"usage_source":"reported","finish_reason":"tool_calls","duration_ms":720}
{"schema":"craftsman-harness/trace@1","seq":5,"ts":"2026-10-16T09:18:12.171Z","run_id":"run-0199f3a9e0b4-52c7d1e9a84f3b06","thread_id":"1792142290-1f4c-0000","kind":"tool_call","tool":"edit_file","agent":"main","round":1,"call_id":"call_2","argument_sha256":"6d0e1014aa33b9f9e8a9ad0da2e39a99cd7b4e7d7a6813c1d4db274116c49d4b","output_sha256":"5d2b53e74462d1d3d2272581f3d494deac2025d4b83273bad2668362751954ac","redacted":false,"started_ms":1765,"ended_ms":1771,"status":"ok","bytes":23,"output_truncated":false,"path":"src/lib.rs","sha256_before":"f8306ec92a207d3b1b584d5216371fcf4d3b5a727d212e4180c3c6c10f06478f","sha256_after":"821d282d75c051d9a2a445ad8ef1551004aba5b3322d7a354c8a2abcd15af1e6","hunks":[[2,1,2,1]]}
{"schema":"craftsman-harness/trace@1","seq":6,"ts":"2026-10-16T09:18:12.173Z","run_id":"run-0199f3a9e0b4-52c7d1e9a84f3b06","thread_id":"1792142290-1f4c-0000","kind":"round","index":1}
{"schema":"craftsman-harness/trace@1","seq":7,"ts":"2026-10-16T09:18:12.690Z","run_id":"run-0199f3a9e0b4-52c7d1e9a84f3b06","thread_id":"1792142290-1f4c-0000","kind":"model_call","role":"main","model":"example-model","input_tokens":null,"output_tokens":null,"usage_source":"missing","finish_reason":null,"duration_ms":515}
{"schema":"craftsman-harness/trace@1","seq":8,"ts":"2026-10-16T09:18:12.691Z","run_id":"run-0199f3a9e0b4-52c7d1e9a84f3b06","thread_id":"1792142290-1f4c-0000","kind":"round","index":2}
{"schema":"craftsman-harness/trace@1","seq":9,"ts":"2026-10-16T09:18:12.692Z","run_id":"run-0199f3a9e0b4-52c7d1e9a84f3b06","thread_id":"1792142290-1f4c-0000","kind":"run_end","exit_reason":"end_turn","rounds":3,"wall_ms":2292,"tree_hash":"cb230a1cdbf9b4a6e44add6e889711f6df59704290879be46dd1ffdb0dd20d10","tree_hash_ms":6}
```

The second edit is a retry of the first (same path, the next attempt, one position later), so there is one chain, of length 2, and it is not first-try: first-try edit-apply 0 of 1, tool-error rate 1 of 2. The result text of the failed call is `state error: Error: old_string not found in /work/addlib/src/lib.rs`.

### Example 3: a stuck exec run

A `craftsman exec` run with `--max-rounds 3` and `--ephemeral`, so the thread is null. The model sends the same `grep` three times in a row and gets the same output, so rule 1 of 8.5 fires at the third call. The first call's usage was counted by the harness (`estimated`), and each `round` line carries the tree hash, which does not change.

```jsonl
{"schema":"craftsman-harness/trace@1","seq":0,"ts":"2026-10-16T09:31:44.280Z","run_id":"run-0199f3b4a1c8-d03e75b21a96c4f8","thread_id":null,"kind":"run_start","model":"example-model","harness_profile":"general","cwd_sha256":"db45314eb66a36f744b4173bd01e8831021e00e4ec652ee73eb3edc0a05a6dfd","tools":["edit_file","execute","glob","grep","ls","read_file","write_file"],"entry":"exec","producer":{"name":"craftsman","version":"0.5.0"},"tree_hash":"a4ad120b6f9c67b2bbc1026c5943d9efa9f14bb864990411990e2c3810dfdffd","tree_hash_ms":7}
{"schema":"craftsman-harness/trace@1","seq":1,"ts":"2026-10-16T09:31:45.000Z","run_id":"run-0199f3b4a1c8-d03e75b21a96c4f8","thread_id":null,"kind":"model_call","role":"main","model":"example-model","input_tokens":1210,"output_tokens":22,"usage_source":"estimated","finish_reason":"tool_calls","duration_ms":700}
{"schema":"craftsman-harness/trace@1","seq":2,"ts":"2026-10-16T09:31:45.012Z","run_id":"run-0199f3b4a1c8-d03e75b21a96c4f8","thread_id":null,"kind":"tool_call","tool":"grep","agent":"main","round":0,"call_id":"call_1","argument_sha256":"ae3c4784f60c8228d37f7978f007c87de5c146f04887071980c39613d3eca7bd","output_sha256":"1c7170f3e63b30df5eb629180f7f08d9b6b3fdc6f604d3cc520d0b6eb8a7d475","redacted":false,"started_ms":906,"ended_ms":912,"status":"ok","bytes":108,"output_truncated":false}
{"schema":"craftsman-harness/trace@1","seq":3,"ts":"2026-10-16T09:31:45.013Z","run_id":"run-0199f3b4a1c8-d03e75b21a96c4f8","thread_id":null,"kind":"round","index":0,"tree_hash":"a4ad120b6f9c67b2bbc1026c5943d9efa9f14bb864990411990e2c3810dfdffd","tree_hash_ms":5}
{"schema":"craftsman-harness/trace@1","seq":4,"ts":"2026-10-16T09:31:45.800Z","run_id":"run-0199f3b4a1c8-d03e75b21a96c4f8","thread_id":null,"kind":"model_call","role":"main","model":"example-model","input_tokens":1296,"output_tokens":21,"usage_source":"reported","finish_reason":"tool_calls","duration_ms":700}
{"schema":"craftsman-harness/trace@1","seq":5,"ts":"2026-10-16T09:31:45.809Z","run_id":"run-0199f3b4a1c8-d03e75b21a96c4f8","thread_id":null,"kind":"tool_call","tool":"grep","agent":"main","round":1,"call_id":"call_2","argument_sha256":"ae3c4784f60c8228d37f7978f007c87de5c146f04887071980c39613d3eca7bd","output_sha256":"1c7170f3e63b30df5eb629180f7f08d9b6b3fdc6f604d3cc520d0b6eb8a7d475","redacted":false,"started_ms":1703,"ended_ms":1709,"status":"ok","bytes":108,"output_truncated":false}
{"schema":"craftsman-harness/trace@1","seq":6,"ts":"2026-10-16T09:31:45.810Z","run_id":"run-0199f3b4a1c8-d03e75b21a96c4f8","thread_id":null,"kind":"round","index":1,"tree_hash":"a4ad120b6f9c67b2bbc1026c5943d9efa9f14bb864990411990e2c3810dfdffd","tree_hash_ms":5}
{"schema":"craftsman-harness/trace@1","seq":7,"ts":"2026-10-16T09:31:46.600Z","run_id":"run-0199f3b4a1c8-d03e75b21a96c4f8","thread_id":null,"kind":"model_call","role":"main","model":"example-model","input_tokens":1337,"output_tokens":21,"usage_source":"reported","finish_reason":"tool_calls","duration_ms":700}
{"schema":"craftsman-harness/trace@1","seq":8,"ts":"2026-10-16T09:31:46.608Z","run_id":"run-0199f3b4a1c8-d03e75b21a96c4f8","thread_id":null,"kind":"tool_call","tool":"grep","agent":"main","round":2,"call_id":"call_3","argument_sha256":"ae3c4784f60c8228d37f7978f007c87de5c146f04887071980c39613d3eca7bd","output_sha256":"1c7170f3e63b30df5eb629180f7f08d9b6b3fdc6f604d3cc520d0b6eb8a7d475","redacted":false,"started_ms":2502,"ended_ms":2508,"status":"ok","bytes":108,"output_truncated":false}
{"schema":"craftsman-harness/trace@1","seq":9,"ts":"2026-10-16T09:31:46.609Z","run_id":"run-0199f3b4a1c8-d03e75b21a96c4f8","thread_id":null,"kind":"round","index":2,"tree_hash":"a4ad120b6f9c67b2bbc1026c5943d9efa9f14bb864990411990e2c3810dfdffd","tree_hash_ms":5}
{"schema":"craftsman-harness/trace@1","seq":10,"ts":"2026-10-16T09:31:46.610Z","run_id":"run-0199f3b4a1c8-d03e75b21a96c4f8","thread_id":null,"kind":"run_end","exit_reason":"max_turn_requests","rounds":3,"wall_ms":2510,"tree_hash":"a4ad120b6f9c67b2bbc1026c5943d9efa9f14bb864990411990e2c3810dfdffd","tree_hash_ms":6}
```

### Other lines

Single lines for the variants the traces above do not show, in this order: a subagent call with estimated usage and the optional token split, a compaction call, a failed model call, a shell command that exited 1, a shell result that the redactor changed, a denied call of a subagent, a cancelled call, a shell command whose output was cut (the exit code survives, the hash of the text without the reminder is added), a `run_start` written by a compaction call with the tree hash skipped, a `run_end` whose end hash timed out, and a line of a kind this revision does not define, which a consumer skips. The hashes on these lines are placeholders.

```jsonl
{"schema":"craftsman-harness/trace@1","seq":21,"ts":"2026-10-16T09:40:12.508Z","run_id":"run-0199f3b4a1c8-d03e75b21a96c4f8","thread_id":null,"kind":"model_call","role":"subagent","model":"example-model","input_tokens":3120,"output_tokens":266,"usage_source":"estimated","finish_reason":"stop","duration_ms":4410,"cached_input_tokens":1024,"reasoning_tokens":80}
{"schema":"craftsman-harness/trace@1","seq":22,"ts":"2026-10-16T09:40:12.508Z","run_id":"run-0199f3b4a1c8-d03e75b21a96c4f8","thread_id":null,"kind":"model_call","role":"compaction","model":"example-model","input_tokens":38810,"output_tokens":1204,"usage_source":"reported","finish_reason":"stop","duration_ms":9275}
{"schema":"craftsman-harness/trace@1","seq":23,"ts":"2026-10-16T09:40:12.508Z","run_id":"run-0199f3b4a1c8-d03e75b21a96c4f8","thread_id":null,"kind":"model_call","role":"main","model":"example-model","input_tokens":null,"output_tokens":null,"usage_source":"missing","finish_reason":null,"duration_ms":120031,"error":true}
{"schema":"craftsman-harness/trace@1","seq":24,"ts":"2026-10-16T09:40:12.508Z","run_id":"run-0199f3b4a1c8-d03e75b21a96c4f8","thread_id":null,"kind":"tool_call","tool":"execute","agent":"main","round":7,"argument_sha256":"46b54e632fb509e603362a785238ddd4c8cecbc6f4c9dfab3715c46f37d34d24","output_sha256":"5c052c90667350f0bd9c2a2126978a585014296a969521a004ef2f2b28cd4e36","redacted":false,"started_ms":61200,"ended_ms":66850,"status":"ok","bytes":33,"output_truncated":false,"exit_code":1}
{"schema":"craftsman-harness/trace@1","seq":25,"ts":"2026-10-16T09:40:12.508Z","run_id":"run-0199f3b4a1c8-d03e75b21a96c4f8","thread_id":null,"kind":"tool_call","tool":"execute","agent":"main","round":7,"argument_sha256":"c88a8c2220e1d104223bd0c39eeeb01e8eb4fd5555830147f817d72837d4fff2","output_sha256":"7dff7e8f0c1128b6f8c717b2fa86f3caf54a64d0047f97a752449d51fb271e6e","redacted":true,"started_ms":66850,"ended_ms":66859,"status":"ok","bytes":19,"output_truncated":false,"exit_code":0}
{"schema":"craftsman-harness/trace@1","seq":26,"ts":"2026-10-16T09:40:12.508Z","run_id":"run-0199f3b4a1c8-d03e75b21a96c4f8","thread_id":null,"kind":"tool_call","tool":"execute","agent":"subagent","round":7,"argument_sha256":"ad1686665270a1d1d4adc015808205829ec2078bbeee89be03d1b3a0245f32a0","output_sha256":"c05da89eb91923ab908e6836f2664a446ebed2944c6e7328446d4625464e7e75","redacted":false,"started_ms":66860,"ended_ms":66861,"status":"denied","bytes":29,"output_truncated":false}
{"schema":"craftsman-harness/trace@1","seq":27,"ts":"2026-10-16T09:40:12.508Z","run_id":"run-0199f3b4a1c8-d03e75b21a96c4f8","thread_id":null,"kind":"tool_call","tool":"execute","agent":"main","round":8,"argument_sha256":"fc48bbd271de2d8370c99fe9a2cb44e4eba40d8dac861212ddef56c44f8ed8c7","output_sha256":"1ef406f01662a6b8efb5b5d11cf48674c13df3671fa44f6c6a583ec7c34c8093","redacted":false,"started_ms":70000,"ended_ms":70002,"status":"cancelled","bytes":32,"output_truncated":false}
{"schema":"craftsman-harness/trace@1","seq":29,"ts":"2026-10-16T09:40:13.100Z","run_id":"run-0199f3b4a1c8-d03e75b21a96c4f8","thread_id":null,"kind":"tool_call","tool":"execute","agent":"main","round":8,"call_id":"call_9","argument_sha256":"b4488a1db0f2a7603ee69a047d32ac4f6aa0fac40f3588e7a9e4949cb4b295d2","output_sha256":"19a74d661cc86a087c62755915c9fe14477c195343425123d76cc35978fbd4ad","output_body_sha256":"2a21fe6d592a19b7de898b50eb53c429608de1a66f3e9f62da19714a770553d1","redacted":false,"started_ms":71000,"ended_ms":74100,"status":"ok","bytes":50120,"output_truncated":true,"exit_code":1}
{"schema":"craftsman-harness/trace@1","seq":0,"ts":"2026-10-16T09:50:00.000Z","run_id":"run-0199f3c000aa-1b2c3d4e5f607182","thread_id":"1792144200-1f4c-0000","kind":"run_start","model":"example-model","harness_profile":"general","cwd_sha256":"db45314eb66a36f744b4173bd01e8831021e00e4ec652ee73eb3edc0a05a6dfd","tools":[],"entry":"one-shot","producer":{"name":"craftsman","version":"0.4.0"},"tree_hash_ms":20003,"tree_hash_skipped":"timeout"}
{"schema":"craftsman-harness/trace@1","seq":44,"ts":"2026-10-16T09:55:00.000Z","run_id":"run-0199f3c000aa-1b2c3d4e5f607182","thread_id":"1792144200-1f4c-0000","kind":"run_end","exit_reason":"end_turn","rounds":9,"wall_ms":290000,"tree_hash_ms":20001,"tree_hash_skipped":"timeout"}
{"schema":"craftsman-harness/trace@1","seq":28,"ts":"2026-10-16T09:40:12.508Z","run_id":"run-0199f3b4a1c8-d03e75b21a96c4f8","thread_id":null,"kind":"budget_warning","limit":"tokens","used":91000}
```

## 10. Validation

`docs/schemas/trace-v1.schema.json` is a JSON Schema (draft 2020-12) for one line. It checks shapes, enumerations, the nullability of the token counts, the fields each tool may carry (`path`, the file hashes and `hunks` on `write_file` and `edit_file` only, `exit_code` on `execute` only), that a failed model call has no usage, that a tree hash is absent only with a reason, and the timestamp format. `docs/schemas/check_trace.py` (Python 3 standard library, run it with `python3 -I`) validates a whole file: it implements the schema keywords the schema uses, so it needs no package, checks that every `ts` names a real calendar date (the pattern alone lets 30 February through), and then checks the file-level rules of 5.6. Every example above, extracted from this document, validates. A writer's tests validate every line it produces, check the file-level rules and run the vectors of `docs/schemas/trace-v1.vectors.json`. The rules that need file contents and so cannot be checked from the trace alone are the hashing rules of section 6 and the line-count bounds of the hunks; a writer's tests check them against real files.
