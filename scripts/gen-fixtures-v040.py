#!/usr/bin/env python3
"""Generate the 0.4.0 fixture corpus in crates/craftsman-rs/tests/fixtures/v0.4.0/.

Identity note: the existing corpus is frozen original evidence. Historical
reproduction requires the pre-migration generator and producer/build instructions
from Git history in an isolated working area; do not regenerate it with this
checkout's renamed binary. Current names below describe this source copy.

The corpus is what the baseline binary (tag baseline-0.4.0, crate version
0.4.0) leaves on disk: one threads.db and the JSONL journals under sessions-rs/.
The fixture tests in crates/craftsman-rs/src/fixture_corpus.rs open it with the
current code. The databases must come from the baseline, not from a newer
binary, so the script runs a binary you built from the tag; it never builds one.

Build the baseline once (a throwaway worktree keeps your checkout untouched):

    git worktree add /tmp/craftsman-baseline baseline-0.4.0
    (cd /tmp/craftsman-baseline && cargo build --locked --no-default-features \\
        -p craftsman-rs --bin craftsman)

Then, from the repository root:

    python3 scripts/gen-fixtures-v040.py \\
        --binary /tmp/craftsman-baseline/target/debug/craftsman

    git worktree remove --force /tmp/craftsman-baseline

What it does: starts a tiny OpenAI-compatible streaming (SSE) model server on
127.0.0.1, then runs `craftsman -m <prompt>` once per scenario below with HOME and
CRAFTSMAN_CONFIG_DIR pointing into a scratch directory and a config.json that
points at the mock server. `-m` ignores --session, so each run creates its own
thread, all in the same threads.db:

    plain       a two-message exchange
    write-file  the model calls write_file and edit_file (--auto-approve), so
                the workspace checkpoint tables and the journal are populated
    http-400    the model calls read_file, then the second round fails with
                HTTP 400; the stored thread ends in an unanswered tool call
    tools       one round of three tool calls: write_todos (the stored state
                gets todos), read_file of a file that does not exist (a tool
                message with is_error true) and write_file under /memories (a
                store_files row, not a workspace file)

Afterwards the scratch database is made reproducible and free of the machine it
ran on: thread ids, timestamps (store_files.modified included) and the workspace
path are replaced by fixed values, also inside stored messages and journal lines,
where a tool error quotes the absolute path of the file it could not read (rows,
columns and JSON payloads are otherwise untouched). The file is vacuumed so the
old values are gone from its free pages, and the result is checked for leftovers
of the scratch paths and of any user, home or temp directory. PRAGMA
user_version stays 0, as the baseline wrote it; the script refuses a binary that
stamps it.

On one machine the output is deterministic: repeated runs write the same bytes.
The SQLite header records the version of the library that last wrote the file
(Python's, here), so another Python can change bytes without changing a row: the
journals come out identical under Python 3.9 and 3.15, threads.db differs only
in that header. Regenerate only when the corpus itself has to change, and review
the fixture tests when you do. Work on a copy of threads.db, not the committed
file: opening a WAL-mode database creates -wal and -shm files next to it.

The script replaces only its own files in --out: threads.db (with its -wal and
-shm) and sessions-rs/. It refuses an --out that holds anything else (a mistyped
~/.craftsman, say) unless that is the default fixture directory, and --force is the
explicit way past the refusal. Even then nothing but those files is removed.
scripts/check-gen-fixtures-v040-out.py tests this without a baseline binary. The
fixture directory's README.md lists the SHA-256 of every file the script writes;
update it from the table rows the script prints.

Python 3, standard library only (run with 3.9 and 3.15, on macOS).
"""

import argparse
import hashlib
import http.server
import json
import os
import re
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import threading
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
DEFAULT_OUT = REPO / "crates" / "craftsman-rs" / "tests" / "fixtures" / "v0.4.0"

# What replaces the machine-specific values. The workspace is one directory
# that is wiped and re-seeded before every scenario.
WORKSPACE_PATH = "/workspace"
FIXED_EPOCH = 1_760_000_000
README = "# Fixture workspace\n\nA tiny project.\n"
EXPECTED_VERSION = "craftsman 0.4.0"


# ---------------------------------------------------------------------------
# The mock model: replays scripted /chat/completions responses.
# ---------------------------------------------------------------------------


