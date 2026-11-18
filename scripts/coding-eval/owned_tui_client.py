"""Single marked client mounted into a disposable agent container.

It contains no grader, reference solution or signing key. Raw terminal and
checkpoint text stays in memory; only bounded actual metadata leaves stdout.
"""
import sys


def _valid_marker(arguments):
    if arguments in (["--owned-tui-client-v1"], ["--owned-native-relay-v3"]):
        return True
    if len(arguments) != 4 or arguments[0] != "--owned-tui-keeper-v1":
        return False
    if not all(1 <= len(value) <= 9 and all(char in "0123456789" for char in value) for value in arguments[1:3]):
        return False
    if int(arguments[1]) <= 2 or int(arguments[2]) <= 2 or arguments[1] == arguments[2]:
        return False
    if arguments[3] == "new":
        return True
    parts = arguments[3].split("-")
    return (len(parts) == 3 and 1 <= len(parts[0]) <= 20 and all(char in "0123456789" for char in parts[0])
            and 1 <= len(parts[1]) <= 8 and all(char in "0123456789abcdef" for char in parts[1])
            and len(parts[2]) == 4 and all(char in "0123456789abcdef" for char in parts[2]))


if __name__ == "__main__" and not _valid_marker(sys.argv[1:]):
    raise SystemExit(0)

import base64
from collections import Counter
import hashlib
import json
import os
from pathlib import Path
import select
import signal
import secrets
import sqlite3
import stat
import struct
import subprocess
import time


class ClientFailure(ValueError):
    pass


def require(condition, code):
    if not condition:
        raise ClientFailure(code)


def canonical(value):
    return json.dumps(value, sort_keys=True, ensure_ascii=False, separators=(",", ":"), allow_nan=False).encode()


def sha(value):
    return hashlib.sha256(canonical(value)).hexdigest()


class TerminalFacts:
    """Finite OSC/cursor decoder. Printable terminal text is never retained."""
    def __init__(self, rows=60, columns=120):
        self.pending = bytearray()
        self.digest = hashlib.sha256()
        self.bytes = 0
        self.records = []
        self.rows, self.columns = rows, columns
        self.row = self.column = 0
        self.state = None
        self.kind = None
        self.stopping = False
        self.queries = 0

    def feed(self, chunk):
        self.bytes += len(chunk)
        require(self.bytes <= 8 * 1024 * 1024, "terminal_output_bound")
        self.digest.update(chunk)
        self.pending.extend(chunk)
        replies = []
        while self.pending:
            if self.pending[0] != 27:
                byte = self.pending.pop(0)
                if byte == 13:
                    self.column = 0
                elif byte == 10:
                    self.row = min(self.rows - 1, self.row + 1)
                elif 32 <= byte < 127:
                    self.column = min(self.columns - 1, self.column + 1)
                continue
            if len(self.pending) < 2:
                break
            if self.pending[1] == 93:
                end = self.pending.find(b"\x1b\\", 2)
                if end < 0:
                    require(len(self.pending) <= 8192, "terminal_control_bound")
                    break
                frame = bytes(self.pending[2:end])
                del self.pending[:end + 2]
                if frame.startswith(b"7501;"):
                    require(len(frame) <= 4096, "status_frame_bound")
                    self._status(frame[5:])
                continue
            if self.pending[1] == 91:
                end = next((i for i in range(2, len(self.pending)) if 64 <= self.pending[i] <= 126), None)
                if end is None:
                    require(len(self.pending) <= 4096, "terminal_control_bound")
                    break
                value = bytes(self.pending[2:end])
                code = chr(self.pending[end])
                del self.pending[:end + 1]
                if code == "n" and value == b"6":
                    self.queries += 1
                    require(self.queries <= 128, "cursor_query_bound")
                    replies.append(("\x1b[" + str(self.row + 1) + ";" + str(self.column + 1) + "R").encode())
                elif code in {"H", "f"}:
                    parts = value.split(b";")
                    try:
                        self.row = min(self.rows - 1, max(0, int(parts[0] or b"1") - 1))
                        self.column = min(self.columns - 1, max(0, int(parts[1] or b"1") - 1)) if len(parts) > 1 else 0
                    except ValueError:
                        raise ClientFailure("invalid_cursor_frame") from None
                continue
            del self.pending[:2]
        return replies

    def _status(self, frame):
        try:
            values = dict(part.split("=", 1) for part in frame.decode("ascii").split(":"))
            require(set(values) <= {"state", "app", "kind", "id", "msg"}, "invalid_status_fields")
            require(values.get("app") == "craftsman", "foreign_status_app")
            require(values.get("state") in {"idle", "working", "blocked", "done", "error", "clear"}, "invalid_status_state")
            require(values.get("kind") in {None, "permission", "question"}, "invalid_status_kind")
            stopping = values.get("msg") == base64.b64encode(b"Stopping").decode()
            record = {"state": values["state"], "kind": values.get("kind"), "is_root": "id" not in values,
                      "stopping_observed": stopping}
            require(len(self.records) < 256, "status_record_bound")
            self.records.append(record)
            if record["is_root"]:
                self.state, self.kind = record["state"], record["kind"]
                self.stopping = self.stopping or stopping
        except (ValueError, UnicodeError):
            raise ClientFailure("invalid_status_frame") from None


