# The instrumented baseline (R0)

> **Transport retirement (November 11):** references to `serve.rs`,
> `desktop_serve.rs` and their command/protocol surfaces below are historical.
> The owner removed those servers; ACP stdio and its WebSocket bridge remain.
> This does not change the retained token, provider, trace or attribution contracts.

> **Status:** P0 deliverable · plan references: sections 5.9 and 7.2 (P0) of `docs/craftsman-harness-plan.md` · trace format: `docs/trace-schema-v1.md` and `docs/schemas/` on branch `p0/trace-schema` (commit `3181aaf` or later; the copies are identical from `bd8b175` on). `crates/craftsman-rs/tests/data/trace-v1.schema.json` and `trace-v1.vectors.json` here are byte-identical copies of `docs/schemas/trace-v1.schema.json` and `trace-v1.vectors.json` at that commit. They are read only by tests, so updating them does not change the binary.
> **History:** this document and the build replace the first R0 (head `5ec8e3f`, binary `7cddd696…`), which three independent reviews found to need changes before it could be frozen. Those reviews and what was done about each finding are summarised in section 8.

## 1. What it is

R0 is the reference build of the scoreboard that every later build is measured against: the tag `baseline-0.4.0` (commit `0b77980`, crate version 0.4.0) plus **only** a writer for trace schema v1 in `craftsman -m`. It is not a new feature of Craftsman. It exists so that the old behaviour can be measured with the same collector as the new one, and so that the binary never has to be rebuilt later.

- **Source.** Branch `baseline-0.4.0-instrumented`: four code commits on top of the tag (the trace module, the hooks that call it, the tests, and the revision after review), then one commit that updates the two test-data copies to the current spec (read by tests only, not compiled into the binary), then this document alone. Besides this document, thirteen files differ from the tag, all under `crates/craftsman-rs/`: `src/trace.rs` (new), `tests/trace_baseline.rs` (new), `tests/data/trace-v1.schema.json` and `tests/data/trace-v1.vectors.json` (new), and one-line to sixteen-line hooks in `src/agent.rs`, `src/llm.rs`, `src/main.rs`, `src/service_agent.rs`, `src/platform_os.rs`, `src/tools.rs`, `src/lsp.rs`, `src/mcp.rs` and `src/workspace_checkpoint.rs`. `Cargo.toml` and `Cargo.lock` are identical to the tag: no new dependency.
- **Switch.** The environment variable `CRAFTSMAN_TRACE_FILE=<path>`, read by `craftsman -m` only. Unset or empty, the binary does what the tag does (section 4 shows how that was checked). Set, it writes one JSON line per event to that file: `run_start`, then for every round the model calls and tool calls and a `round` line, then `run_end`.
- **Never in the way.** The writer observes. It prints nothing, changes no request, tool result, output or exit code. The file is opened and written by a thread of its own, so the run never waits on it: a path that is a FIFO, a device or a directory, that ends in a symbolic link, or that lies inside the working directory is not used; an existing file is replaced only when it is empty or starts like a trace; a file that cannot be written, or that falls more than 8,192 lines behind, switches the trace off for the rest of the run (the file then ends early). The end of the run waits at most two seconds for the thread. Whenever `CRAFTSMAN_TRACE_FILE` is set and non-empty, it is removed from the environment of the commands the agent runs, of language servers, of MCP servers and of the checkpoint `git` call, so a command that prints its environment sees what it sees in an untraced run. This happens in every mode, not only while a trace is written: in `-m` also when the path was refused, and in the modes listed next.
- **Where it is not active.** The TUI, `serve`, `desktop-serve`, `acp-serve`, `acp-bridge` and `service-agent` ignore the variable for tracing (they write no trace), but still hide it from the child processes above when it is set, which the tag does not do. That is harmless: the variable has no effect outside `-m`, and it is absent in normal use.

## 2. Build it