def sse(chunks):
    """One streamed completion: `data:` events, then the [DONE] sentinel."""
    body = "".join("data: %s\n\n" % json.dumps(chunk) for chunk in chunks)
    return ("%sdata: [DONE]\n\n" % body).encode()


def reply(text):
    """A plain assistant answer."""
    return (
        200,
        "text/event-stream",
        sse(
            [
                {"choices": [{"index": 0, "delta": {"role": "assistant", "content": text}}]},
                {"choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]},
                {"choices": [], "usage": {"prompt_tokens": 40, "completion_tokens": 8}},
            ]
        ),
    )


def tool_calls(*calls):
    """An assistant turn that asks for tools: each call is (id, name, args)."""
    deltas = [
        {
            "index": index,
            "id": call_id,
            "type": "function",
            "function": {"name": name, "arguments": json.dumps(args)},
        }
        for index, (call_id, name, args) in enumerate(calls)
    ]
    return (
        200,
        "text/event-stream",
        sse(
            [
                {"choices": [{"index": 0, "delta": {"role": "assistant", "tool_calls": deltas}}]},
                {"choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}]},
                {"choices": [], "usage": {"prompt_tokens": 40, "completion_tokens": 20}},
            ]
        ),
    )


def http_error(status, message):
    """A rejected request, as an OpenAI-compatible server reports it."""
    body = {"error": {"message": message, "type": "invalid_request_error"}}
    return status, "application/json", json.dumps(body).encode()


class MockModel(object):
    """A /chat/completions endpoint that serves queued responses in order."""

    def __init__(self):
        self.lock = threading.Lock()
        self.queue = []
        self.requests = []
        self.unexpected = 0
        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), _Handler)
        self.server.mock = self
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)

    @property
    def base_url(self):
        return "http://127.0.0.1:%d" % self.server.server_address[1]

    def start(self):
        self.thread.start()

    def stop(self):
        self.server.shutdown()
        self.server.server_close()

    def script(self, responses):
        """Queue the responses for the next run and forget earlier requests."""
        with self.lock:
            self.queue = list(responses)
            self.requests = []
            self.unexpected = 0

    def handle(self, handler, raw):
        with self.lock:
            self.requests.append(
                {
                    "path": handler.path,
                    "authorization": handler.headers.get("Authorization"),
                    "body": json.loads(raw) if raw else None,
                }
            )
            response = self.queue.pop(0) if self.queue else None
            if response is None:
                self.unexpected += 1
        if response is None:
            response = http_error(500, "fixture mock: no scripted response left")
        status, content_type, payload = response
        handler.send_response(status)
        handler.send_header("Content-Type", content_type)
        handler.send_header("Content-Length", str(len(payload)))
        handler.send_header("Connection", "close")
        handler.end_headers()
        handler.wfile.write(payload)
        handler.close_connection = True


class _Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_POST(self):
        length = int(self.headers.get("Content-Length") or 0)
        self.server.mock.handle(self, self.rfile.read(length))

    def log_message(self, *args):
        pass


# ---------------------------------------------------------------------------
# Scenarios. `roles` is the message roles each request to the model must carry,
# which proves a run went the way it was scripted.
# ---------------------------------------------------------------------------

