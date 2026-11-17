"""Run prepared scenarios through an actual owned TUI in isolated Docker.

Only synthetic providers and a private zero-price fixture ledger are accepted.
The approved real ledger is read before/after and never mounted or written.
"""
import argparse
import hashlib
import inspect
import json
import os
from pathlib import Path
import secrets
import selectors
import shlex
import subprocess
import sys
import tempfile
import time

sys.path.insert(0, str(Path(__file__).resolve().parent))
from preflight_continuation import canonical, read_json, require, Refused, validate_build
from grade_long_session import validate_extension, updates, grade_capture
import grade_long_session as observation_source
import preflight_continuation as primitives
from coding_eval_anchors import ORIGINAL_CORPUS_COMMIT, ORIGINAL_CORPUS_SHA256, SYNTHETIC_IMAGE_ID, SYNTHETIC_BINARY_SHA256, BASE_SCENARIO_SHA256, V2_SCENARIO_SHA256
from owned_tui_files import read_leaf, snapshot_retained
from run_suite import agent_configuration, snapshot_files
from check_corpus import grade
from owned_tui_receipt import DockerRunIssuer, seal_actual_driver, require_authenticated


class JsonProcess:
    def __init__(self, argv):
        self.process = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        self.selector = selectors.DefaultSelector()
        self.selector.register(self.process.stdout, selectors.EVENT_READ, "stdout")
        self.selector.register(self.process.stderr, selectors.EVENT_READ, "stderr")
        self.pending = bytearray()
        self.values = []
        self.history = []
        self.total = 0
        self.stderr_bytes = 0
        self.stderr_hash = hashlib.sha256()

    def send(self, value):
        data = canonical(value) + b"\n"
        require(len(data) <= 1024 * 1024, "driver_control_bound")
        self.process.stdin.write(data)
        self.process.stdin.flush()

    def pump(self, timeout=0):
        for key, _ in self.selector.select(timeout):
            data = os.read(key.fd, 65536)
            if not data:
                self.selector.unregister(key.fileobj)
                continue
            self.total += len(data)
            require(self.total <= 8 * 1024 * 1024, "private_observation_stream_bound")
            if key.data == "stderr":
                self.stderr_bytes += len(data)
                self.stderr_hash.update(data)
                continue
            self.pending.extend(data)
            require(len(self.pending) <= 1024 * 1024, "private_observation_frame_bound")
            while b"\n" in self.pending:
                line, _, rest = self.pending.partition(b"\n")
                self.pending = bytearray(rest)
                value = json.loads(line)
                self.values.append(value)
                if value.get("kind") != "poll":
                    self.history.append(value)
                require(len(self.history) <= 4096, "private_observation_history_bound")
                require(len(self.values) <= 4096, "private_observation_event_bound")

    def wait_kind(self, kind, timeout=15):
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            self.pump(0.02)
            for index, value in enumerate(self.values):
                if value.get("kind") == kind:
                    return self.values.pop(index)
                require(value.get("kind") not in {"client_failed", "fixture_request_failed"}, "actual_child_reported_failure")
            if self.process.poll() is not None:
                self.pump(0.02)
                for index, value in enumerate(self.values):
                    if value.get("kind") == kind:
                        return self.values.pop(index)
                raise Refused("private_process_returned_early_" + kind)
        raise Refused("private_process_observation_timeout")

    def close(self):
        if self.process.stdin and not self.process.stdin.closed:
            self.process.stdin.close()
        if self.process.poll() is None:
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=5)
        self.selector.close()
        for stream in (self.process.stdout, self.process.stderr):
            if stream is not None:stream.close()


def docker(*args, timeout=60):
    result = subprocess.run(["docker", *args], stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                            stderr=subprocess.PIPE, timeout=timeout)
    require(result.returncode == 0, "docker_operation_failed")
    require(len(result.stdout) <= 8 * 1024 * 1024, "docker_output_bound")
    return result.stdout.decode().strip()