def safe_state(thread, sequence, state, once_command):
    if "_capture_checkpoint" in globals():
        observation = _capture_checkpoint({"thread": thread, "seq": sequence, "state": state}, {"probe_command": once_command})
        pending = [group["call_sha256"] for group in observation["group_positions"] if group["result_index"] is None]
        return observation, pending, observation["durable_in_progress"]
    require(len(canonical(state)) <= 8 * 1024 * 1024, "checkpoint_state_bound")
    messages = state.get("messages")
    require(isinstance(messages, list) and len(messages) <= 4096, "checkpoint_messages_bound")
    groups = Counter()
    pending_calls = []
    selected_count = 0
    selected_results = []
    for index, message in enumerate(messages):
        require(isinstance(message, dict), "invalid_checkpoint_message")
        if message.get("role") != "assistant":
            continue
        following = []
        for item in messages[index + 1:]:
            if item.get("role") != "tool":
                break
            following.append(item)
        for call in message.get("tool_calls", []):
            results = [item for item in following if item.get("tool_call_id") == call.get("id")]
            require(len(results) <= 1, "duplicate_checkpoint_result")
            if results:
                groups[sha({"call": call, "result": results[0]})] += 1
            else:
                pending_calls.append(sha(call))
            if once_command is not None and call.get("name") == "execute" and call.get("args", {}).get("command") == once_command:
                selected_count += 1
                selected_results.extend(hashlib.sha256(item["content"].encode()).hexdigest() for item in results)
    observation = {"version": 1, "session_sha256": hashlib.sha256(thread.encode()).hexdigest(), "seq": sequence,
        "message_count": len(messages), "messages_sha256": sha(messages), "files_sha256": sha(state.get("files", {})),
        "todos_sha256": sha(state.get("todos", [])), "compactions": state.get("custom", {}).get("_craftsman_context_v1", {}).get("compactions"),
        "accepted_input_sha256": [hashlib.sha256(item["content"].encode()).hexdigest() for item in messages if item.get("role") == "user"],
        "completed_groups": [{"sha256": value, "count": groups[value]} for value in sorted(groups)],
        "selected_once_command_count": selected_count, "selected_once_result_sha256": selected_results}
    return observation, pending_calls, state.get("custom", {}).get("_craftsman_durable_turn_v1", {}).get("in_progress")


