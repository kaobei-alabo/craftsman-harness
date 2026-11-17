"""Private driver, gateway-boundary and independent-review rejection checks."""
import sys

if len(sys.argv) > 1 and sys.argv[1] == "--synthetic-gateway-fixture-v2":
    if len(sys.argv) != 2:
        raise SystemExit(0)
    import json
    from pathlib import Path
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    from gateway import serve
    serve(json.loads(sys.stdin.buffer.read(1024 * 1024)), ("127.0.0.1", 0))
    raise SystemExit(0)

from copy import deepcopy
import hashlib
import hmac
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import io
import json
import os
from pathlib import Path
import queue
import subprocess
import tempfile
import threading
import time
import unittest
from unittest import mock
sys.path.insert(0, str(Path(__file__).resolve().parent))
import grade_long_session as grader
from test_long_session import CORPUS, EXTENSION, synthetic_capture
import test_preflight_latest as preflight_fixtures
import preflight_latest as preflight
import wiring_baseline_fixture as wiring_baseline
from preflight_continuation import Refused
from gateway_observations import GatewayObservations, ObservationFailure
from owned_tui_client import TerminalFacts
from owned_tui_receipt import AuthenticatedRun, require_authenticated, seal_actual_driver
import owned_tui_receipt as receipt_source
from preflight_continuation import canonical
from coding_eval_anchors import ORIGINAL_CORPUS_SHA256


def raw_fixture(scenario):
    values = []
    observe = grader.safe_checkpoint_observation
    def capture(value, selected):
        values.append(deepcopy(value))
        return observe(value, selected)
    with mock.patch.object(grader, "safe_checkpoint_observation", side_effect=capture):
        task, metadata, files = synthetic_capture(scenario)
    return task, metadata, files, dict(zip(grader.STAGES, values))