def scenario_actions(task, scenario):
    original = [{"id": "original-" + str(index), "name": "edit_file", "args": {"file_path": path, "old_string": task["files"][path], "new_string": text}}
                for index, (path, text) in enumerate(task["reference"].items())]
    original.append({"id": "original-check", "name": "execute", "args": {"command": shlex.join(task["public_command"])}})
    setup = [{"id": "scenario-effect", "name": "execute", "args": {"command": scenario["probe_command"]}}] if scenario["probe_command"] else [
        {"id": "scenario-effect", "name": "write_file", "args": {"file_path": scenario["effect_file"], "content": scenario["expected_effect"]}}]
    final = [{"id": "final-answer", "name": "write_file", "args": {"file_path": "session_result.json", "content": json.dumps(scenario["expected_answer"], ensure_ascii=False) + "\n"}},
             {"id": "final-check", "name": "execute", "args": {"command": shlex.join(task["public_command"])}}]
    duplicate = [{"id": "explicit-duplicate", "name": "execute", "args": {"command": scenario["probe_command"] or "true"}}]
    reread = [{"id": "explicit-current-read", "name": "read_file", "args": {"file_path": "probe_input.txt"}}]
    return {"original": original, "setup": setup, "final": final, "duplicate": duplicate, "reread": reread}


def client_bundle(path, source):
    prefix = 'import sys\nif __name__ == "__main__" and (not sys.argv[1:] or sys.argv[1] not in {"--owned-tui-client-v1", "--owned-tui-keeper-v1"}): raise SystemExit(0)\n'
    prefix += 'import json, hashlib, re\nfrom collections import Counter\nclass Refused(ValueError): pass\nLONG_LIMITS = {"checkpoint_capture_bytes": 8388608}\n'
    for name in ("require", "keys", "integer", "digest", "text_size", "canonical", "canonical_sha256"):
        prefix += inspect.getsource(getattr(primitives, name)) + '\n'
    for name in ("checkpoint", "completed_groups", "command_count", "results_for_command", "owned_input_observation", "durable_observation", "group_positions", "safe_checkpoint_observation"):
        prefix += inspect.getsource(getattr(observation_source, name)) + '\n'
    prefix += '_capture_checkpoint = safe_checkpoint_observation\n'
    with path.open("x") as output:
        output.write(prefix + source.read_text())
    return path