```sh
git switch baseline-0.4.0-instrumented
CODE=$(git rev-parse HEAD~2)          # the last code commit, c77cf86; the two commits after it change only test data and this document
TARGET=/private/tmp/craftsman-r0-build/target; mkdir -p "$TARGET"
SRC=$(pwd -P); CARGO_DIR=$(cd "${CARGO_HOME:-$HOME/.cargo}" && pwd -P)
CARGO_TARGET_DIR="$TARGET" CRAFTSMAN_BUILD_COMMIT="$CODE" \
RUSTFLAGS="--remap-path-prefix=$TARGET=/target --remap-path-prefix=$SRC=/src --remap-path-prefix=$CARGO_DIR=/cargo" \
  cargo build --release --locked --no-default-features -p craftsman-rs        # $TARGET/release/craftsman
```

`--no-default-features` leaves out the voice feature, whose native dependencies (CMake, ALSA on Linux) a minimal CI image does not have. `CRAFTSMAN_BUILD_COMMIT` is what the trace reports as `run_start.producer.commit`; the remap flags keep the directory names out of the binary. The binary reports `craftsman 0.4.0`, like the tag; the way to tell the two apart is the checksum in section 6.

## 3. Use it

```sh
CRAFTSMAN_TRACE_FILE=/runs/42/trace.jsonl ./craftsman -m "fix the failing test" --auto-approve
```

Point the variable at a file **outside the directory the agent works in**. A path inside it is refused (the model could list the file, which would make the run differ from an untraced one) and the run is simply not traced. The runner chooses the path; that path is how a trace is joined to its task, model and repeat (trace spec, 8.0).

## 4. Check that it is behaviour-neutral

The claim: with `CRAFTSMAN_TRACE_FILE` unset or empty the binary behaves like the tag, and with it set the stdout, stderr, exit code, edited files, the environment the agent's commands see and every request body that a model server receives are the same as without it, apart from the trace file.

### 4.1 What is different when the variable is unset

| Where | Without `CRAFTSMAN_TRACE_FILE` |
|---|---|
| `main.rs`, `run_once` | `Tracer::from_env()` returns `None`. The session is built by the same `agent::build_session` call as in the tag. The three `finish` calls are skipped. |
| `agent.rs`, `StreamingLlmProvider::complete` | One extra `Instant::now()`. The error mapping moved to after an `if let Some(trace)` that is skipped; same value, order and `?`. |
| `agent.rs`, `build_session_ext` | Uses of `extras.trace`, which is `None`: the three providers get `trace: None`; `set_context` and the middleware insertion are skipped; `wrap_subagent_tools(None, tools)` returns `tools` unchanged. |
| `llm.rs` | `TurnResult.usage_reported`, a flag nothing reads unless tracing. |
| `platform_os.rs`, `lsp.rs`, `mcp.rs`, `workspace_checkpoint.rs` | `hide_from_child` checks whether the variable is set and non-empty and, only then, calls `env_remove`. Unset or empty: no change to the child. |
| `tools.rs`, `execute_impl` | `note_exit_code` writes into a task-local slot that exists only inside a traced call; without a slot it does nothing. |
| `service_agent.rs` | `trace: None` in a struct literal. |

With the variable set, the additions are: the file output on a thread of its own; inside a git work tree, one `git ls-files` run and a read of every file it lists at the start and at the end of the run (at most 20 seconds each, then the hash is left out); a read of the target file of every `write_file` and `edit_file` call before and after it; one extra middleware, first in the main agent's chain, and a pass-through wrapper around each subagent tool.

### 4.2 The byte comparison

Run the same scripted scenario against the tag binary and against this binary, with and without the variable, and compare. The model server is a mock (Python 3 standard library only) that answers a `read_file` call, an `edit_file` call and a final answer, picking the reply by the number of tool results already in the request, and stores each request body exactly as received.

Build the control (the plain tag) in its own worktree and target directory, then run the script with the two binaries:

```sh
git worktree add /tmp/r0-plain baseline-0.4.0
(cd /tmp/r0-plain && CARGO_TARGET_DIR=/tmp/r0-plain-target \
   cargo build --release --locked --no-default-features -p craftsman-rs)
mkdir /tmp/r0-check && cd /tmp/r0-check          # put mock.py and neutrality.sh here
sh neutrality.sh /tmp/r0-plain-target/release/craftsman <path to the instrumented craftsman> /tmp/r0-check/work
echo "exit $?"                                    # 0 when everything is the same
git worktree remove --force /tmp/r0-plain
```