class ReviewCounterexamples(unittest.TestCase):
    def rejects(self, scenario, task, capture, files, raw=None):
        with self.assertRaises(Refused):
            if raw is not None:
                capture["checkpoints"] = {stage: grader.safe_checkpoint_observation(value, scenario) for stage, value in raw.items()}
            grader.grade_capture(scenario, task, capture, files, EXTENSION)

    def test_accepted_original_updates_lost_after_interruption(self):
        scenario = EXTENSION["scenarios"][1]
        task, capture, files, raw = raw_fixture(scenario)
        updates = set(grader.updates(scenario))
        for stage in ("after", "reopened", "final"):
            raw[stage]["state"]["messages"] = [message for message in raw[stage]["state"]["messages"] if message.get("content") not in updates]
        self.rejects(scenario, task, capture, files, raw)

    def test_in_progress_final_checkpoint_is_rejected(self):
        scenario = EXTENSION["scenarios"][1]
        task, capture, files, raw = raw_fixture(scenario)
        for value in raw.values():
            value["state"]["custom"]["_craftsman_durable_turn_v1"]["in_progress"] = True
        self.rejects(scenario, task, capture, files, raw)

    def test_duplicate_same_group_call_ids_cannot_share_one_result(self):
        scenario = EXTENSION["scenarios"][1]
        task, capture, files, raw = raw_fixture(scenario)
        for value in raw.values():
            message = value["state"]["messages"][1]
            message["tool_calls"].append(deepcopy(message["tool_calls"][0]))
        self.rejects(scenario, task, capture, files, raw)

    def test_invalid_role_and_non_boolean_error_are_rejected(self):
        scenario = EXTENSION["scenarios"][1]
        for mutation in ("role", "error", "content"):
            task, capture, files, raw = raw_fixture(scenario)
            for value in raw.values():
                if mutation == "role":value["state"]["messages"].append({"role": "invalid-role", "content": ""})
                elif mutation == "error":value["state"]["messages"][2]["is_error"] = "false"
                else:value["state"]["messages"][0]["content"] = None
            with self.subTest(mutation=mutation):self.rejects(scenario, task, capture, files, raw)

    def test_contradictory_zero_message_count_is_rejected(self):
        scenario = EXTENSION["scenarios"][1]
        task, capture, files, _ = raw_fixture(scenario)
        for value in capture["checkpoints"].values():value["message_count"] = 0
        self.rejects(scenario, task, capture, files)

    def test_forbidden_direct_reread_of_changed_probe_is_rejected(self):
        scenario = EXTENSION["scenarios"][2]
        task, capture, files, raw = raw_fixture(scenario)
        raw["final"]["state"]["messages"].extend([
            {"role": "assistant", "content": "", "tool_calls": [{"id": "fresh-read", "name": "read_file", "args": {"file_path": "probe_input.txt"}}]},
            {"role": "tool", "tool_call_id": "fresh-read", "content": "changed", "is_error": False}])
        self.rejects(scenario, task, capture, files, raw)

    def test_summary_only_compaction_without_kept_tail_is_rejected(self):
        scenario = EXTENSION["scenarios"][0]
        task, capture, files, raw = raw_fixture(scenario)
        raw["after"]["state"]["messages"] = [{"role": "user", "content": "synthetic summary only"}]
        raw["reopened"] = deepcopy(raw["after"])
        self.rejects(scenario, task, capture, files, raw)

    def test_caller_runtime_pivot_cannot_replace_original_corpus_anchor(self):
        fixture = preflight_fixtures.Cases()
        fixture.setUp()
        try:
            fixture.corpus["tasks"][0]["held_out"] = "print('weakened')\n"
            fixture.write(fixture.corpus_path, fixture.corpus)
            subprocess.run(["git", "-C", str(fixture.root), "add", str(fixture.corpus_path.relative_to(fixture.root))], check=True)
            subprocess.run(["git", "-C", str(fixture.root), "-c", "core.hooksPath=/dev/null", "-c", "commit.gpgsign=false", "-c", "user.name=Fixture", "-c", "user.email=fixture@localhost", "commit", "--quiet", "--no-gpg-sign", "-m", "Evaluation-only pivot"], check=True)
            commit = preflight.git(fixture.root, "rev-parse", "HEAD").decode().strip()
            fixture.plan["source"].update(commit=commit, tree=preflight.git(fixture.root, "rev-parse", "HEAD^{tree}").decode().strip())
            fixture.extension.update(source_commit=commit, source_tree=fixture.plan["source"]["tree"], base_corpus_sha256=fixture.hash(fixture.corpus_path))
            for scenario in fixture.extension["scenarios"]:
                task = next(task for task in fixture.corpus["tasks"] if task["id"] == scenario["base_task"])
                scenario["original_task_sha256"] = preflight.canonical_sha256(task)
            fixture.refresh()
            with self.assertRaisesRegex(Refused, "source_corpus_differs"):fixture.prepare()
        finally:fixture.tearDown()

    def test_untracked_automatic_build_input_is_not_attested(self):
        fixture = preflight_fixtures.Cases()
        fixture.setUp()
        try:
            (fixture.runtime_file.parent / "build.rs").write_text("fn main() {}\n")
            with self.assertRaisesRegex(Refused, "untracked_automatic_build_input_refused"):fixture.prepare()
        finally:fixture.tearDown()

    def test_original_task_and_setup_cannot_disappear_before_transition(self):
        scenario = EXTENSION["scenarios"][1]
        task, capture, files, raw = raw_fixture(scenario)
        original = {task["prompt"], scenario["setup_prompt"]}
        for value in raw.values():
            value["state"]["messages"] = [message for message in value["state"]["messages"] if message.get("content") not in original]
        self.rejects(scenario, task, capture, files, raw)

    def test_same_pair_relocated_to_later_turn_is_rejected(self):
        scenario = EXTENSION["scenarios"][1]
        task, capture, files, raw = raw_fixture(scenario)
        pair = deepcopy(raw["before"]["state"]["messages"][5:7])
        for stage in ("after", "reopened", "final"):
            del raw[stage]["state"]["messages"][5:7]
            raw[stage]["state"]["messages"].extend(deepcopy(pair))
        self.rejects(scenario, task, capture, files, raw)