SCENARIOS = [
    {
        "name": "plain",
        "thread": "fixture-plain",
        "prompt": "Say hello.",
        "flags": [],
        "exit": 0,
        "script": [reply("Hello from the fixture model.")],
        "roles": [["system", "user"]],
        "stdout": "Hello from the fixture model.",
        "checkpoints": 2,
        "workspace_files": 0,
        "memory_files": [],
    },
    {
        "name": "write-file",
        "thread": "fixture-write-file",
        "prompt": "Create notes.txt and mention it in README.md.",
        "flags": ["--auto-approve"],
        "exit": 0,
        "script": [
            tool_calls(
                (
                    "call_1",
                    "write_file",
                    {"file_path": "notes.txt", "content": "hello from the fixture\n"},
                ),
                (
                    "call_2",
                    "edit_file",
                    {
                        "file_path": "README.md",
                        "old_string": "A tiny project.",
                        "new_string": "A tiny project. See notes.txt.",
                    },
                ),
            ),
            reply("Created notes.txt and mentioned it in README.md."),
        ],
        "roles": [
            ["system", "user"],
            ["system", "user", "assistant", "tool", "tool"],
        ],
        "stdout": "Created notes.txt and mentioned it in README.md.",
        "checkpoints": 3,
        "workspace_files": 2,
        "memory_files": [],
    },
    {
        "name": "http-400",
        "thread": "fixture-http-400",
        "prompt": "Read README.md.",
        "flags": [],
        "exit": 1,
        "script": [
            tool_calls(("call_1", "read_file", {"file_path": "README.md"})),
            http_error(400, "fixture: the request was rejected"),
        ],
        "roles": [["system", "user"], ["system", "user", "assistant", "tool"]],
        "stdout": "",
        "checkpoints": 1,
        "workspace_files": 0,
        "memory_files": [],
    },
    # Appended last, so the threads above keep exactly the rows they always had.
    {
        "name": "tools",
        "thread": "fixture-tools",
        "prompt": "Plan the work, read missing.txt and remember the outcome.",
        "flags": ["--auto-approve"],
        "exit": 0,
        "script": [
            tool_calls(
                (
                    "call_1",
                    "write_todos",
                    {
                        "todos": [
                            {"content": "step one", "status": "in_progress"},
                            {"content": "step two", "status": "pending"},
                        ]
                    },
                ),
                # Fails: the stored tool message has is_error true and, as the
                # baseline wrote it, the absolute path of the scratch workspace.
                ("call_2", "read_file", {"file_path": "missing.txt"}),
                # Under /memories, so the row lands in store_files, not the workspace.
                (
                    "call_3",
                    "write_file",
                    {"file_path": "/memories/fixture.md", "content": "remember me\n"},
                ),
            ),
            reply("Planned two steps; missing.txt does not exist; noted in memory."),
        ],
        "roles": [
            ["system", "user"],
            ["system", "user", "assistant", "tool", "tool", "tool"],
        ],
        "stdout": "noted in memory",
        "checkpoints": 3,
        "workspace_files": 0,
        "memory_files": ["/memories/fixture.md"],
    },
]


# ---------------------------------------------------------------------------
# Running the baseline.
# ---------------------------------------------------------------------------


def fail(message):
    sys.exit("gen-fixtures-v040: %s" % message)


def reset_workspace(workspace):
    shutil.rmtree(str(workspace), ignore_errors=True)
    workspace.mkdir(parents=True)
    (workspace / "README.md").write_text(README)


def thread_ids(db_path):
    if not db_path.exists():
        return set()
    con = sqlite3.connect(str(db_path))
    try:
        return {row[0] for row in con.execute("SELECT id FROM threads")}
    finally:
        con.close()


def run_scenario(binary, scenario, mock, env, workspace, db_path):
    """Run one `craftsman -m` turn; return the id of the thread it created."""
    reset_workspace(workspace)
    mock.script(scenario["script"])
    before = thread_ids(db_path)
    try:
        done = subprocess.run(
            [binary, "-m", scenario["prompt"]] + scenario["flags"],
            cwd=str(workspace),
            env=env,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=120,
        )
    except subprocess.TimeoutExpired:
        fail("%s: the binary did not finish within 120 s" % scenario["name"])
    stdout = done.stdout.decode("utf-8", "replace")
    stderr = done.stderr.decode("utf-8", "replace")

    def explain(problem):
        fail(
            "%s: %s\n--- stdout ---\n%s\n--- stderr ---\n%s"
            % (scenario["name"], problem, stdout, stderr)
        )

    if done.returncode != scenario["exit"]:
        explain("exit code %d, expected %d" % (done.returncode, scenario["exit"]))
    if scenario["stdout"] not in stdout:
        explain("stdout lacks %r" % scenario["stdout"])
    if mock.unexpected or mock.queue:
        explain(
            "model requests did not match the script (%d unexpected, %d unserved)"
            % (mock.unexpected, len(mock.queue))
        )
    roles = [[m["role"] for m in r["body"]["messages"]] for r in mock.requests]
    if roles != scenario["roles"]:
        explain("request roles were %r, expected %r" % (roles, scenario["roles"]))
    for request in mock.requests:
        if request["path"] != "/chat/completions":
            explain("unexpected request path %s" % request["path"])
        if request["authorization"] != "Bearer fixture-key":
            explain("unexpected Authorization header")
    created = thread_ids(db_path) - before
    if len(created) != 1:
        explain("expected exactly one new thread, found %r" % sorted(created))
    return created.pop()


# ---------------------------------------------------------------------------
# Making the scratch output reproducible and free of the machine it ran on.
# ---------------------------------------------------------------------------