`mock.py`:

```python
#!/usr/bin/env python3
# Scripted OpenAI-compatible mock: a read_file call, an edit_file call, a final answer.
# usage: mock.py PORT_FILE REQUEST_DIR WORKSPACE
import http.server, json, os, sys

port_file, out_dir, ws = sys.argv[1:4]
os.makedirs(out_dir, exist_ok=True)
STEPS = [  # the reply is chosen by how many tool results the request already carries
    ("call", "call_read", "read_file", {"file_path": "notes.txt"}),
    ("call", "call_edit", "edit_file",
     {"file_path": ws + "/notes.txt", "old_string": "beta", "new_string": "BETA"}),
    ("text", "all done"),
]
n = 0


class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *args):
        pass

    def do_POST(self):
        global n
        body = self.rfile.read(int(self.headers["Content-Length"]))
        n += 1
        with open(f"{out_dir}/req-{n}.json", "wb") as f:  # the bytes as received
            f.write(body)
        tools = sum(1 for m in json.loads(body)["messages"] if m.get("role") == "tool")
        step = STEPS[min(tools, len(STEPS) - 1)]
        if step[0] == "call":
            _, call_id, name, args = step
            delta = {"tool_calls": [{"index": 0, "id": call_id,
                                     "function": {"name": name, "arguments": json.dumps(args)}}]}
            events = [json.dumps({"choices": [{"delta": delta}]}),
                      '{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}']
        else:
            events = [json.dumps({"choices": [{"delta": {"content": step[1]}}]}),
                      '{"choices":[{"delta":{},"finish_reason":"stop"}]}']
        events.append(json.dumps({"choices": [], "usage":
                                  {"prompt_tokens": 100 + 20 * tools, "completion_tokens": 10 + tools}}))
        out = ("".join(f"data: {e}\n\n" for e in events) + "data: [DONE]\n\n").encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Content-Length", str(len(out)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(out)


server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
with open(port_file, "w") as f:
    f.write(str(server.server_address[1]))
server.serve_forever()
```

`neutrality.sh`:

```sh
#!/bin/sh
# usage: neutrality.sh PLAIN_CRAFTSMAN INSTRUMENTED_CRAFTSMAN WORKDIR   (mock.py sits next to this script)
# Runs the same scripted scenario three times: the tag binary, the instrumented
# binary without CRAFTSMAN_TRACE_FILE, and the instrumented binary with it, then
# compares exit code, stdout, stderr, the edited file and every request body.
set -eu
PLAIN=$1 INSTR=$2
HERE=$(cd "$(dirname "$0")" && pwd -P)
mkdir -p "$3" && W=$(cd "$3" && pwd -P)

run() { # run NAME BINARY [TRACE_FILE]
  name=$1 bin=$2 trace=${3:-}
  rm -rf "$W/$name" "$W/ws" "$W/home"
  mkdir -p "$W/$name" "$W/ws" "$W/home/.craftsman"
  (cd "$W/ws" && git init -q && printf 'alpha\nbeta\ngamma\n' > notes.txt)
  python3 -I "$HERE/mock.py" "$W/$name/port" "$W/$name/req" "$W/ws" &
  mock=$!
  while [ ! -s "$W/$name/port" ]; do kill -0 "$mock" 2>/dev/null || { echo "mock failed to start"; exit 1; }; sleep 0.05; done
  printf '{"base_url":"http://127.0.0.1:%s","api_key":"k","model":"mock-model"}' \
    "$(cat "$W/$name/port")" > "$W/home/.craftsman/config.json"
  set +e
  (cd "$W/ws" && env -i PATH="$PATH" HOME="$W/home" CRAFTSMAN_CONFIG_DIR="$W/home/.craftsman" \
     ${trace:+CRAFTSMAN_TRACE_FILE="$trace"} \
     "$bin" -m 'fix the typo in notes.txt' --auto-approve </dev/null \
     >"$W/$name/stdout" 2>"$W/$name/stderr")
  echo $? > "$W/$name/exit"
  set -e
  kill "$mock"; wait "$mock" 2>/dev/null || true
  cp "$W/ws/notes.txt" "$W/$name/notes.txt"
}

run plain "$PLAIN"
run instr-unset "$INSTR"
rm -f "$W/trace.jsonl"
run instr-set "$INSTR" "$W/trace.jsonl"

status=0
for variant in instr-unset instr-set; do
  for f in exit stdout stderr notes.txt; do
    if cmp -s "$W/plain/$f" "$W/$variant/$f"; then echo "same   $variant $f"
    else echo "DIFFER $variant $f"; status=1; fi
  done
  [ "$(ls "$W/plain/req" | wc -l)" -eq "$(ls "$W/$variant/req" | wc -l)" ] || { echo "DIFFER $variant request count"; status=1; }
  for r in "$W"/plain/req/req-*.json; do
    if cmp -s "$r" "$W/$variant/req/$(basename "$r")"; then echo "same   $variant $(basename "$r") $(shasum -a 256 "$r" | cut -c1-16)"
    else echo "DIFFER $variant $(basename "$r")"; status=1; fi
  done
done
echo "plain: exit $(cat "$W/plain/exit"), $(ls "$W/plain/req" | wc -l) requests, stdout: $(cat "$W/plain/stdout")"
exit $status
```

