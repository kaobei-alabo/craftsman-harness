"""Read-only separately owned collector for one independently requested fixture."""
import sys

if __name__ == "__main__" and sys.argv[1:] != ["--owned-native-collector-v3"]:
    raise SystemExit(0)

import base64
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import secrets
import threading

if "_native" not in globals():
    import native_probe_wrapper
    _native = vars(native_probe_wrapper)
    _native_source_sha256 = hashlib.sha256(Path(native_probe_wrapper.__file__).read_bytes()).hexdigest()

COMMAND = "python3 -I -B /driver/client.py --owned-native-relay-v3"
SOURCE = "33a80d99431fd707b148485cb714f365b1946803"
IMAGE = "sha256:6123321bf37c5641bc6ff4b4f79848f8b8e752fbc78a2f4361406162943cb38b"
BINARY = "08542be11e7d207b43c4e0622a6f24180eaa2de9d65b247599f7f70660b56585"


def canonical(value):
    return json.dumps(value, sort_keys=True, ensure_ascii=False, separators=(",", ":"), allow_nan=False).encode()


def require(condition, code):
    if not condition:
        raise ValueError(code)


def digest(value):
    require(isinstance(value, str) and len(value) == 64 and value != "0" * 64
            and all(char in "0123456789abcdef" for char in value), "collector_digest_refused")


class Collector:
    def __init__(self, bootstrap, root, private, emit, cwd=None):
        require(set(bootstrap) == {"source_commit", "image_id", "binary_sha256", "scenario_sha256",
                    "run_sha256", "native_wrapper_sha256"} and bootstrap["source_commit"] == SOURCE
                and bootstrap["image_id"] == IMAGE and bootstrap["binary_sha256"] == BINARY,
                "collector_frozen_binding_refused")
        for name in ("scenario_sha256", "run_sha256", "native_wrapper_sha256"):
            digest(bootstrap[name])
        if "_native_source_sha256" in globals():
            require(bootstrap["native_wrapper_sha256"] == _native_source_sha256, "collector_source_binding_refused")
        self.bootstrap = dict(bootstrap)
        self.root, self.private, self.emit = root, private, emit
        self.cwd = cwd
        self.lock = threading.Lock()
        self.release = threading.Event()
        self.state = "new"
        self.binding = None
        self.call_sha256 = hashlib.sha256(canonical(
            {"id": "scenario-effect", "name": "execute", "args": {"command": COMMAND}})).hexdigest()

    def arm(self, value):
        require(set(value) == {"op", "request_ordinal", "request_sha256", "call_sha256", "snapshot_id",
                    "session_sha256", "checkpoint_seq", "assistant_index", "call_ordinal"}
                and value["op"] == "arm" and value["call_sha256"] == self.call_sha256
                and type(value["request_ordinal"]) is int and 0 <= value["request_ordinal"] < 24
                and type(value["checkpoint_seq"]) is int and 0 <= value["checkpoint_seq"] <= 1000
                and type(value["assistant_index"]) is int and 0 <= value["assistant_index"] < 4096
                and value["call_ordinal"] == 0 and type(value["call_ordinal"]) is int,
                "collector_intended_request_refused")
        for name in ("request_sha256", "call_sha256", "session_sha256"):
            digest(value[name])
        require(isinstance(value["snapshot_id"], str) and len(value["snapshot_id"]) == 32
                and all(char in "0123456789abcdef" for char in value["snapshot_id"]), "collector_snapshot_instance_refused")
        with self.lock:
            require(self.state == "new", "collector_single_request_already_closed")
            # Reserve before any child work, including failed requests. This
            # slot and its forwarding slot are outside candidate access.
            fd = os.open("request-slot", os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600, dir_fd=self.private)
            try:
                data = canonical(value)
                require(os.write(fd, data) == len(data), "collector_request_slot_partial")
                os.fsync(fd)
            finally:
                os.close(fd)
            self.binding = {**self.bootstrap, **{key: item for key, item in value.items() if key != "op"}}
            self.state = "armed"
        self.emit({"kind": "collector_intent_reserved", "binding": self.binding,
                   "outer_runtime_approval_or_dispatch_authority": False})

    def allow_fixture(self, value):
        require(set(value) == {"op", "call_sha256"} and value["op"] == "allow_fixture"
                and value["call_sha256"] == self.call_sha256, "collector_release_refused")
        with self.lock:
            require(self.state == "armed" and not self.release.is_set(), "collector_duplicate_release")
            self.release.set()
        self.emit({"kind": "collector_fixture_released", "call_sha256": self.call_sha256,
                   "outer_runtime_approval_or_dispatch_authority": False})

    def trigger(self, forward):
        require(self.release.wait(5), "collector_unreleased_fixture_refused")
        with self.lock:
            require(self.state == "armed", "collector_duplicate_consumption_refused")
            self.state = "closed_incomplete"
        source = witness = None
        facts = None
        try:
            forwarding = os.open("forward-slot", os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600, dir_fd=self.private)
            os.close(forwarding)
            source, snapshot, expected_version = _native["verified_source"](self.root)
            witness = os.open("native_probe_witness.json", os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600, dir_fd=self.private)
            facts, stdout, stderr = _native["collect_child"](
                self.root, source, snapshot, expected_version, secrets.token_hex(16), binding=self.binding, cwd=self.cwd)
            payload = canonical({"version": 3, "kind": "independent_fixed_fixture_bytes",
                "stdout": base64.b64encode(stdout).decode("ascii"), "stderr": base64.b64encode(stderr).decode("ascii"),
                "exit": facts["actual_child_exit"], "instance": facts["child_instance"],
                "source_sha256": facts["child_source_sha256"]})
            require(len(payload) <= 196608, "collector_forward_payload_bound")
            # Reserve consumption first; a partial/disconnected write closes
            # the instance. No retry, second publication or child replay.
            require(forward(payload) == len(payload), "collector_forward_partial")
            completed = {**facts, "kind": "native_child_completed",
                "classification": "independently_requested_fixed_fixture_child",
                "collector_binding_sha256": hashlib.sha256(canonical(self.binding)).hexdigest(),
                "producer_source_sha256": self.bootstrap["native_wrapper_sha256"],
                "local_forward_write_returned": True, "forward_bytes": len(payload),
                "forward_sha256": hashlib.sha256(payload).hexdigest(), "receiver_exactly_once_verified": False}
            encoded = canonical(completed)
            require(len(encoded) <= 2048, "collector_witness_bound")
            position = 0
            while position < len(encoded):
                count = os.write(witness, encoded[position:])
                require(count > 0, "collector_witness_write_failed")
                position += count
            os.fsync(witness)
            self.state = "completed"
            self.emit(completed)
        except (ValueError, OSError) as error:
            self.state = "closed_incomplete"
            observed = error.facts if isinstance(error, _native["NativeFailure"]) else (facts or {})
            self.emit({"kind": "native_child_incomplete", "complete": False,
                "classification": "independently_requested_fixed_fixture_child",
                "instance_closed": True, "reason": str(error) if isinstance(error, ValueError) else "collector_io_refused",
                "observed": {key: value for key, value in observed.items() if key != "kind"},
                "receiver_exactly_once_verified": False})
            raise
        finally:
            for fd in (source, witness):
                if fd is not None:
                    os.close(fd)