class ProtocolAndAuthority(unittest.TestCase):
    def test_arbitrary_json_cannot_become_a_driver_receipt(self):
        with self.assertRaisesRegex(Refused, "caller_json_is_not_a_driver_receipt"):
            require_authenticated({"passed": True, "source_commit": "0" * 40}, {})
        with self.assertRaisesRegex(Refused, "driver_authority_required"):
            AuthenticatedRun(Path("unused"), b"x" * 32, {}, -1, (0, 0), object())
        with self.assertRaisesRegex(Refused, "driver_authority_required"):
            seal_actual_driver({}, Path("unused"), {}, {}, [], [], [], Path("unused"), {})

    def test_original_anchor_does_not_follow_caller_digest(self):
        corpus = deepcopy(CORPUS)
        corpus["tasks"][0]["held_out"] = "weakened"
        extension = deepcopy(EXTENSION)
        changed = hashlib.sha256(json.dumps(corpus).encode()).hexdigest()
        extension["base_corpus_sha256"] = changed
        with self.assertRaisesRegex(Refused, "original_corpus_anchor_changed"):
            grader.validate_extension(extension, corpus, changed)
        self.assertEqual(EXTENSION["base_corpus_sha256"], ORIGINAL_CORPUS_SHA256)

    def test_unsigned_standalone_grading_refuses_before_inputs_or_docker(self):
        with tempfile.TemporaryDirectory(prefix="unsigned-grading-test-") as directory:
            root = Path(directory)
            with mock.patch.object(grader, "read_json", side_effect=AssertionError("No input may be read")):
                code = grader.main(["--corpus", str(root / "missing-corpus.json"), "--extension", str(root / "missing-extension.json"),
                    "--scenario", "compaction-v1", "--capture", str(root / "caller.json"), "--workspace", str(root),
                    "--image", "sha256:" + "0" * 64, "--output", str(root / "must-not-exist.json")])
            self.assertEqual(code, 2)
            self.assertFalse((root / "must-not-exist.json").exists())

    def test_status_parser_discards_terminal_text_and_handles_split_frames(self):
        facts = TerminalFacts()
        payload = b'private-prompt-and-key\x1b]7501;state=blocked:app=craftsman:kind=permission:msg=V2FpdGluZw==\x1b\\'
        for byte in payload:facts.feed(bytes([byte]))
        self.assertEqual(facts.state, "blocked")
        self.assertEqual(facts.kind, "permission")
        self.assertNotIn("private-prompt-and-key", json.dumps(facts.records))
        self.assertLessEqual(len(facts.pending), 1)

    def test_gateway_metadata_has_no_bodies_keys_or_endpoints_and_is_bounded(self):
        stream = io.StringIO()
        facts = GatewayObservations(stream, maximum=2)
        facts.record("attempted", "synthetic-private-run-label")
        facts.record("admitted", "synthetic-private-run-label", 0)
        self.assertNotIn("synthetic-private-run-label", stream.getvalue())
        self.assertEqual(len(stream.getvalue().splitlines()), 2)
        with self.assertRaises(ObservationFailure):facts.record("provider_eof", "run", 0, 200)
        self.assertTrue(facts.closed)

    def test_failed_observation_write_closes_future_records(self):
        class Broken:
            def write(self, value):raise OSError()
            def flush(self):pass
        facts = GatewayObservations(Broken())
        with self.assertRaises(ObservationFailure):facts.record("attempted", "run")
        self.assertTrue(facts.closed)
        with self.assertRaises(ObservationFailure):facts.record("attempted", "run")

    def test_data_seal_rejects_changed_observation_binding_and_workspace(self):
        if os.name != "posix":
            with self.assertRaisesRegex(Refused, "caller_json_is_not_a_driver_receipt"):
                require_authenticated({"passed": True}, {})
            return
        with tempfile.TemporaryDirectory(prefix="data-seal-test-") as directory:
            root = Path(directory)
            workspace = root / "workspace"
            workspace.mkdir()
            (workspace / "file.txt").write_bytes(b"original")
            files = {"file.txt": {"bytes": 8, "sha256": hashlib.sha256(b"original").hexdigest()}}
            captures = {"kind": "synthetic-data-only"}
            observations = {}
            for name, value in (("capture.json", captures), ("files.json", files)):
                data = canonical(value)
                (root / name).write_bytes(data)
                observations[name] = hashlib.sha256(data).hexdigest()
            key = b"synthetic-unit-key-not-runtime".ljust(32, b"x")
            bindings = {"source": "synthetic-unit-binding"}
            document = {"producer": "actual_owned_tui_synthetic_driver", "actual_agent_container_removed": True,
                        "paid_model_calls": 0, "bindings": bindings, "observations": observations}
            envelope = {"document": document, "hmac_sha256": hmac.new(key, canonical(document), hashlib.sha256).hexdigest()}
            (root / "receipt.json").write_bytes(canonical(envelope))
            fd = os.open(workspace, os.O_RDONLY | os.O_DIRECTORY)
            info = os.fstat(fd)
            authority = AuthenticatedRun(root, key, envelope, fd, (info.st_dev, info.st_ino), receipt_source._ISSUED)
            try:
                self.assertEqual(authority.verified_capture(bindings), captures)
                with self.assertRaises(Refused):authority.verified_capture({"source": "changed"})
                (workspace / "file.txt").write_bytes(b"changed")
                with self.assertRaisesRegex(Refused, "signed_task_snapshot_changed"):authority.verified_capture(bindings)
                (workspace / "file.txt").write_bytes(b"original")
                (root / "capture.json").write_bytes(b'{}')
                with self.assertRaisesRegex(Refused, "driver_observation_changed"):authority.verified_capture(bindings)
            finally:authority.close()

    def test_all_original_validation_lines_are_retained(self):
        root = Path(__file__).resolve().parents[2]
        for path in ("scripts/verify.sh", ".github/workflows/ci.yml"):
            actual = (root / path).read_bytes()
            original = wiring_baseline.original_validation_bytes(actual, path)
            preflight.validate_additive_wiring(original, actual, path)
            with self.assertRaises(Refused):preflight.validate_additive_wiring(original, actual.replace(b"--no-fail-fast", b"--fail-fast"), path) if path.endswith("ci.yml") else preflight.validate_additive_wiring(original, actual.replace(b"run_check core-tests", b"omit core-tests"), path)

    def test_validation_guard_needs_no_git_or_historical_object(self):
        missing = subprocess.CalledProcessError(128, ["git", "show", wiring_baseline.SOURCE_COMMIT])
        with mock.patch.object(subprocess, "check_output", side_effect=missing) as lookup, \
                mock.patch.object(subprocess, "run", side_effect=OSError("Git unavailable")) as command:
            self.test_all_original_validation_lines_are_retained()
        lookup.assert_not_called()
        command.assert_not_called()

    def test_frozen_validation_baseline_rejects_every_old_local_check(self):
        path = "scripts/verify.sh"
        actual = (Path(__file__).resolve().parents[2] / path).read_bytes()
        original = wiring_baseline.original_validation_bytes(actual, path)
        commands = [line for line in original.splitlines(keepends=True) if line.startswith(b"run_check ")]
        self.assertEqual(len(commands), 14)
        for line in commands:
            with self.subTest(command=line.split()[1].decode()):
                changed = actual.replace(line, line.replace(b"run_check", b"omit_check", 1), 1)
                with self.assertRaisesRegex(Refused, "original_validation_baseline_changed"):
                    wiring_baseline.original_validation_bytes(changed, path)

    def test_frozen_validation_baseline_rejects_ci_condition_and_command_changes(self):
        path = ".github/workflows/ci.yml"
        actual = (Path(__file__).resolve().parents[2] / path).read_bytes()
        mutations = (
            (b"runs-on: ubuntu-24.04", b"runs-on: ubuntu-22.04"),
            (b"runner: ubuntu-24.04", b"runner: ubuntu-22.04"),
            (b"runner: macos-15", b"runner: macos-14"),
            (b"runner: windows-2025", b"runner: windows-2022"),
            (b"rustup toolchain install --no-self-update", b"rustup show"),
            (b"sudo apt-get update --error-on=any", b"sudo apt-get update"),
            (b"GGML_NATIVE=OFF cargo check --locked --workspace", b"cargo check --workspace"),
            (b"cargo test --locked --no-default-features --workspace --no-fail-fast", b"cargo test --workspace"),
            (b"cargo test --locked --no-default-features --features craftsman-rs/gadgets --workspace", b"cargo test --workspace"),
            (b"if: ${{ !cancelled() }}", b"if: false"),
            (b"run: bash scripts/check-lineage.sh", b"run: true"),
            (b"run: git diff --exit-code -- crates/craftsman-rs/tests/golden", b"run: true"),
        )
        for before, after in mutations:
            with self.subTest(condition=before.decode()):
                self.assertIn(before, actual)
                with self.assertRaisesRegex(Refused, "original_validation_baseline_changed"):
                    wiring_baseline.original_validation_bytes(actual.replace(before, after, 1), path)

    def test_frozen_validation_baseline_rejects_same_length_old_edits(self):
        root = Path(__file__).resolve().parents[2]
        for path, before, after in (
                ("scripts/verify.sh", b"run_check core-tests", b"run_check xore-tests"),
                (".github/workflows/ci.yml", b"fail-fast: false", b"fail-fast: False")):
            with self.subTest(path=path):
                actual = (root / path).read_bytes()
                self.assertIn(before, actual)
                changed = actual.replace(before, after, 1)
                self.assertEqual(len(actual), len(changed))
                with self.assertRaisesRegex(Refused, "original_validation_baseline_changed"):
                    wiring_baseline.original_validation_bytes(changed, path)

    def test_frozen_validation_baseline_requires_all_exact_addition_counts(self):
        root = Path(__file__).resolve().parents[2]
        for path, commands in wiring_baseline.ADDITIONS.items():
            actual = (root / path).read_bytes()
            original = wiring_baseline.original_validation_bytes(actual, path)
            with self.subTest(path=path, mutation="all additions absent"), \
                    self.assertRaisesRegex(Refused, "incomplete_additive_validation_wiring"):
                wiring_baseline.original_validation_bytes(original, path)
            for command in commands:
                for mutation, changed in (
                        ("removed", actual.replace(command, b"", 1)),
                        ("duplicated", actual.replace(command, command + command, 1)),
                        ("modified", actual.replace(command, command.replace(b"-I -B", b"-I   -B"), 1))):
                    with self.subTest(path=path, command=command.decode(), mutation=mutation), \
                            self.assertRaisesRegex(Refused, "incomplete_additive_validation_wiring"):
                        wiring_baseline.original_validation_bytes(changed, path)