def normalize_database(db_path, threads, machine_paths):
    """Replace ids, timestamps and the workspace path; keep everything else.

    `threads` is a list of (raw thread id, scenario) in run order.
    `machine_paths` are the spellings of the scratch workspace; wherever one
    occurs inside a stored message (a tool error quotes the absolute path of the
    file it could not read) it becomes WORKSPACE_PATH.
    """
    # isolation_level=None: transactions are explicit, so VACUUM can run.
    con = sqlite3.connect(str(db_path), isolation_level=None)
    try:
        if con.execute("PRAGMA user_version").fetchone()[0] != 0:
            fail(
                "the binary stamped PRAGMA user_version; the fixtures must come from "
                "the 0.4.0 baseline (tag baseline-0.4.0), which does not"
            )
        mode = con.execute("PRAGMA journal_mode").fetchone()[0]
        con.execute("BEGIN IMMEDIATE")
        checkpoint_number = 0
        for index, (raw, scenario) in enumerate(threads):
            thread = scenario["thread"]
            base = FIXED_EPOCH + 3600 * index
            con.execute(
                "UPDATE threads SET id = ?, created = ?, updated = ?, workspace = ? WHERE id = ?",
                (thread, base, base + 10, WORKSPACE_PATH, raw),
            )
            con.execute(
                "UPDATE thread_project_contexts SET thread_id = ?, path = ? WHERE thread_id = ?",
                (thread, WORKSPACE_PATH, raw),
            )
            con.execute("UPDATE checkpoints SET thread_id = ? WHERE thread_id = ?", (thread, raw))
            old_ids = [
                row[0]
                for row in con.execute(
                    "SELECT id FROM workspace_checkpoints WHERE thread_id = ? ORDER BY created, id",
                    (raw,),
                )
            ]
            for old_id in old_ids:
                checkpoint_number += 1
                new_id = "wc-fixture-%04d" % checkpoint_number
                con.execute(
                    "UPDATE workspace_checkpoint_files SET checkpoint_id = ? WHERE checkpoint_id = ?",
                    (new_id, old_id),
                )
                con.execute(
                    "UPDATE workspace_checkpoints SET id = ?, thread_id = ?, created = ? WHERE id = ?",
                    (new_id, thread, base + 5, old_id),
                )
        for machine_path in machine_paths:
            con.execute(
                "UPDATE checkpoints SET state_json = replace(state_json, ?1, ?2), "
                "suspension_json = replace(suspension_json, ?1, ?2)",
                (machine_path, WORKSPACE_PATH),
            )

        # The memory files the scenarios wrote, with the clock replaced.
        memory_files = {}
        for index, (raw, scenario) in enumerate(threads):
            for memory_path in scenario["memory_files"]:
                memory_files[memory_path] = FIXED_EPOCH + 3600 * index + 3
        stored = [row[0] for row in con.execute("SELECT path FROM store_files ORDER BY path")]
        if stored != sorted(memory_files):
            fail("store_files holds %r, the scenarios declare %r" % (stored, sorted(memory_files)))
        for memory_path, modified in memory_files.items():
            con.execute("UPDATE store_files SET modified = ? WHERE path = ?", (modified, memory_path))
        con.execute("COMMIT")

        # Every row must still point at a parent, and the counts must be the
        # ones the scenarios promise.
        if con.execute("PRAGMA foreign_key_check").fetchall():
            fail("foreign keys broke while normalizing ids")
        for raw, scenario in threads:
            thread = scenario["thread"]
            counts = {
                "checkpoints": con.execute(
                    "SELECT COUNT(*) FROM checkpoints WHERE thread_id = ?", (thread,)
                ).fetchone()[0],
                "workspace_files": con.execute(
                    "SELECT COUNT(*) FROM workspace_checkpoint_files f "
                    "JOIN workspace_checkpoints w ON w.id = f.checkpoint_id "
                    "WHERE w.thread_id = ?",
                    (thread,),
                ).fetchone()[0],
                "contexts": con.execute(
                    "SELECT COUNT(*) FROM thread_project_contexts WHERE thread_id = ?",
                    (thread,),
                ).fetchone()[0],
            }
            expected = {
                "checkpoints": scenario["checkpoints"],
                "workspace_files": scenario["workspace_files"],
                "contexts": 1,
            }
            if counts != expected:
                fail("%s: stored rows %r, expected %r" % (scenario["name"], counts, expected))

        # Drop the old values from free pages and fold the WAL into the file.
        con.execute("VACUUM")
        con.execute("PRAGMA wal_checkpoint(TRUNCATE)")
        if con.execute("PRAGMA integrity_check").fetchone()[0] != "ok":
            fail("integrity_check failed after normalizing")
        if con.execute("PRAGMA user_version").fetchone()[0] != 0:
            fail("PRAGMA user_version changed while normalizing")
        return mode
    finally:
        con.close()