def main():
    bootstrap_raw = sys.stdin.buffer.readline(4097)
    require(len(bootstrap_raw) <= 4096, "collector_bootstrap_bound")
    bootstrap = json.loads(bootstrap_raw)
    require(hashlib.sha256(Path("/usr/local/bin/craftsman").read_bytes()).hexdigest() == BINARY,
            "collector_actual_binary_binding_refused")
    root = os.open("/workspace", os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    private = os.open("/private", os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    output_lock = threading.Lock()
    def emit(value):
        data = canonical(value)
        require(len(data) <= 8192, "collector_metadata_bound")
        with output_lock:
            require(sys.stdout.buffer.write(data + b"\n") == len(data) + 1, "collector_observation_partial")
            sys.stdout.buffer.flush()
    collector = Collector(bootstrap, root, private, emit)
    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"
        def log_message(self, *_):
            pass
        def do_POST(self):
            try:
                require(self.path == "/fixed-probe" and self.headers.get("Content-Length") == "0"
                        and self.headers.get("Transfer-Encoding") is None, "collector_trigger_shape_refused")
                self.connection.settimeout(5)
                def forward(payload):
                    self.send_response(200)
                    self.send_header("Content-Type", "application/json")
                    self.send_header("Content-Length", str(len(payload)))
                    self.send_header("Connection", "close")
                    self.end_headers()
                    self.close_connection = True
                    count = self.wfile.write(payload)
                    self.wfile.flush()
                    return count
                collector.trigger(forward)
            except (ValueError, OSError):
                self.close_connection = True
                # A fixed refusal does not export task/provider/error bodies.
    server = ThreadingHTTPServer(("0.0.0.0", 9877), Handler)
    server.daemon_threads = False
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    emit({"kind": "collector_ready", "source_commit": SOURCE, "image_id": IMAGE,
          "actual_binary_sha256": BINARY,
          "actual_python_binary_sha256": hashlib.sha256(Path(sys.executable).read_bytes()).hexdigest(),
          "collector_source_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest()})
    try:
        for raw in sys.stdin.buffer:
            require(len(raw) <= 4096, "collector_control_bound")
            value = json.loads(raw)
            if value.get("op") == "arm":
                collector.arm(value)
            elif value.get("op") == "allow_fixture":
                collector.allow_fixture(value)
            else:
                raise ValueError("collector_control_refused")
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)
        os.close(root)
        os.close(private)


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError):
        raise SystemExit(2)