class NativeChildWitness(unittest.TestCase):
    def prepare(self, root):
        scenario = EXTENSION["scenarios"][2]
        for name, value in scenario["fixture_files"].items():(root / name).write_bytes(value.encode())
        wrapper = Path(__file__).resolve().parent / "native_probe_wrapper.py"
        (root / "native_probe_wrapper.py").write_bytes(wrapper.read_bytes())
        return root / "native_probe_wrapper.py", scenario

    def test_absent_unknown_duplicate_markers_have_no_child_or_witness(self):
        with tempfile.TemporaryDirectory(prefix="probe-marker-test-") as directory:
            root = Path(directory)
            script, _ = self.prepare(root)
            for args in ([], ["unknown"], ["--owned-native-probe-v2", "--owned-native-probe-v2"]):
                result = subprocess.run([sys.executable, "-I", "-B", str(script), *args], cwd=root, capture_output=True, timeout=20)
                self.assertEqual(result.returncode, 0)
                self.assertEqual(result.stdout + result.stderr, b"")
                self.assertFalse((root / "probe_calls.txt").exists())
                self.assertFalse((root / "native_probe_witness.json").exists())

    def test_one_actual_child_exit_and_streams_then_no_replay(self):
        with tempfile.TemporaryDirectory(prefix="native-child-test-") as directory:
            root = Path(directory)
            script, scenario = self.prepare(root)
            argv = [sys.executable, "-I", "-B", str(script), "--owned-native-probe-v2"]
            result = subprocess.run(argv, cwd=root, capture_output=True, timeout=30)
            if os.name != "posix":
                self.assertEqual(result.returncode, 2)
                self.assertFalse((root / "native_probe_witness.json").exists())
                return
            self.assertEqual(result.returncode, 7)
            self.assertEqual(result.stdout, scenario["expected_answer"]["stdout"].encode())
            self.assertEqual(result.stderr, scenario["expected_answer"]["stderr"].encode())
            witness = (root / "native_probe_witness.json").read_bytes()
            value = json.loads(witness)
            self.assertEqual(value["actual_child_exit"], result.returncode)
            self.assertTrue(value["actual_wait_returned"])
            self.assertFalse(value["outer_toolmeta_observed"])
            self.assertEqual(value["stdout_sha256"], hashlib.sha256(result.stdout).hexdigest())
            repeated = subprocess.run(argv, cwd=root, capture_output=True, timeout=30)
            self.assertEqual(repeated.returncode, 2)
            self.assertEqual((root / "probe_calls.txt").read_bytes(), b"invoked\n")
            self.assertEqual((root / "native_probe_witness.json").read_bytes(), witness)

    def test_changed_immutable_child_source_is_refused_before_effects(self):
        with tempfile.TemporaryDirectory(prefix="changed-child-test-") as directory:
            root = Path(directory)
            script, _ = self.prepare(root)
            (root / "probe_once.py").write_bytes(b"raise SystemExit(0)\n")
            result = subprocess.run([sys.executable, "-I", "-B", str(script), "--owned-native-probe-v2"], cwd=root, capture_output=True, timeout=20)
            self.assertEqual(result.returncode, 2)
            self.assertFalse((root / "probe_calls.txt").exists())
            self.assertFalse((root / "native_probe_witness.json").exists())