What it printed for the binary of section 6 (exit status 0):

```text
same   instr-unset exit
same   instr-unset stdout
same   instr-unset stderr
same   instr-unset notes.txt
same   instr-unset req-1.json a6b9115c6d9d5b56
same   instr-unset req-2.json 602636b1ff06a41f
same   instr-unset req-3.json 7e795b24b5ea8134
same   instr-set exit
same   instr-set stdout
same   instr-set stderr
same   instr-set notes.txt
same   instr-set req-1.json a6b9115c6d9d5b56
same   instr-set req-2.json 602636b1ff06a41f
same   instr-set req-3.json 7e795b24b5ea8134
plain: exit 0, 3 requests, stdout: all done
```

The script was also run against a wrapper that appends a line to stdout; it printed `DIFFER` for stdout, so the comparison can fail. The first two request hashes are the same as for the first R0 and do not depend on where the script runs; the third contains the path of the work directory, so its hash does.

### 4.3 What else was run for this build

- **Tests.** `cargo test --locked --no-default-features --workspace -- --test-threads=4`: 572 passed, 1 ignored, 0 failed (113 `deepagents-rs`, 1 `dep_guard`, 440 unit tests of `craftsman-rs`, 7 `loop_test`, 11 `trace_baseline`). The tag has 541 passed and 1 ignored; the 31 new tests are 20 unit tests in `trace.rs` and 11 in `tests/trace_baseline.rs`. Every scenario of the latter runs without and with the variable and compares stdout, stderr, exit code, every request body and the workspace listing: a failing edit, a shell exit code, a result cut at the output budget, a denied call, a delegated agent, HTTP 400, the round limit, a first prompt that compacts before the first request, an agent that prints its environment, a FIFO trace path, a path inside the work tree and a file that is not a trace. The trace of each is validated against the JSON Schema with the `jsonschema` crate and against the file-level rules; the hashes are compared with the tool messages the model was sent. The unit tests also run the shared vectors of the trace spec (41 canonical-JSON and 24 hunk cases made by an implementation that shares no code with the writer; the 24 include the two at the 2000-distance cap added after the re-review, and the 20 `trace.rs` unit tests and the 11 of `tests/trace_baseline.rs` were run again with the updated copies: all passed). The five `unparsed_arguments` vectors (an overflowing number, unpaired surrogate escapes, text that is not JSON) are not run by the unit tests; their hashes were checked against what this binary writes when a mock sends those argument texts and show that a sink that never returns cannot stall the writer.
- **Do the tests fail when behaviour changes?** Four deliberate changes were applied one at a time to the sources, each active only when tracing: the environment scrub removed, truncation of a file that is not a trace allowed, an extra stderr line at the round limit, and the exit-status hand-over removed. Each made a test fail (the first attempt at the second one did not, because the test file was shorter than a trace header; the test was fixed and the change then failed it).
- **A second harness, 714 checks, run against the file of section 6 and a fresh build of the tag.** Fourteen scenarios, each run with the tag binary, with this binary without the variable and with it (stdout, stderr, exit code, final workspace and every request body byte for byte): a read and an edit with and without git; every kind of edit (creation, replacement, insertion, deletion in the middle and at the end, a failed edit, a refused overwrite, an edit under `/memories/`, a shell command that exits 3); the todo reminder; a delegated subagent; HTTP 400; the round limit; a denied call; an agent that prints its environment (it finds `CRAFTSMAN_TRACE_FILE` 0 times, and 1 time with an empty variable, under the tag and under R0 alike); a 120 kB result with exit status 3; a first prompt that compacts first (`run_start` has `tools: []` and the right tree hash); and a mix of edits with a shell edit in between. Then: an unwritable path, a directory, a FIFO nobody reads (the run finishes in under a second), a path inside the workspace, a file that is not a trace (left alone), a previous trace (replaced), a symlink, a repository with a `core.fsmonitor` hook (the hook runs the same number of times as under the tag), `GIT_DIR` naming another repository, and `kill -9`. The traces were checked by code that shares nothing with the writer: canonical JSON with ECMAScript numbers, a Myers diff, the tree hash, and a strict validator written from the JSON Schema (`docs/schemas/check_trace.py`, standard library only; 298 lines accepted). The old R0 binary fails this harness exactly where the reviewers found problems (environment visible, FIFO hang, hook runs 5, 5 and 7 times, trace file inside the workspace, non-trace file truncated, `GIT_DIR` repository hashed).
- **The first harness (496 checks)** was run again unchanged against the same two binaries. Its neutrality checks all pass (429 passed, 0 neutrality failures). It stops at what changed on purpose: its trace checkers read the field `changed_ranges`, which is now `hunks`, and it expects a pre-existing file that is not a trace to be truncated, which the writer now refuses.
- **The metrics from the spec text alone.** A collector written from sections 8.1 to 8.5 of the trace spec (first-try apply, wrong-place edits with mapping to the snapshot frame, tool-error rate, stuck rules 1 to 4) gives the expected numbers on 46 hand-derived and real-trace cases and agrees with an identity-tracking oracle on 4,809 fuzzed edits. It needed five readings the text had not stated; they are now in the spec.
- **Independent recomputation.** For all 82 main-agent tool lines of the proof traces, `bytes`, `output_sha256`, `output_body_sha256`, `output_truncated` and the exit status were recomputed from the tool messages the mock received: 0 mismatches. Hunks equal an independent Myers implementation on every edit.
- **Formatting and lints.** `rustfmt` was run on `trace.rs` and `tests/trace_baseline.rs`; the other touched files were left as the tag has them (the tag is not formatted). `cargo clippy` reports nothing in the lines this branch adds.