def normalize_journals(sessions_dir, threads, out_dir, machine_paths):
    """Copy each journal under its fixed thread id with fixed timestamps."""
    out_dir.mkdir(parents=True)
    for index, (raw, scenario) in enumerate(threads):
        source = sessions_dir / ("%s.jsonl" % raw)
        base = FIXED_EPOCH + 3600 * index
        lines = []
        for number, line in enumerate(source.read_bytes().splitlines()):
            for machine_path in machine_paths:
                line = line.replace(machine_path.encode(), WORKSPACE_PATH.encode())
            stamped, changed = re.subn(rb'"ts":\d+\}$', b'"ts":%d}' % (base + 1 + number), line)
            if changed != 1 or json.loads(stamped)["ts"] != base + 1 + number:
                fail("%s: journal line %d has no trailing ts field" % (scenario["name"], number))
            lines.append(stamped + b"\n")
        (out_dir / ("%s.jsonl" % scenario["thread"])).write_bytes(b"".join(lines))


def assert_clean(paths, needles):
    """Fail if any file still contains something from the scratch machine."""
    for path in paths:
        data = path.read_bytes()
        for needle in needles:
            if needle and needle.encode() in data:
                fail("%s still contains %r" % (path.name, needle))


# ---------------------------------------------------------------------------
# Writing the result. The generator replaces its own files in --out and never
# deletes anything else.
# ---------------------------------------------------------------------------

DATABASE_FILES = ("threads.db", "threads.db-wal", "threads.db-shm")
JOURNAL_DIR = "sessions-rs"


def journal_names():
    return {"%s.jsonl" % scenario["thread"] for scenario in SCENARIOS}


def is_plain_file(path):
    return path.is_file() and not path.is_symlink()


def foreign_entries(out):
    """What is in `out` that the generator did not write, as relative names.

    The generator's own files are threads.db (with its -wal and -shm), and
    sessions-rs/ holding one <thread>.jsonl per scenario. A symlink or a
    directory in their place is foreign too.
    """
    if not out.exists():
        return []
    if not out.is_dir():
        return [str(out)]
    found = []
    for entry in sorted(out.iterdir()):
        if entry.name in DATABASE_FILES and is_plain_file(entry):
            continue
        if entry.name == JOURNAL_DIR and entry.is_dir() and not entry.is_symlink():
            for inner in sorted(entry.iterdir()):
                if not (inner.name in journal_names() and is_plain_file(inner)):
                    found.append("%s/%s" % (JOURNAL_DIR, inner.name))
            continue
        found.append(entry.name)
    return found


def check_out_dir(out, force):
    """Refuse an --out that holds files the generator did not write.

    Allowed without --force: the default fixture directory, a directory that
    does not exist or is empty, and one that holds only the generator's own
    files from an earlier run.
    """
    out = Path(out).resolve()
    if force or out == DEFAULT_OUT.resolve():
        return
    found = foreign_entries(out)
    if found:
        shown = ", ".join(found[:5]) + (", ..." if len(found) > 5 else "")
        fail(
            "refusing to write into %s: it holds files this script did not write (%s). "
            "Use an empty or missing directory, or pass --force to replace its %s/ "
            "and %s anyway (nothing else in it is touched)."
            % (out, shown, JOURNAL_DIR, "/".join(DATABASE_FILES))
        )


def install_output(staged, out, force):
    """Replace the generator's files in `out` with the staged ones.

    Only threads.db, its -wal and -shm, and the sessions-rs/ directory are
    removed; `check_out_dir` has already refused anything else unless forced,
    and it runs again here because the run takes a while.
    """
    check_out_dir(out, force)
    out.mkdir(parents=True, exist_ok=True)
    for name in DATABASE_FILES:
        target = out / name
        if target.is_symlink() or target.is_file():
            target.unlink()
        elif target.exists():
            fail("%s is a directory; not touching it" % target)
    journals = out / JOURNAL_DIR
    if journals.is_symlink() or journals.is_file():
        journals.unlink()
    elif journals.is_dir():
        shutil.rmtree(str(journals))
    shutil.copyfile(str(staged / "threads.db"), str(out / "threads.db"))
    shutil.copytree(str(staged / JOURNAL_DIR), str(out / JOURNAL_DIR))