class RetainedStore:
    def __init__(self, exchange=None):
        self.directory = Path("/home/eval/.craftsman")
        self.directory_fd = os.open(self.directory, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        self.file_fd = None
        self.identity = None
        self.thread = None
        self.input_receipts = []
        self.owner_generation = 0
        self.group_witnesses = []
        self.group_keys = set()
        self.exchange = exchange
        self.cached = None
        self.cached_signature = None
        self.epoch = 0
        self.last_members = None
        self.last_generation = None

    def checkpoint(self, once_command, stage):
        # No sqlite3 connection ever opens the live store. Independently held
        # retained bytes are queried only by the external private-copy reader.
        if "_snapshot_acquire" in globals():
            acquire, signature = _snapshot_acquire, _snapshot_signature
        else:
            from sqlite_snapshot import acquire, signature
        require(self.exchange is not None, "private_snapshot_reader_required")
        if self.file_fd is None:
            self.file_fd = os.open("threads.db", os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=self.directory_fd)
            info = os.fstat(self.file_fd)
            require(stat.S_ISREG(info.st_mode) and info.st_nlink == 1, "nonregular_checkpoint_store")
            self.identity = (info.st_dev, info.st_ino)
        named_directory = os.stat(self.directory, follow_symlinks=False)
        actual_directory = os.fstat(self.directory_fd)
        require((named_directory.st_dev, named_directory.st_ino) == (actual_directory.st_dev, actual_directory.st_ino),
                "checkpoint_directory_rebound")
        current = signature(self.directory_fd, self.file_fd)
        if self.cached is not None and self.cached_signature == current:
            return self.cached
        # Every acquisition opens the newly observed optional WAL generation
        # relative to the retained root; old handles are never silently reused.
        membership = {name: None if value is None else tuple(value[:2]) for name, value in current.items()}
        acquired = acquire(self.directory_fd, self.file_fd, named_root=self.directory)
        generation = (membership, acquired["pair"]["wal_generation_sha256"])
        if generation != self.last_generation:
            self.epoch += 1
            self.last_generation = generation
            self.last_members = membership
        result = self.exchange(acquired, stage, self.epoch, self.thread)
        self.thread = result.pop("thread")
        observation = result["checkpoint"]
        result["retained_store_identity"] = list(self.identity)
        self.cached, self.cached_signature = result, acquired["versions"]
        for group in observation.get("group_positions", []):
            key = (self.owner_generation, group["assistant_index"], group["call_ordinal"], group["call_sha256"])
            if key not in self.group_keys:
                require(len(self.group_witnesses) < 512, "group_witness_bound")
                self.group_keys.add(key)
                self.group_witnesses.append({"owner_generation": self.owner_generation, "first_seen_checkpoint_seq": observation["seq"],
                    "assistant_index": group["assistant_index"], "call_ordinal": group["call_ordinal"], "call_sha256": group["call_sha256"],
                    "source": "first_seen_stored_row", "dispatch_observed": False})
        return result

    def hold_store_writer(self):
        raise ClientFailure("live_writer_connection_unverified_quarantined")

    def release_store_writer(self):
        raise ClientFailure("live_writer_connection_unverified_quarantined")

    def close(self):
        if self.file_fd is not None:
            os.close(self.file_fd)
            self.file_fd = None
        os.close(self.directory_fd)


def keeper():
    import fcntl
    import termios
    require(len(sys.argv) == 5 and all(value.isdecimal() for value in sys.argv[2:4]), "invalid_keeper_arguments")
    status_fd, control_fd = int(sys.argv[2]), int(sys.argv[3])
    require(status_fd > 2 and control_fd > 2 and status_fd != control_fd, "invalid_keeper_descriptors")
    fcntl.ioctl(0, termios.TIOCSCTTY, 0)
    os.write(status_fd, b'{"kind":"keeper_ready"}\n')
    require(os.read(control_fd, 1) == b"S", "keeper_start_refused")
    argv = ["/usr/local/bin/craftsman", "--no-mouse"]
    if sys.argv[4] != "new":
        require(len(sys.argv[4].encode()) <= 512, "invalid_session_argument")
        argv.extend(["--session", sys.argv[4]])
    process = subprocess.Popen(argv, stdin=0, stdout=1, stderr=2, close_fds=True)
    def forward(signum, _frame):
        if process.poll() is None:
            process.send_signal(signum)
    for signum in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP):
        signal.signal(signum, forward)
    while process.poll() is None:
        if select.select([control_fd], [], [], 0.1)[0]:
            command = os.read(control_fd, 1)
            if command == b"K":
                process.kill()
            elif command in (b"T", b""):
                process.send_signal(signal.SIGTERM)
    code = process.wait()
    os.write(status_fd, (json.dumps({"kind": "child_exit", "code": code}) + "\n").encode())
    os.read(control_fd, 1)
    os.close(status_fd)
    os.close(control_fd)
    raise SystemExit(code if code >= 0 else 1)