class ActualGatewayBoundary(unittest.TestCase):
    def test_real_twentieth_admission_twenty_first_refusal_and_provider_eof(self):
        received = []
        mode = ["completed"]
        class Provider(BaseHTTPRequestHandler):
            def log_message(self, *_):pass
            def do_POST(self):
                self.rfile.read(int(self.headers["Content-Length"]))
                received.append(1)
                event = {"type": "response.created", "response": {"id": "fixture", "status": "in_progress"}} if mode[0] == "eof" else {
                    "type": "response.completed", "response": {"usage": {"input_tokens": 10, "output_tokens": 2}, "private_body": "synthetic-provider-body-marker"}}
                data = b"data: " + json.dumps(event).encode() + b"\n\n"
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream")
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)
        server = ThreadingHTTPServer(("127.0.0.1", 0), Provider)
        server.daemon_threads = True
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        process = None
        with tempfile.TemporaryDirectory(prefix="actual-gateway-observations-") as directory:
            root = Path(directory)
            bootstrap = {"endpoint": "http://127.0.0.1:" + str(server.server_port) + "/v1", "api_key": "synthetic-provider-key", "model": "gpt-6.1-sol",
                "tokens": {"synthetic-agent-key": "at-twenty", "synthetic-eof-key": "new-eof"},
                "prices": {"input_per_million": "0", "output_per_million": "0", "request_fee": "0"}, "prices_verified": True,
                "cap_usd": "25.00", "ledger": str(root / "ledger.json"), "synthetic": True, "private_observations": True}
            observations = []
            ready = queue.Queue()
            try:
                process = subprocess.Popen([sys.executable, "-I", "-B", str(Path(__file__)), "--synthetic-gateway-fixture-v2"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
                def reader():
                    for line in process.stdout:
                        value = json.loads(line)
                        observations.append(value)
                        if value.get("ready"):ready.put(value)
                reader_thread = threading.Thread(target=reader, daemon=True)
                reader_thread.start()
                process.stdin.write(json.dumps(bootstrap).encode())
                process.stdin.close()
                bound = ready.get(timeout=10)["synthetic_bound_port"]
                import urllib.request
                import urllib.error
                opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
                def request(key):
                    data = json.dumps({"model": "gpt-6.1-sol", "input": "synthetic-input-body-marker", "max_output_tokens": 8192}).encode()
                    req = urllib.request.Request("http://127.0.0.1:" + str(bound) + "/v1/responses", data=data, headers={"Authorization": "Bearer " + key, "Content-Type": "application/json"})
                    with opener.open(req, timeout=10) as response:return response.status, response.read()
                for _ in range(20):self.assertEqual(request("synthetic-agent-key")[0], 200)
                with self.assertRaises(urllib.error.HTTPError) as refused:request("synthetic-agent-key")
                self.assertEqual(refused.exception.code, 402)
                mode[0] = "eof"
                self.assertEqual(request("synthetic-eof-key")[0], 200)
                end = time.monotonic() + 5
                while not any(value.get("phase") == "unknown_usage" for value in observations) and time.monotonic() < end:time.sleep(0.01)
                events = [value for value in observations if value.get("kind") == "private_gateway_observation"]
                run = hashlib.sha256(b"at-twenty").hexdigest()
                selected = [value for value in events if value["run_sha256"] == run]
                self.assertEqual(sum(value["phase"] == "attempted" for value in selected), 21)
                self.assertEqual(sum(value["phase"] == "admitted" for value in selected), 20)
                self.assertEqual([value["reason"] for value in selected if value["phase"] == "refused"], ["request_limit"])
                self.assertEqual(len(received), 21)
                self.assertTrue(any(value["phase"] == "provider_eof" and value["terminal_event"] is None for value in events))
                self.assertTrue(any(value["phase"] == "unknown_usage" for value in events))
                ledger = json.loads((root / "ledger.json").read_text())
                self.assertEqual(len(ledger["requests"]), 21)
                self.assertEqual(sum(value["state"] == "unsettled" for value in ledger["requests"]), 1)
                output = json.dumps(observations)
                for marker in ("synthetic-input-body-marker", "synthetic-provider-body-marker", "synthetic-provider-key", "synthetic-agent-key", bootstrap["endpoint"]):self.assertNotIn(marker, output)
            finally:
                if process is not None:
                    process.terminate()
                    process.wait(timeout=5)
                    reader_thread.join(timeout=2)
                    process.stdout.close()
                    process.stderr.close()
        server.shutdown()
        server.server_close()
        thread.join(timeout=2)


if __name__ == "__main__":
    # Keep the 17 discoverable entrypoints; the owned-TUI entrypoint also runs
    # the independently reviewed retained-SQLite/collector rejection cases.
    from test_owned_tui_boundaries import (NegativeIssuanceBoundaries, RetainedWriteBoundaries,
        SqliteSnapshotBoundaries, NativeCollectorBoundaries, CleanupAndMarkerBoundaries, SnapshotChannelBoundaries)
    unittest.main()