## 5. Read a trace

For the traced run of section 4.2 (`jq -c '[.seq, .kind, (.tool // .role // .exit_reason // .index), (.status // .usage_source // ""), (.path // ""), (.hunks // "")]' trace.jsonl`):

```text
[0,"run_start",null,"","",""]
[1,"model_call","main","reported","",""]
[2,"tool_call","read_file","ok","",""]
[3,"round",0,"","",""]
[4,"model_call","main","reported","",""]
[5,"tool_call","edit_file","ok","notes.txt",[[2,1,2,1]]]
[6,"round",1,"","",""]
[7,"model_call","main","reported","",""]
[8,"round",2,"","",""]
[9,"run_end","end_turn","","",""]
```

The fields, the hashing rules and the metrics that the collector derives are in `docs/trace-schema-v1.md`; `python3 -I docs/schemas/check_trace.py trace.jsonl` validates a file. The helpers of `src/trace.rs` (canonical JSON, file and tree hashes, `hunks`) are the reference implementation that P2a reuses.

## 6. This build

| | |
|---|---|
| File | `craftsman-baseline-0.4.0-instrumented-aarch64-apple-darwin` (31,077,888 bytes, Mach-O arm64) |
| SHA-256 | `54abfb882ebf348efb8b37119f50c2a979ee0e1b42e38cd6bdc9dd1187e34e91` |
| Source | branch `baseline-0.4.0-instrumented`, code commit `c77cf869fe26908a8fed58c9d55e1edfbc19f717` (the trace reports it as `producer.commit`); the two commits after it (test-data copies, this document) change nothing that is compiled |
| Command | the one in section 2, with the target directory `/private/tmp/craftsman-r0-build/target` |
| Host | macOS 26.6.2 (build 25G83), Apple M5, Darwin 25.6.0 arm64 |
| Toolchain | rustc 1.99.0 (b940084d7 2026-09-28), cargo 1.99.0, host `aarch64-apple-darwin`; no `rust-toolchain.toml` (the tag has none), so this is the stable toolchain of that machine |
| Control | the tag `0b77980`, built with `cargo build --release --locked --no-default-features -p craftsman-rs` in its own worktree and target directory: SHA-256 `917e0ac4499ecfcfe7dcd084ab2ac509ec904a0a82df174c66e6f92234fcd564` (31 MB; its checksum depends on its build path, which the tag build does not remap) |