class TuiOwner:
    def __init__(self, store, once_command, allowed_calls):
        import fcntl
        import pty
        import termios
        self.termios = termios
        self.store, self.once_command = store, once_command
        store.owner_generation += 1
        self.allowed_calls = set(allowed_calls)
        self.approved = set()
        self.master, self.slave = pty.openpty()
        fcntl.ioctl(self.slave, termios.TIOCSWINSZ, struct.pack("HHHH", 60, 120, 0, 0))
        self.modes_before = termios.tcgetattr(self.slave)
        self.status_read, status_write = os.pipe()
        control_read, self.control_write = os.pipe()
        self.facts = TerminalFacts()
        self.status_pending = bytearray()
        self.child_exit = None
        self.ready = False
        self.sent = None
        self.current_input_sha256 = None
        self.input_status_start = 0
        self.capture_after_owned_return = False
        self.last_return = None
        self.approval_status_start = None
        env = {"HOME": "/home/eval", "CRAFTSMAN_CONFIG_DIR": "/home/eval/.craftsman", "TERM": "xterm-256color", "PATH": os.defpath}
        self.process = subprocess.Popen([sys.executable, "-I", "-B", str(Path(__file__).resolve()),
            "--owned-tui-keeper-v1", str(status_write), str(control_read), store.thread or "new"],
            cwd="/workspace", env=env, stdin=self.slave, stdout=self.slave, stderr=self.slave,
            start_new_session=True, pass_fds=(status_write, control_read))
        os.close(status_write)
        os.close(control_read)
        deadline = time.monotonic() + 5
        while not self.ready and time.monotonic() < deadline:
            self.pump(0.05)
        require(self.ready, "keeper_readiness_failed")
        os.write(self.control_write, b"S")

    def pump(self, timeout=0):
        readable = select.select([self.master, self.status_read], [], [], timeout)[0]
        if self.master in readable:
            try:
                data = os.read(self.master, 65536)
            except OSError:
                data = b""
            for reply in self.facts.feed(data):
                os.write(self.master, reply)
        if self.status_read in readable:
            self.status_pending.extend(os.read(self.status_read, 4096))
            require(len(self.status_pending) <= 4096, "keeper_status_bound")
            while b"\n" in self.status_pending:
                line, _, rest = self.status_pending.partition(b"\n")
                self.status_pending = bytearray(rest)
                value = json.loads(line)
                if value["kind"] == "keeper_ready":
                    self.ready = True
                elif value["kind"] == "child_exit":
                    require(self.child_exit is None and type(value["code"]) is int, "invalid_child_return")
                    self.child_exit = value["code"]
                else:
                    raise ClientFailure("invalid_keeper_status")

    def input(self, text, capture_after_owned_return=False):
        require(isinstance(text, str) and len(text.encode()) <= 98304, "input_bound")
        require(type(capture_after_owned_return) is bool, "invalid_capture_stage_selection")
        data = b"\x1b[200~" + text.encode() + b"\x1b[201~\r"
        if not text.startswith("/"):
            self.capture_after_owned_return = capture_after_owned_return
            self.current_input_sha256 = hashlib.sha256(text.encode()).hexdigest()
            self.input_status_start = len(self.facts.records)
            prior = {} if self.store.cached is None else self.store.cached["checkpoint"]
            self.sent = {"sha256": hashlib.sha256(text.encode()).hexdigest(), "before_count": prior.get("message_count", 0),
                         "before_revision": prior.get("owned_input", {}).get("last_revision", 0),
                         "after_status_index": len(self.facts.records)}
        position = 0
        while position < len(data):
            position += os.write(self.master, data[position:position + 4096])
            self.pump()

    def poll(self):
        self.pump(0.02)
        result = {"root_state": self.facts.state, "root_kind": self.facts.kind,
                  "stopping_observed": self.facts.stopping, "child_exit": self.child_exit,
                  "approval_count": len(self.approved), "terminal_bytes": self.facts.bytes,
                  "submitted_input_sha256": self.current_input_sha256,
                  "observed_current_input_activity": any(record["is_root"] and record["state"] in {"working", "blocked"}
                      for record in self.facts.records[self.input_status_start:])}
        try:
            stage = {"root_state": self.facts.state, "root_kind": self.facts.kind,
                     "owner_generation": self.store.owner_generation}
            safe_stage = self.checkpoint_stage_available()
            if self.sent is not None and len(self.facts.records) <= self.sent["after_status_index"]:
                safe_stage = False
            if self.capture_after_owned_return and self.facts.state != "blocked":
                safe_stage = False
            checkpoint = self.store.checkpoint(self.once_command, stage) if safe_stage else None
            self.pump()
            require(not safe_stage or (stage["root_state"], stage["root_kind"]) == (self.facts.state, self.facts.kind),
                    "snapshot_scoped_quiescent_stage_changed")
            if checkpoint is not None:
                result.update(checkpoint)
        except (OSError, sqlite3.Error):
            checkpoint = None
        except ClientFailure as error:
            if self.store.input_receipts or str(error) not in {"checkpoint_missing", "checkpoint_initializing", "owned_input_provenance_missing", "owned_original_input_missing", "owned_input_stamp_missing"}:
                raise
            checkpoint = None
        if checkpoint is not None:
            self.observe_input(checkpoint)
        result["owned_input_receipts"] = self.store.input_receipts
        if self.facts.state == "blocked" and self.facts.kind == "permission" and checkpoint is not None:
            pending = checkpoint["pending_call_sha256"]
            if pending:
                value = pending[0]
                require(value in self.allowed_calls, "unreviewed_approval_refused")
                if value not in self.approved:
                    # The actual modal is initialized at choice zero; this
                    # driver never sends selection-down or an Always action.
                    require(os.write(self.master, b"\r") == 1, "approval_key_write_incomplete")
                    self.approved.add(value)
                    self.approval_status_start = len(self.facts.records)
                    result["approval_once_observed"] = value
        return result

    def checkpoint_stage_available(self):
        # An Enter key may release real work before the corresponding OSC
        # update reaches the PTY. The old blocked display is not quiescence.
        # Wait for an actual later root transition before another acquisition.
        if self.approval_status_start is not None:
            changed = any(record["is_root"] and record["state"] != "blocked"
                          for record in self.facts.records[self.approval_status_start:])
            if not changed:
                return False
            self.approval_status_start = None
        return self.facts.state in {"idle", "done", "error"} or self.facts.state == "blocked" and self.facts.kind == "permission"

    def observe_input(self, checkpoint):
        if self.sent is not None:
            observed = checkpoint["checkpoint"]
            owned = observed.get("owned_input", {})
            if owned.get("last_sha256") == self.sent["sha256"] and owned.get("last_revision", 0) > self.sent["before_revision"]:
                matches = [item["index"] for item in observed["user_content_sha256"]
                           if item["sha256"] == self.sent["sha256"] and item["index"] >= self.sent["before_count"]]
                require(len(matches) == 1, "owned_input_instance_not_identified")
                pin = next((record for record in owned["retained_inputs"] if record["revision"] == owned["last_revision"]
                            and record["sha256"] == self.sent["sha256"]), None)
                require(pin is not None, "current_step_saved_pin_missing")
                require(len(self.store.input_receipts) < 14, "owned_input_receipt_bound")
                self.store.input_receipts.append({"sha256": self.sent["sha256"], "revision": owned["last_revision"],
                    "checkpoint_seq": observed["seq"], "session_sha256": observed["session_sha256"], "scope_sha256": owned["scope_sha256"],
                    "message_index": matches[0], "original_bytes": pin["original_bytes"], "source": "actual_owned_context_stamp"})
                self.sent = None

    def closed_capture(self):
        require(self.capture_after_owned_return and self.last_return is not None
                and self.process.poll() == 0 and self.last_return["child_exit"] == 0
                and self.last_return["keeper_exit"] == 0 and self.last_return["modes_restored"] is True
                and self.last_return["root_state"] == "done", "actual_completed_owned_return_required")
        stage = {"root_state": self.last_return["root_state"], "root_kind": self.facts.kind,
                 "owner_generation": self.store.owner_generation, "after_owned_return": True}
        value = self.store.checkpoint(self.once_command, stage)
        self.observe_input(value)
        return {**value, "root_state": self.last_return["root_state"], "root_kind": self.facts.kind,
                "owned_input_receipts": self.store.input_receipts, "after_owned_return": True}

    def quit(self):
        self.input("/quit")
        deadline = time.monotonic() + 30
        while self.child_exit is None and time.monotonic() < deadline:
            self.pump(0.05)
        require(self.child_exit is not None, "owned_shutdown_did_not_return")
        after = self.termios.tcgetattr(self.slave)
        restored = after == self.modes_before
        os.write(self.control_write, b"A")
        keeper_exit = self.process.wait(timeout=5)
        result = {"child_exit": self.child_exit, "keeper_exit": keeper_exit, "modes_restored": restored,
                  "root_state": self.facts.state, "terminal_bytes": self.facts.bytes,
                  "terminal_sha256": self.facts.digest.hexdigest(), "status_records": self.facts.records,
                  "approvals_once": len(self.approved)}
        require(self.child_exit == 0 and keeper_exit == 0 and restored, "owned_exit_or_restore_failed")
        for fd in (self.master, self.slave, self.status_read, self.control_write):
            os.close(fd)
        self.last_return = result
        return result

    def fail_cleanup(self):
        if self.process.poll() is None:
            os.write(self.control_write, b"T")
            deadline = time.monotonic() + 30
            while self.child_exit is None and time.monotonic() < deadline:
                self.pump(0.05)
            if self.child_exit is None:
                os.write(self.control_write, b"K")
                deadline = time.monotonic() + 5
                while self.child_exit is None and time.monotonic() < deadline:
                    self.pump(0.05)
            os.write(self.control_write, b"A")
            self.process.wait(timeout=5)