# ---------------------------------------------------------------------------


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument(
        "--binary", required=True, help="the `craftsman` binary built from the baseline-0.4.0 tag"
    )
    parser.add_argument(
        "--out",
        default=str(DEFAULT_OUT),
        help="fixture directory to (re)write (default: %(default)s). Refused unless it is "
        "the default, empty or missing, or holds only files this script wrote earlier",
    )
    parser.add_argument(
        "--force",
        action="store_true",
        help="write into an --out that holds other files too; still only threads.db* "
        "and sessions-rs/ are replaced",
    )
    args = parser.parse_args()
    binary = os.path.abspath(args.binary)
    out = Path(args.out).resolve()
    check_out_dir(out, args.force)  # before any work, and before anything can be deleted
    if not (os.path.isfile(binary) and os.access(binary, os.X_OK)):
        fail("%s is not an executable file" % binary)
    version = subprocess.run(
        [binary, "--version"], stdout=subprocess.PIPE, universal_newlines=True
    ).stdout.strip()
    if version != EXPECTED_VERSION:
        fail("%s reports %r, expected %r" % (binary, version, EXPECTED_VERSION))

    os.umask(0o022)  # stable file modes in the workspace checkpoint rows
    mock = MockModel()
    mock.start()
    try:
        with tempfile.TemporaryDirectory(prefix="craftsman-fixtures-") as scratch:
            root = Path(os.path.realpath(scratch))
            home = root / "home"
            config_dir = home / ".craftsman"
            workspace = root / "workspace"
            config_dir.mkdir(parents=True)
            (config_dir / "config.json").write_text(
                json.dumps(
                    {"base_url": mock.base_url, "api_key": "fixture-key", "model": "fixture-model"}
                )
            )
            env = {
                "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
                "HOME": str(home),
                "CRAFTSMAN_CONFIG_DIR": str(config_dir),
                "NO_PROXY": "127.0.0.1,localhost",
                "no_proxy": "127.0.0.1,localhost",
                # Keep git from finding a repository above the scratch workspace.
                "GIT_CEILING_DIRECTORIES": str(root),
                "TERM": "dumb",
            }
            db_path = config_dir / "threads.db"

            threads = []
            for scenario in SCENARIOS:
                raw = run_scenario(binary, scenario, mock, env, workspace, db_path)
                threads.append((raw, scenario))
                print("ran %-10s -> thread %s" % (scenario["name"], raw))

            # The workspace as the binary reports it (resolved) and as spelled here.
            machine_paths = sorted({str(workspace), os.path.join(scratch, "workspace")}, key=len, reverse=True)
            mode = normalize_database(db_path, threads, machine_paths)
            for sidecar in ("-wal", "-shm"):
                leftover = Path(str(db_path) + sidecar)
                if leftover.exists():
                    leftover.unlink()

            staged = root / "staged"
            normalize_journals(config_dir / "sessions-rs", threads, staged / "sessions-rs", machine_paths)
            shutil.copyfile(str(db_path), str(staged / "threads.db"))

            staged_files = [staged / "threads.db"] + sorted((staged / "sessions-rs").iterdir())
            needles = [str(root), scratch, root.name]
            real_home = str(Path.home())
            if len(real_home) > 1:
                needles.append(real_home)
            needles += [raw for raw, _ in threads]
            # Whatever the scratch spelling was, no path into a user, temp or
            # home directory may survive in the committed data.
            needles += ["/Users/", "/home/", "/private/var/", "/var/folders/", "/tmp/"]
            assert_clean(staged_files, needles)

            install_output(staged, out, args.force)
    finally:
        mock.stop()

    print("\nwrote %s (journal mode %s, PRAGMA user_version 0)" % (out, mode))
    print("rows for the table in README.md (a test compares it with the files):")
    for path in [out / "threads.db"] + sorted((out / "sessions-rs").iterdir()):
        print("| `%s` | %d | `%s` |" % (path.relative_to(out).as_posix(), path.stat().st_size, sha256(path)))


if __name__ == "__main__":
    main()