**Reproducing the checksum.** With the command of section 2 the build does not depend on the source directory or the user: two clean builds at the same target path, one from the working tree and one from a worktree in a deeper, differently named directory, gave the same file, byte for byte (`54abfb88…`). The target path does matter to one thing only. The linker derives the Mach-O `LC_UUID` from the paths of its object files, and the ad-hoc code signature covers it. Builds made in other target directories differed from this file in exactly those two places (16 bytes at offset 1944 and 30 bytes in the signature) and in nothing else: the string tables are identical, and the checksum with the UUID and the signature blob zeroed (`macho_normalize.py` below) is `73dce5f3494eefb21347e79a075fc31a35a8470e8baee0291394449927263549` for this file and for a build in a different directory. Disabling the UUID (`-Wl,-no_uuid`) is not an option: the system loader refuses a binary without one.

```python
#!/usr/bin/env python3
"""sha256 of a Mach-O file with the two regions that depend on the link-time object paths zeroed:
the LC_UUID value and the code-signature blob (LC_CODE_SIGNATURE). Everything else is hashed as is.
usage: python3 -I macho_normalize.py FILE ..."""
import hashlib, struct, sys

def normalized(path):
    data = bytearray(open(path, 'rb').read())
    magic, = struct.unpack_from('<I', data, 0)
    assert magic == 0xfeedfacf, 'not a 64-bit little-endian Mach-O'
    ncmds, = struct.unpack_from('<I', data, 16)
    off = 32
    zeroed = []
    for _ in range(ncmds):
        cmd, size = struct.unpack_from('<II', data, off)
        if cmd == 0x1b:                      # LC_UUID
            data[off + 8:off + 24] = bytes(16); zeroed.append(('uuid', off + 8, 16))
        elif cmd == 0x1d:                    # LC_CODE_SIGNATURE
            dataoff, datasize = struct.unpack_from('<II', data, off + 8)
            data[dataoff:dataoff + datasize] = bytes(datasize); zeroed.append(('signature', dataoff, datasize))
        off += size
    return hashlib.sha256(bytes(data)).hexdigest(), zeroed

if __name__ == '__main__':
    for p in sys.argv[1:]:
        h, z = normalized(p)
        print(h, p, z)
```

The checksum is of this one file; the plan keeps the binary itself. This document does not publish anything; the plan's release asset and tag are the owner's step. A Linux build for the scoreboard runner has to be made on Linux with the command above, and its checksum recorded next to it.

## 7. Limits

**Fields of the schema that R0 cannot fill** (section 7 of the trace spec):