def main():
    require(os.name == "posix", "owned_pty_requires_unix")
    bootstrap = json.loads(sys.stdin.buffer.readline(1024 * 1024 + 1))
    config = bootstrap["config"]
    provider = config["providers"]["openai"]
    require(provider["base_url"] == "http://gateway:8765/v1" and provider["api_key"] == "synthetic-agent-key", "non_synthetic_route_refused")
    require(config["permission_mode"] == "manual" and config["auto_approve"] is False, "non_manual_configuration_refused")
    directory = Path("/home/eval/.craftsman")
    directory.mkdir(parents=True)
    path = directory / "config.json"
    with path.open("x") as file:
        json.dump(config, file)
    path.chmod(0o600)
    config_digest = hashlib.sha256(path.read_bytes()).hexdigest()
    settings_digest = sha(provider["models"]["gpt-6.1-sol"]["settings"])
    binary_digest = hashlib.sha256(Path("/usr/local/bin/craftsman").read_bytes()).hexdigest()
    owner = None
    returned_owner = None
    def emit(value):
        data = canonical(value)
        require(len(data) <= 1024 * 1024, "client_metadata_bound")
        sys.stdout.buffer.write(data + b"\n")
        sys.stdout.buffer.flush()
    def exchange(acquisition, stage, epoch, thread):
        identity = secrets.token_hex(16)
        emit({"kind": "private_snapshot_begin", "snapshot_id": identity, **stage,
              **{key: value for key, value in acquisition.items() if key != "files"},
              "source_epoch": epoch, "thread": thread})
        for name, data in acquisition["files"].items():
            for offset in range(0, len(data), 49152):
                emit({"kind": "private_snapshot_chunk", "snapshot_id": identity, "name": name, "offset": offset,
                      "data": base64.b64encode(data[offset:offset + 49152]).decode("ascii")})
        emit({"kind": "private_snapshot_end", "snapshot_id": identity})
        reply_raw = sys.stdin.buffer.readline(1024 * 1024 + 1)
        require(len(reply_raw) <= 1024 * 1024, "snapshot_reply_bound")
        reply = json.loads(reply_raw)
        require(reply.get("op") == "private_snapshot_reply" and reply.get("snapshot_id") == identity,
                "private_snapshot_reply_instance_changed")
        require(reply.get("ok") is True, reply.get("reason", "private_snapshot_reader_refused"))
        return {key: value for key, value in reply.items() if key not in {"op", "ok", "snapshot_id"}}
    store = RetainedStore(exchange)
    try:
        emit({"kind": "client_ready", "binary_sha256": binary_digest, "config_sha256": config_digest,
              "actual_config_model_settings_sha256": settings_digest})
        for line in sys.stdin.buffer:
            require(len(line) <= 1024 * 1024, "client_control_bound")
            request = json.loads(line)
            op = request.get("op")
            if op == "start":
                require(owner is None, "duplicate_owner_start")
                owner = TuiOwner(store, bootstrap.get("once_command"), bootstrap["allowed_calls"])
                emit({"kind": "owner_started"})
            elif op == "input":
                require(owner is not None, "owner_missing")
                owner.input(request["text"], request.get("capture_after_owned_return", False))
                emit({"kind": "input_sent", "input_sha256": hashlib.sha256(request["text"].encode()).hexdigest()})
            elif op == "poll":
                require(owner is not None, "owner_missing")
                value = owner.poll()
                if "checkpoint" in value:
                    observed = value["checkpoint"]
                    value["checkpoint"] = {"seq": observed["seq"], "session_sha256": observed["session_sha256"],
                                           "owned_input": observed["owned_input"]}
                value.pop("pending_call_sha256", None)
                emit({"kind": "poll", **value})
            elif op == "capture":
                require(owner is not None and request.get("stage") in {"before", "after", "reopened", "final"}, "invalid_capture_stage")
                value = owner.poll()
                allow_error = request.get("allow_error_observation") is True and value.get("root_state") == "error"
                require((value.get("durable_in_progress") is False or allow_error) and not value.get("suspension_present"), "unfinished_capture_refused")
                emit({"kind": "checkpoint_captured", "stage": request["stage"], **value})
            elif op == "capture_closed":
                require(owner is None and returned_owner is not None and request.get("stage") == "after",
                        "closed_capture_without_actual_owned_return")
                value = returned_owner.closed_capture()
                require(value.get("durable_in_progress") is False and not value.get("suspension_present"),
                        "unfinished_closed_capture_refused")
                emit({"kind": "checkpoint_captured", "stage": "after", **value})
            elif op == "cancel":
                require(owner is not None, "owner_missing")
                os.write(owner.master, b"\x03")
                emit({"kind": "cancel_key_sent"})
            elif op == "quit":
                require(owner is not None, "owner_missing")
                result = owner.quit()
                returned_owner = owner
                owner = None
                emit({"kind": "owner_returned", **result})
            elif op == "hold_store_writer":
                store.hold_store_writer()
            elif op == "release_store_writer":
                store.release_store_writer()
            elif op == "finish":
                require(owner is None, "live_owner_on_finish")
                require(hashlib.sha256(path.read_bytes()).hexdigest() == config_digest, "isolated_configuration_changed")
                emit({"kind": "client_finished", "config_sha256": config_digest, "owned_input_receipts": store.input_receipts,
                      "stored_group_witnesses": store.group_witnesses})
                return
            else:
                raise ClientFailure("unknown_client_control")
    finally:
        if owner is not None:
            owner.fail_cleanup()
        store.close()