def run_case(root, case, task, scenario, extension, image, build, contract, variant="normal"):
    workspace = case / "workspace"
    workspace.mkdir()
    for relative, text in {**task["files"], **scenario["fixture_files"]}.items():
        path = workspace / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(text.encode())
    workspace_fd = os.open(workspace, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    git = ["git", "-c", "core.hooksPath=/dev/null", "-c", "commit.gpgsign=false", "-c", "user.name=Evaluation fixture", "-c", "user.email=fixture@localhost"]
    subprocess.run([*git, "init", "--quiet", str(workspace)], check=True)
    subprocess.run([*git, "-C", str(workspace), "add", "--all"], check=True)
    subprocess.run([*git, "-C", str(workspace), "commit", "--quiet", "--no-gpg-sign", "-m", "Original task"], check=True)
    nonce = secrets.token_hex(6)
    network = "owned-eval-" + nonce
    agent_name, gateway_name, fixture_name = ("owned-eval-" + name + "-" + nonce for name in ("agent", "gateway", "fixture"))
    paths = root / "scripts/coding-eval"
    client_path = client_bundle(case / "trusted-client.py", paths / "owned_tui_client.py")
    budget = case / "synthetic-budget"
    budget.mkdir()
    run_name = "owned-" + scenario["id"]
    actions = scenario_actions(task, scenario)
    client = gateway = fixture = None
    started = time.monotonic()
    returns, captures, effects = [], {}, {}
    last_poll = None
    removed = False
    native_child = None
    probe_input_observations = []
    try:
        docker("network", "create", "--internal", network)
        common = ["--read-only", "--cap-drop", "ALL", "--security-opt", "no-new-privileges", "--pids-limit", "128", "--memory", "768m", "--cpus", "2", "--user", str(os.getuid()) + ":" + str(os.getgid())]
        fixture = JsonProcess(["docker", "run", "--rm", "--interactive", "--name", fixture_name, "--network", network, "--network-alias", "fixture", *common,
            "--mount", "type=bind,src=" + str(paths / "owned_tui_provider.py") + ",dst=/driver/provider.py,readonly",
            "--entrypoint", "python3", image, "-I", "-B", "/driver/provider.py", "--owned-tui-provider-v1"])
        fixture.send({"scenario": scenario["id"], "actions": actions})
        fixture.wait_kind("fixture_ready")
        gateway = JsonProcess(["docker", "run", "--rm", "--interactive", "--name", gateway_name, "--network", network, "--network-alias", "gateway", *common,
            "--mount", "type=bind,src=" + str(paths) + ",dst=/runner,readonly",
            "--mount", "type=bind,src=" + str(budget) + ",dst=/ledger",
            "--entrypoint", "python3", image, "-I", "-B", "/runner/gateway.py"])
        gateway.send({"endpoint": "http://fixture:9876/v1", "api_key": "synthetic-provider-key", "model": "gpt-6.1-sol",
            "tokens": {"synthetic-agent-key": run_name}, "prices": {"input_per_million": "0", "output_per_million": "0", "request_fee": "0"},
            "prices_verified": True, "cap_usd": "25.00", "ledger": "/ledger/ledger.json", "synthetic": True, "private_observations": True})
        gateway.process.stdin.close()
        # Gateway readiness retains its original public/private bootstrap shape.
        end = time.monotonic() + 15
        while not any(value.get("ready") is True for value in gateway.values):
            gateway.pump(0.02)
            require(time.monotonic() < end and gateway.process.poll() is None, "gateway_readiness_failed")
        docker("run", "--detach", "--name", agent_name, "--network", network, *common,
            "--tmpfs", "/home/eval:rw,nosuid,nodev,uid=" + str(os.getuid()) + ",gid=" + str(os.getgid()) + ",size=128m",
            "--tmpfs", "/tmp:rw,nosuid,nodev,size=128m", "--env", "HOME=/home/eval", "--env", "CRAFTSMAN_CONFIG_DIR=/home/eval/.craftsman",
            "--mount", "type=bind,src=" + str(workspace) + ",dst=/workspace",
            "--mount", "type=bind,src=" + str(client_path) + ",dst=/driver/client.py,readonly",
            "--workdir", "/workspace", "--entrypoint", "sleep", image, "86400")
        client = JsonProcess(["docker", "exec", "--interactive", agent_name, "python3", "-I", "-B", "/driver/client.py", "--owned-tui-client-v1"])
        config = agent_configuration("openai/gpt-6.1-sol", "gateway", "synthetic-agent-key", {},
            {"context": {"enabled": True, "max_bytes": 12288}, "failure_control": {"repeat_limit": 5, "test_review_signals": False}}, "xhigh")
        client.send({"config": config, "once_command": scenario["probe_command"],
                     "allowed_calls": [hashlib.sha256(canonical(call)).hexdigest() for calls in actions.values() for call in calls]})
        ready = client.wait_kind("client_ready")
        require(ready["binary_sha256"] == build["binary_sha256"], "executed_binary_binding_changed")
        require(ready["actual_config_model_settings_sha256"] == extension["effective_model_settings_sha256"], "actual_model_settings_changed")
        bindings = {"source_commit": build["source_commit"], "source_tree": build["source_tree"], "image_id": image,
            "binary_sha256": build["binary_sha256"], "scenario_sha256": hashlib.sha256(canonical(scenario)).hexdigest(),
            "extension_sha256": hashlib.sha256((root / "evals/coding/latest-runtime-v1/long-session.json").read_bytes()).hexdigest(),
            "v2_contract_sha256": V2_SCENARIO_SHA256, "native_wrapper_sha256": contract["native_wrapper_sha256"],
            "actual_config_model_settings_sha256": ready["actual_config_model_settings_sha256"],
            "mounted_client_sha256": hashlib.sha256(client_path.read_bytes()).hexdigest(),
            "driver_files": {name: hashlib.sha256((paths / name).read_bytes()).hexdigest() for name in ["run_owned_tui.py", "owned_tui_client.py", "owned_tui_provider.py", "owned_tui_receipt.py", "owned_tui_files.py", "native_probe_wrapper.py", "gateway.py", "gateway_observations.py", "grade_long_session.py"]}}
        issuer = DockerRunIssuer(client, gateway, fixture, agent_name, bindings, workspace, workspace_fd)
        client.send({"op": "start"})
        client.wait_kind("owner_started")

        def poll():
            nonlocal last_poll
            require(time.monotonic() - started <= 900, "scenario_wall_limit")
            gateway.pump()
            fixture.pump()
            require(not any(value.get("kind") == "fixture_request_failed" for value in fixture.values), "actual_fixture_failed")
            client.send({"op": "poll"})
            last_poll = client.wait_kind("poll")
            return last_poll

        def until(predicate, timeout=30):
            end = time.monotonic() + timeout
            while time.monotonic() < end:
                value = poll()
                if predicate(value):
                    return value
                require(value.get("child_exit") is None, "owned_tui_returned_early")
            raise Refused("owned_tui_state_timeout")

        def phase(name, prompt=None):
            fixture.send({"op": "phase", "phase": name})
            fixture.wait_kind("fixture_phase_selected")
            if prompt is not None:
                client.send({"op": "input", "text": prompt})
                client.wait_kind("input_sent")

        def turn(name, prompt):
            prior = last_poll.get("checkpoint", {}).get("seq", -1) if last_poll else -1
            phase(name, prompt)
            return until(lambda value: value.get("root_state") == "done"
                and value.get("durable_in_progress") is False and value.get("checkpoint", {}).get("seq", -1) > prior
                and value.get("checkpoint", {}).get("owned_input", {}).get("last_sha256") == hashlib.sha256(prompt.encode()).hexdigest())

        def request_count():
            gateway.pump()
            return sum(value.get("phase") == "admitted" for value in gateway.values)

        def attempt_count():
            gateway.pump()
            return sum(value.get("phase") == "attempted" for value in gateway.values)

        def snapshot(stage):
            client.send({"op": "capture", "stage": stage, "allow_error_observation": variant != "normal"})
            value = client.wait_kind("checkpoint_captured")
            require((value.get("durable_in_progress") is False or variant != "normal" and value.get("root_state") == "error")
                    and not value.get("suspension_present"), "checkpoint_not_finished")
            captures[stage] = value["checkpoint"]
            effect = snapshot_retained(workspace_fd)
            require(scenario["effect_file"] in effect, "actual_effect_missing")
            effects[stage] = hashlib.sha256(effect[scenario["effect_file"]]).hexdigest()

        until(lambda value: value.get("root_state") == "idle")
        turn("original", task["prompt"])
        turn("setup", scenario["setup_prompt"])
        if scenario["id"] == "retained-results-v1":
            native_child = json.loads(read_leaf(workspace_fd, "native_probe_witness.json", 2048))
            require(native_child["kind"] == "observed_native_probe_child" and native_child["actual_wait_returned"] is True
                    and native_child["actual_child_exit"] == 7 and native_child["outer_toolmeta_observed"] is False,
                    "native_child_witness_incomplete")
            answer = scenario["expected_answer"]
            require(native_child["stdout_sha256"] == hashlib.sha256(answer["stdout"].encode()).hexdigest()
                    and native_child["stderr_sha256"] == hashlib.sha256(answer["stderr"].encode()).hexdigest()
                    and native_child["stdout_bytes"] == len(answer["stdout"].encode())
                    and native_child["stderr_bytes"] == len(answer["stderr"].encode()), "native_child_stream_observation_mismatch")
            probe_input_observations.append({"stage": "original_native_child_completed", "sha256": hashlib.sha256(read_leaf(workspace_fd, "probe_input.txt")).hexdigest()})
            (workspace / "probe_input.txt").write_bytes(b"changed after the original observation\n")
            probe_input_observations.append({"stage": "operator_changed_input", "sha256": hashlib.sha256(read_leaf(workspace_fd, "probe_input.txt")).hexdigest()})
        for value in updates(scenario):
            turn("update", value)
        snapshot("before")
        if variant != "normal":
            before_requests = request_count()
            before_seq = captures["before"]["seq"]
            if variant == "save-refusal":
                client.send({"op": "hold_store_writer"})
                client.wait_kind("owned_store_writer_hold_started")
                phase("pending", "Observe the isolated checkpoint write refusal; do not perform new work.")
                failed = until(lambda value: value.get("root_state") == "error", timeout=15)
                require(request_count() == before_requests, "save_refusal_sent_model_request")
                require(failed.get("checkpoint", {}).get("seq") == before_seq, "failed_save_changed_stored_row")
                client.send({"op": "release_store_writer"})
                client.wait_kind("owned_store_writer_hold_released")
                expected_failure = "root_error_with_held_writer_and_unchanged_row"
            elif variant == "provider-eof":
                phase("eof", "Observe genuine synthetic provider EOF; do not repeat completed effects.")
                until(lambda value: value.get("root_state") == "error", timeout=15)
                end = time.monotonic() + 15
                while not any(value.get("phase") == "unknown_usage" for value in gateway.history):
                    poll()
                    require(time.monotonic() < end, "eof_unknown_usage_not_observed")
                require(any(value.get("phase") == "provider_eof" and value.get("terminal_event") is None for value in gateway.history),
                        "genuine_provider_eof_not_observed")
                expected_failure = "root_error_after_actual_provider_eof"
            elif variant == "duplicate-create":
                require(scenario["id"] == "interruption-reopen-v1", "duplicate_case_requires_create_only_fixture")
                turn("duplicate", "Explicit negative control: request a new duplicate create-only call and retain its refusal without a repair retry.")
                value = poll()
                require(value["checkpoint"]["seq"] > before_seq, "duplicate_result_not_saved")
                expected_failure = "explicit_duplicate_result_recorded_original_effect_unchanged"
            elif variant == "forbidden-reread":
                require(scenario["id"] == "retained-results-v1", "reread_case_requires_probe_fixture")
                turn("reread", "Explicit negative control: read the changed probe input once and retain the actual read result.")
                expected_failure = "observed_direct_probe_read_rejects_positive_criterion"
            else:
                raise Refused("unknown_negative_case")
            snapshot("after")
            require(effects["after"] == effects["before"], "negative_control_changed_original_effect")
            client.send({"op": "quit"})
            returns.append(client.wait_kind("owner_returned", 35))
            client.send({"op": "finish"})
            finished = client.wait_kind("client_finished")
            docker("rm", "--force", agent_name)
            removed = True
            gateway.pump(0.05)
            fixture.pump(0.05)
            files = snapshot_retained(workspace_fd)
            capture = {"version": 2, "kind": "negative_external_observation_capture", "variant": variant,
                       "checkpoints": captures, "owned_input_receipts": finished["owned_input_receipts"],
                       "expected_failure": expected_failure, "effect_sha256_by_stage": effects}
            manifest = {name: {"bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()} for name, data in files.items()}
            authority = seal_actual_driver(issuer, case, bindings, capture,
                [value for value in gateway.history if value.get("kind") == "private_gateway_observation"], fixture.history,
                returns, workspace, manifest)
            try:
                require_authenticated(authority, bindings)
                result = {"kind": "scoped_owned_tui_negative_result_v2", "variant": variant,
                          "expected_negative_observations": "observed", "positive_saved_success": False,
                          "observed_failure": expected_failure, "receipt_authenticated": True, "paid_model_calls": 0,
                          "source_commit": build["source_commit"], "image_id": image,
                          "stronger_private_criteria": {name: {"status": "unverified"} for name in contract["criteria_unverified"]}}
                with (case / "result.json").open("x") as output:json.dump(result, output, indent=2)
                return result
            finally:authority.close()
        action = "probe_result_saved_then_reopen"
        if scenario["id"] == "compaction-v1":
            before = captures["before"]["seq"]
            phase("compact", "/compact")
            until(lambda value: value.get("root_state") == "idle" and value.get("durable_in_progress") is False
                  and value.get("checkpoint", {}).get("seq", -1) > before)
            action = "manual_compaction_saved"
        elif scenario["id"] == "interruption-reopen-v1":
            phase("pending", "Wait for the explicitly interrupted synthetic model call; do not repeat the completed effect.")
            end = time.monotonic() + 15
            while not any(value.get("kind") == "fixture_pending_observed" for value in fixture.values):
                poll()
                require(time.monotonic() < end, "pending_model_not_observed")
            client.send({"op": "cancel"})
            client.wait_kind("cancel_key_sent")
            until(lambda value: value.get("root_state") == "idle" and value.get("stopping_observed")
                  and value.get("durable_in_progress") is False)
            fixture.send({"op": "release"})
            fixture.wait_kind("fixture_pending_release")
            end = time.monotonic() + 15
            while not any(value.get("phase") == "unknown_usage" for value in gateway.values):
                poll()
                require(time.monotonic() < end, "actual_unknown_settlement_not_observed")
            action = "cancelled_return_then_reopen"
        snapshot("after")
        client.send({"op": "quit"})
        returns.append(client.wait_kind("owner_returned", 35))
        before_reopen = request_count()
        attempts_before_reopen = attempt_count()
        client.send({"op": "start"})
        client.wait_kind("owner_started")
        until(lambda value: value.get("root_state") == "idle" and value.get("durable_in_progress") is False)
        idle_start = time.monotonic()
        while time.monotonic() - idle_start < 5:
            poll()
            require(request_count() == before_reopen and attempt_count() == attempts_before_reopen, "idle_reopen_sampled")
        elapsed = int((time.monotonic() - idle_start) * 1000)
        snapshot("reopened")
        turn("final", scenario["final_prompt"])
        snapshot("final")
        final_requests = request_count()
        client.send({"op": "quit"})
        returns.append(client.wait_kind("owner_returned", 35))
        client.send({"op": "finish"})
        client.wait_kind("client_finished")
        docker("rm", "--force", agent_name)
        removed = True
        gateway.pump(0.05)
        fixture.pump(0.05)
        files = snapshot_retained(workspace_fd)
        capture = {"version": 1, "kind": "operator_long_session_capture", "scenario_id": scenario["id"],
            "source_commit": extension["source_commit"], "source_tree": extension["source_tree"],
            "feature_options_sha256": extension["feature_options_sha256"], "effective_model_settings_sha256": extension["effective_model_settings_sha256"],
            "checkpoints": captures, "gateway_requests": {"before_reopen": before_reopen, "after_idle_reopen": before_reopen, "final": final_requests},
            "completed_update_turns": 10, "observed_action": action, "effect_sha256_by_stage": effects,
            "idle_reopen_elapsed_millis": elapsed, "owned_input_receipts": last_poll["owned_input_receipts"]}
        manifest = {name: {"bytes": len(value), "sha256": hashlib.sha256(value).hexdigest()} for name, value in files.items()}
        authority = seal_actual_driver(issuer, case, bindings, capture,
            [value for value in gateway.history if value.get("kind") == "private_gateway_observation"], fixture.history,
            returns, workspace, manifest)
        try:
            validated = require_authenticated(authority, bindings)
            scored = grade_capture(scenario, task, validated, files, extension)
            with tempfile.TemporaryDirectory(prefix="owned-tui-independent-grade-") as temporary:
                tmp = Path(temporary)
                tmp.chmod(0o755)
                candidate, grader = tmp / "candidate", tmp / "grader"
                candidate.mkdir()
                grader.mkdir()
                for name, value in files.items():
                    destination = candidate / name
                    destination.parent.mkdir(parents=True, exist_ok=True)
                    destination.write_bytes(value)
                behavior = grade(task, candidate, grader, image)
            require(behavior["passed"], "actual_independent_grader_failed")
            result = {"kind": "scoped_owned_tui_external_result_v2", "scenario": scenario["id"],
                      "external_observation_criteria": "completed", "original_task_behavior": "passed",
                      "receipt_authenticated_before_grading": True, "paid_model_calls": 0, "synthetic_provider_requests": final_requests,
                      "is_live_quality_score": False, "source_commit": build["source_commit"], "image_id": image,
                      "original_held_out_passed": True, "wall_seconds": round(time.monotonic() - started, 3),
                      "no_new_gateway_attempts_or_admissions_in_idle_window": True,
                      "stored_group_witnesses": next(value["stored_group_witnesses"] for value in issuer.client.history if value.get("kind") == "client_finished"),
                      "native_child_observation": native_child, "physical_probe_input_observations": probe_input_observations,
                      "stronger_private_criteria": {name: {"status": "unverified"} for name in contract["criteria_unverified"]},
                      "all_stronger_private_criteria_verified": False}
            with (case / "result.json").open("x") as output:
                json.dump(result, output, indent=2)
                output.write("\n")
            return result
        finally:
            authority.close()
    except Exception:
        failure = {"kind": "actual_driver_failure", "scenario": scenario["id"], "passed": False,
                   "agent_removed_before_grading": removed, "last_safe_poll": last_poll,
                   "gateway_observations": [] if gateway is None else [value for value in gateway.values if value.get("kind") == "private_gateway_observation"],
                   "fixture_observations": [] if fixture is None else fixture.values, "paid_model_calls": 0}
        failure["client_observations"] = [] if client is None else client.values
        failure["private_stderr_observations"] = {name: {"bytes": process.stderr_bytes, "sha256": process.stderr_hash.hexdigest()}
            for name, process in (("client", client), ("gateway", gateway), ("fixture", fixture)) if process is not None}
        failure["private_process_returns"] = {name: process.process.poll() for name, process in (("client", client), ("gateway", gateway), ("fixture", fixture)) if process is not None}
        with (case / "failure.json").open("x") as output:
            json.dump(failure, output, indent=2)
        raise
    finally:
        for name in (agent_name, gateway_name, fixture_name):
            subprocess.run(["docker", "rm", "--force", name], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=15)
        for process in (client, gateway, fixture):
            if process is not None:
                process.close()
        subprocess.run(["docker", "network", "rm", network], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=15)
        os.close(workspace_fd)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--image-build-record", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--scenario", choices=["all", "compaction-v1", "interruption-reopen-v1", "retained-results-v1"], default="all")
    parser.add_argument("--negative-case", choices=["save-refusal", "provider-eof", "duplicate-create", "forbidden-reread"])
    args = parser.parse_args(argv)
    try:
        require(os.name == "posix", "owned_pty_driver_requires_unix_docker_host")
        args.output = args.output.resolve()
        root = Path(__file__).resolve().parents[2]
        build, _ = read_json(args.image_build_record, 65536)
        validate_build({key: build[key] for key in ("source_commit", "source_tree", "image_id")})
        require(build["source_commit"] == ORIGINAL_CORPUS_COMMIT and build["build_exit"] == 0
                and build["image_id"] == SYNTHETIC_IMAGE_ID and build["binary_sha256"] == SYNTHETIC_BINARY_SHA256,
                "actual_frozen_source_build_required")
        archive = subprocess.check_output(["git", "-C", str(root), "archive", ORIGINAL_CORPUS_COMMIT])
        require(hashlib.sha256(archive).hexdigest() == build["source_archive_sha256"], "tracked_archive_authority_changed")
        image = docker("image", "inspect", build["image_id"], "--format", "{{.Id}}")
        require(image == build["image_id"], "immutable_image_binding_changed")
        corpus, corpus_hash = read_json(root / "evals/coding/tasks.json", 8 * 1024 * 1024)
        require(corpus_hash == ORIGINAL_CORPUS_SHA256, "original_corpus_anchor_changed")
        extension, base_hash = read_json(root / "evals/coding/latest-runtime-v1/long-session.json", 262144)
        require(base_hash == BASE_SCENARIO_SHA256, "frozen_base_scenario_changed")
        validate_extension(extension, corpus, corpus_hash)
        contract, contract_hash = read_json(root / "evals/coding/owned-terminal-v2/scenarios.json", 262144)
        require(contract_hash == V2_SCENARIO_SHA256 and contract["version"] == 2
                and contract["base_scenario_sha256"] == base_hash and contract["original_corpus_sha256"] == corpus_hash
                and contract["image_id"] == image, "v2_frozen_contract_changed")
        require(build["source_tree"] == extension["source_tree"], "source_tree_binding_changed")
        plan, _ = read_json(root / "evals/coding/latest-runtime-v1/plan.json", 262144)
        ledger = Path(plan["ledger"]["path"])
        _, before = read_json(ledger, 4 * 1024 * 1024)
        require(before == plan["ledger"]["sha256"], "real_ledger_digest_changed")
        args.output.mkdir(parents=True, exist_ok=False)
        args.output.chmod(0o700)
        results = []
        for scenario in contract["scenarios"]:
            if args.scenario not in {"all", scenario["id"]}:
                continue
            case = args.output / scenario["id"]
            case.mkdir()
            task = next(value for value in corpus["tasks"] if value["id"] == scenario["base_task"])
            results.append(run_case(root, case, task, scenario, extension, image, build, contract, args.negative_case or "normal"))
        _, after = read_json(ledger, 4 * 1024 * 1024)
        require(after == before, "real_ledger_modified")
        report = {"kind": "owned_tui_external_observation_suite_v2", "external_observation_criteria": "completed",
                  "paid_model_calls": 0, "real_ledger_sha256": after, "all_stronger_private_criteria_verified": False,
                  "real_ledger_modified": False, "image_build_record": build, "v2_contract_sha256": contract_hash, "results": results}
        with (args.output / "summary.json").open("x") as output:
            json.dump(report, output, indent=2)
            output.write("\n")
        print(json.dumps(report, indent=2))
        return 0
    except (Refused, OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError) as error:
        print("Owned TUI driver refused: " + (str(error) if isinstance(error, Refused) else type(error).__name__), file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