- `usage_source` is `reported`, or `missing` with null counts when the stream carried no `usage` object. It is never `estimated`: R0 has no usage meter. `cached_input_tokens` and `reasoning_tokens` are never written: the client does not read them. Tokens spent on a provider's internal retries are not counted.
- `redacted` is always `false`: R0 has no redactor, so `output_sha256` covers the raw text the model saw.
- `round.tree_hash` is never written, so the tree-based variants of stuck rules 3 and 4 and the count of state changes that no edit explains are not available; the call-based forms are.
- `tool_call.status` is never `cancelled`: `-m` has no cancel source.
- `exit_code` of a shell call is the real exit status, handed over by the shell implementation, also when the result was cut at the output budget; `output_truncated` says that it was cut. `-1` means the process ended without an exit status, because a signal killed it (trace spec 5.3). The shell tool reads at most 100,000 bytes from each of stdout and stderr and then closes that pipe, and a process that writes to the closed pipe dies of `SIGPIPE`. When that process is the command's shell, the call is recorded as `-1`, whatever status it would have exited with: `yes | head -c 120000; echo end; exit 3` gives `-1`, while `yes | head -c 120000; exit 3` (only `head` dies) keeps the 3, and so does a 70 kB output. Stuck rule 2 counts `-1` as failing, so a very verbose command reads as a failure on R0. If the hand-over were missing, the status would be read from the result text (a final `[exit code N]` line, ignoring a trailing todo reminder; none in a cut result).
- `status` `denied` is recognised from the refusal text of the approval gate, plan mode and the delegated-run gate; a tool whose own error began with the same words would be taken for a denial.
- `call_id` is absent on a subagent's calls, and a line does not say which subagent made it.
- Edits made through `execute` (`sed -i`, `patch`, a script) are not seen by first-try or wrong-place edits; they break the hash chain of a file they touch (the later edits of that file are then not scored).
- `tree_hash` is absent outside a git work tree, when git is missing or fails, or when hashing takes more than 20 seconds; `tree_hash_skipped` says which. The start hash is inside `wall_ms` (`run_start.tree_hash_ms` is its cost); the end hash is not.
- `exit_reason` is one of `end_turn`, `max_turn_requests`, `provider_failure`, `error`. The round limit is the framework default of 50 and is not written to the file.
- If the first prompt is so large that compaction runs before the first main request (over 85% of a configured context window), `run_start` is written by that call with an empty `tools` list (the collector reads that as "start unknown"); it has the tree hash.

**Other limits.**

- Only `craftsman -m` is traced.
- A run that never starts (no model configured, route resolution fails) writes no file. A run that panics or is killed has no `run_end`; the collector counts it as crashed. So does a run whose trace was switched off.
- Each tree hash reads every file the work tree lists, twice per run. On a repository of 100,000 files that took 13 to 15 seconds per hash in an earlier measurement, against the 20 second budget.
- Lines from a background subagent that arrive after `run_end` are dropped, so that `run_end` stays last. Background subagents make the tag itself nondeterministic (its output order varies between runs), so no byte comparison can cover them.
- `path` is made relative by lexical comparison with the working directory the process reports. A model that spells the same directory differently (for example `/var/...` for `/private/var/...` on macOS) gets an absolute path.
- Linux and Windows were not run. The writer thread, the non-blocking open and the symlink check are Unix code paths with Windows fallbacks that were compiled by reading only.

## 8. What the three reviews found and what happened to it

Spec review (9 findings), conformance review (3) and behaviour review (8). In short: the edit record was changed from new-file ranges to hunks so that wrong-place edits can be mapped to the snapshot; first-try apply, the stuck rules, the per-build availability and the external inputs were written down; the exit status, truncation and the reminder in the output hash are now recorded; the environment variable no longer reaches child processes; the writer cannot stall the run; the git call no longer runs repository helpers; the trace path is checked; the checksum reproduces across source directories; the number rule is named and tested by shared vectors; and the integration test compares every scenario against an untraced run. The finding-by-finding record is in the commit messages of this branch and of `p0/trace-schema`. A re-review of the spec and of the writer found no blocker and no writer error. Its remaining findings were fixed in the spec and in this document, not in the code: the meaning of `exit_code` `-1` and the per-pipe read cap (section 7), and the fact that the variable is hidden from child processes in every mode (section 1). The binary of section 6 is unchanged.