def relay():
    """One local forwarding attempt from the independently owned collector.

    This fixed route conveys synthetic fixture bytes. It is not outer runtime
    approval/dispatch authority and does not claim receiver exactly-once delivery.
    """
    import urllib.request
    class NoRedirect(urllib.request.HTTPRedirectHandler):
        def redirect_request(self, *_):
            raise ClientFailure("fixture_relay_redirect_refused")
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())
    request = urllib.request.Request("http://collector:9877/fixed-probe", data=b"", method="POST")
    with opener.open(request, timeout=25) as response:
        payload = response.read(196609)
    require(len(payload) <= 196608, "fixture_relay_payload_bound")
    value = json.loads(payload)
    require(set(value) == {"version", "kind", "stdout", "stderr", "exit", "instance", "source_sha256"}
            and value["version"] == 3 and value["kind"] == "independent_fixed_fixture_bytes"
            and type(value["exit"]) is int and value["source_sha256"] == "4ca33bc733f3498c6817bf3a628bb0875b7b7cb17a3a35bc4e69d1c7d9ea6c87",
            "fixture_relay_contract_refused")
    stdout, stderr = (base64.b64decode(value[name], validate=True) for name in ("stdout", "stderr"))
    require(len(stdout) <= 65536 and len(stderr) <= 65536, "fixture_relay_stream_bound")
    require(sys.stdout.buffer.write(stdout) == len(stdout), "fixture_relay_stdout_partial")
    sys.stdout.buffer.flush()
    require(sys.stderr.buffer.write(stderr) == len(stderr), "fixture_relay_stderr_partial")
    sys.stderr.buffer.flush()
    return value["exit"] if value["exit"] >= 0 else 128 - value["exit"]


if __name__ == "__main__":
    try:
        if sys.argv[1:] == ["--owned-native-relay-v3"]:
            raise SystemExit(relay())
        elif sys.argv[1] == "--owned-tui-keeper-v1":
            keeper()
        else:
            require(sys.argv[1:] == ["--owned-tui-client-v1"], "invalid_client_marker")
            main()
    except (ClientFailure, OSError, sqlite3.Error, KeyError, TypeError, ValueError, subprocess.TimeoutExpired) as error:
        reason = str(error) if isinstance(error, ClientFailure) or type(error).__name__ == "SnapshotFailure" else type(error).__name__
        sys.stdout.write(json.dumps({"kind": "client_failed", "reason": reason}) + "\n")
        sys.stdout.flush()
        raise SystemExit(2)
