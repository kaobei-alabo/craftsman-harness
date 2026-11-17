"""Deterministic grader checks; synthetic captures are not runtime evidence."""
from copy import deepcopy
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
import grade_long_session as long
from preflight_continuation import Refused, canonical_sha256


ROOT = Path(__file__).resolve().parents[2]
CORPUS = json.loads((ROOT / "evals/coding/tasks.json").read_text())
EXTENSION_PATH = ROOT / "evals/coding/latest-runtime-v1/long-session.json"
EXTENSION = json.loads(EXTENSION_PATH.read_text())


def synthetic_capture(scenario):
    """A fabricated unit-test input, deliberately never reported as a real session."""
    task = next(row for row in CORPUS["tasks"] if row["id"] == scenario["base_task"])
    command = scenario["probe_command"] or "python3 -B test_public.py"
    content = "Public checks passed."
    if scenario["id"] == "interruption-reopen-v1":
        content = "created I1\n"
    if scenario["id"] == "retained-results-v1":
        answer = scenario["expected_answer"]
        content = answer["stdout"] + "\n--- stderr ---\n" + answer["stderr"] + "\n[exit code 7]"
    messages = [
        {"role": "user", "content": task["prompt"]},
        {"role": "assistant", "content": "", "tool_calls": [
            {"id": "task-check", "name": "execute", "args": {"command": "python3 -B test_public.py"}}]},
        {"role": "tool", "tool_call_id": "task-check", "content": "Public checks passed.", "is_error": False},
        {"role": "assistant", "content": "Original task completed."},
        {"role": "user", "content": scenario["setup_prompt"]},
        {"role": "assistant", "content": "", "tool_calls": [
            {"id": "original-tool", "name": "execute", "args": {"command": command}}]},
        {"role": "tool", "tool_call_id": "original-tool", "content": content, "is_error": False},
        {"role": "assistant", "content": "Scenario effect completed."},
    ]
    for update in long.updates(scenario):
        messages.extend([{"role": "user", "content": update}, {"role": "assistant", "content": "Acknowledged."}])
    state = {"messages": messages, "files": {"retained.txt": "actual fixture state"},
             "todos": [{"content": "Retain the original result", "status": "completed"}],
             "custom": {"_craftsman_context_v1": {"compactions": 0}}}
    before = {"thread": "synthetic-owned-session", "seq": 10, "state": deepcopy(state)}
    after = deepcopy(before)
    after["seq"] = 11
    action = "cancelled_return_then_reopen"
    if scenario["id"] == "compaction-v1":
        after["state"]["messages"] = [{"role": "user", "content": "Synthetic unit-test summary."}]
        after["state"]["custom"]["_craftsman_context_v1"]["compactions"] = 1
        action = "manual_compaction_saved"
    elif scenario["id"] == "retained-results-v1":
        action = "probe_result_saved_then_reopen"
    reopened = deepcopy(after)
    final = deepcopy(reopened)
    final["seq"] = 12
    final["state"]["messages"].extend([{"role": "user", "content": scenario["final_prompt"]},
                                       {"role": "assistant", "content": "The retained outcome was written."}])
    files = {path: value.encode() for path, value in {**task["files"], **task["reference"], **scenario["fixture_files"]}.items()}
    effect = scenario["expected_effect"].encode()
    files[scenario["effect_file"]] = effect
    files["session_result.json"] = json.dumps(scenario["expected_answer"], ensure_ascii=False).encode()
    if scenario["id"] == "retained-results-v1":
        files["probe_input.txt"] = b"changed after the original observation\n"
    capture = {"version": 1, "kind": "operator_long_session_capture", "scenario_id": scenario["id"],
        "source_commit": EXTENSION["source_commit"], "source_tree": EXTENSION["source_tree"],
        "feature_options_sha256": EXTENSION["feature_options_sha256"],
        "effective_model_settings_sha256": EXTENSION["effective_model_settings_sha256"],
        "checkpoints": {"before": before, "after": after, "reopened": reopened, "final": final},
        "gateway_requests": {"before_reopen": 15, "after_idle_reopen": 15, "final": 18},
        "effect_sha256_by_stage": {stage: hashlib.sha256(effect).hexdigest() for stage in long.STAGES},
        "completed_update_turns": 10, "idle_reopen_elapsed_millis": 5000, "observed_action": action}
    capture["checkpoints"] = {stage: long.safe_checkpoint_observation(value, scenario)
                              for stage, value in capture["checkpoints"].items()}
    return task, capture, files


class Cases(unittest.TestCase):
    def grade(self, scenario, capture, files):
        task = next(row for row in CORPUS["tasks"] if row["id"] == scenario["base_task"])
        return long.grade_capture(scenario, task, capture, files, EXTENSION)

    def test_versioned_extension_binds_all_three_original_tasks_and_fixed_limits(self):
        corpus_hash = hashlib.sha256((ROOT / "evals/coding/tasks.json").read_bytes()).hexdigest()
        checked = long.validate_extension(EXTENSION, CORPUS, corpus_hash)
        self.assertEqual([row["id"] for row in checked["scenarios"]], list(long.SCENARIOS))
        self.assertEqual(checked["limits"]["max_model_requests_total"], 40)
        self.assertEqual(checked["limits"]["scenario_wall_seconds"], 900)
        for scenario in checked["scenarios"]:
            self.assertEqual([len(value.encode()) for value in long.updates(scenario)], [8192] * 10)

    def test_each_synthetic_reference_capture_passes_without_becoming_runtime_evidence(self):
        for scenario in EXTENSION["scenarios"]:
            with self.subTest(scenario=scenario["id"]):
                _, capture, files = synthetic_capture(scenario)
                report = self.grade(scenario, capture, files)
                self.assertTrue(report["observation_contract_passed"])
                self.assertFalse(report["operator_capture_provenance_authenticated"])
                self.assertFalse(report["synthetic_capture_checks_are_runtime_evidence"])

    def test_missing_or_incorrect_final_answer_fails_each_scenario(self):
        for scenario in EXTENSION["scenarios"]:
            _, capture, files = synthetic_capture(scenario)
            files["session_result.json"] = b'{}'
            with self.subTest(scenario=scenario["id"]), self.assertRaises(Refused):
                self.grade(scenario, capture, files)

    def test_original_public_test_cannot_be_weakened_to_make_a_soak_pass(self):
        for scenario in EXTENSION["scenarios"]:
            _, capture, files = synthetic_capture(scenario)
            files["test_public.py"] = b"assert True\n"
            with self.subTest(scenario=scenario["id"]), self.assertRaisesRegex(Refused, "protected_fixture_changed"):
                self.grade(scenario, capture, files)

    def test_reopen_cannot_sample_or_switch_sessions(self):
        scenario = EXTENSION["scenarios"][1]
        _, capture, files = synthetic_capture(scenario)
        capture["gateway_requests"]["after_idle_reopen"] += 1
        with self.assertRaisesRegex(Refused, "reopen_sampled_or_followup_absent"):
            self.grade(scenario, capture, files)
        _, capture, files = synthetic_capture(scenario)
        capture["checkpoints"]["reopened"]["session_sha256"] = "0" * 64
        with self.assertRaisesRegex(Refused, "different_session_reopened"):
            self.grade(scenario, capture, files)

    def test_compaction_requires_actual_replacement_and_saved_context_counter(self):
        scenario = EXTENSION["scenarios"][0]
        for mutation in ("messages", "counter", "files", "return"):
            _, capture, files = synthetic_capture(scenario)
            if mutation == "messages":
                capture["checkpoints"]["after"]["message_count"] = capture["checkpoints"]["before"]["message_count"]
            elif mutation == "counter":
                capture["checkpoints"]["after"]["compactions"] = 0
            elif mutation == "files":
                capture["checkpoints"]["before"]["files_sha256"] = "0" * 64
            else:
                capture["observed_action"] = "summary_requested_only"
            with self.subTest(mutation=mutation), self.assertRaises(Refused):
                self.grade(scenario, capture, files)

    def test_original_results_are_group_bound_and_retained_after_reopen(self):
        scenario = EXTENSION["scenarios"][1]
        _, capture, files = synthetic_capture(scenario)
        capture["checkpoints"]["final"]["completed_groups"][0]["sha256"] = "0" * 64
        with self.assertRaisesRegex(Refused, "original_tool_result_lost_or_replayed"):
            self.grade(scenario, capture, files)
        state = {"messages": [
            {"role": "assistant", "tool_calls": [{"id": "original-tool", "name": "execute", "args": {"command": "first command"}}]},
            {"role": "tool", "tool_call_id": "original-tool", "content": "original result", "is_error": False}]}
        original_group = long.completed_groups(state)
        state["messages"].extend([
            {"role": "assistant", "tool_calls": [{"id": "original-tool", "name": "execute", "args": {"command": "different command"}}]},
            {"role": "tool", "tool_call_id": "original-tool", "content": "new group result", "is_error": False}])
        self.assertEqual(sum(long.completed_groups(state).values()), sum(original_group.values()) + 1)

    def test_duplicate_original_result_is_not_accepted_as_retention(self):
        scenario = EXTENSION["scenarios"][1]
        _, capture, files = synthetic_capture(scenario)
        capture["checkpoints"]["final"]["completed_groups"][0]["count"] += 1
        with self.assertRaisesRegex(Refused, "original_tool_result_lost_or_replayed"):
            self.grade(scenario, capture, files)

    def test_create_only_effect_grades_json_semantics_and_original_bytes_stay_fixed(self):
        scenario = EXTENSION["scenarios"][1]
        _, capture, files = synthetic_capture(scenario)
        compact = b'{"ticket":"I1","count":1}\n'
        files[scenario["effect_file"]] = compact
        capture["effect_sha256_by_stage"] = {stage: hashlib.sha256(compact).hexdigest() for stage in long.STAGES}
        self.assertTrue(self.grade(scenario, capture, files)["observation_contract_passed"])
        capture["effect_sha256_by_stage"]["before"] = "0" * 64
        with self.assertRaisesRegex(Refused, "effect_changed_since_original_result"):
            self.grade(scenario, capture, files)

    def test_repeated_probe_and_incorrect_observed_exit_are_failures(self):
        scenario = EXTENSION["scenarios"][2]
        _, capture, files = synthetic_capture(scenario)
        files["probe_calls.txt"] += b"invoked\n"
        with self.assertRaisesRegex(Refused, "effect_missing_changed_or_replayed"):
            self.grade(scenario, capture, files)
        _, capture, files = synthetic_capture(scenario)
        for value in capture["checkpoints"].values():
            value["selected_once_result_sha256"] = [hashlib.sha256(b"incorrect exit observation").hexdigest()]
        with self.assertRaisesRegex(Refused, "actual_probe_result_not_observed"):
            self.grade(scenario, capture, files)

    def test_probe_replay_with_a_fresh_call_id_still_fails(self):
        scenario = EXTENSION["scenarios"][2]
        _, capture, files = synthetic_capture(scenario)
        capture["checkpoints"]["final"]["selected_once_command_count"] = 2
        with self.assertRaisesRegex(Refused, "probe_missing_or_replayed"):
            self.grade(scenario, capture, files)

    def test_missing_accepted_update_and_short_idle_observation_fail(self):
        scenario = EXTENSION["scenarios"][1]
        _, capture, files = synthetic_capture(scenario)
        capture["checkpoints"]["before"]["accepted_input_sha256"].pop()
        with self.assertRaisesRegex(Refused, "missing_original_update_input"):
            self.grade(scenario, capture, files)
        _, capture, files = synthetic_capture(scenario)
        capture["idle_reopen_elapsed_millis"] = 4999
        with self.assertRaises(Refused):
            self.grade(scenario, capture, files)

    def test_capture_cannot_change_recorded_source_options_or_model_settings(self):
        scenario = EXTENSION["scenarios"][0]
        for field in ("source_commit", "source_tree", "feature_options_sha256", "effective_model_settings_sha256"):
            _, capture, files = synthetic_capture(scenario)
            capture[field] = "0" * len(capture[field])
            with self.subTest(field=field), self.assertRaisesRegex(Refused, "different_long_session_capture_settings"):
                self.grade(scenario, capture, files)

    def test_new_checkpoint_observations_omit_prompt_body_endpoints_and_credentials(self):
        scenario = EXTENSION["scenarios"][2]
        state = {"messages": [
            {"role": "user", "content": "synthetic-private-prompt-marker"},
            {"role": "assistant", "tool_calls": [{"id": "private-id", "name": "execute", "args": {
                "command": "synthetic-private-body-marker", "api_key": "synthetic-private-key-marker"}}]},
            {"role": "tool", "tool_call_id": "private-id", "content": "synthetic-provider-body-marker", "is_error": True}],
            "files": {"private.txt": "synthetic-file-text-marker"},
            "todos": [{"content": "synthetic-todo-text-marker"}],
            "custom": {"endpoint": "synthetic-endpoint-marker"}}
        observation = long.safe_checkpoint_observation({"thread": "synthetic-session-marker", "seq": 1, "state": state}, scenario)
        long.safe_checkpoint(observation)
        encoded = json.dumps(observation)
        for marker in ("synthetic-private-prompt-marker", "synthetic-private-body-marker", "synthetic-private-key-marker",
                       "synthetic-provider-body-marker", "synthetic-file-text-marker", "synthetic-todo-text-marker",
                       "synthetic-endpoint-marker", "synthetic-session-marker"):
            self.assertNotIn(marker, encoded)
        self.assertEqual(observation["message_count"], 3)
        self.assertEqual(len(observation["completed_groups"]), 1)

    def test_real_marked_probe_has_exact_stdout_stderr_exit_and_unmarked_no_effect(self):
        scenario = EXTENSION["scenarios"][2]
        with tempfile.TemporaryDirectory(prefix="craftsman-soak-probe-") as directory:
            root = Path(directory)
            for path, value in scenario["fixture_files"].items():
                (root / path).write_text(value)
            script = root / "probe_once.py"
            unmarked = subprocess.run([sys.executable, "-I", "-B", str(script)], cwd=root,
                capture_output=True, timeout=20)
            self.assertEqual(unmarked.returncode, 0)
            self.assertEqual(unmarked.stdout + unmarked.stderr, b"")
            self.assertFalse((root / "probe_calls.txt").exists())
            marked = subprocess.run([sys.executable, "-I", "-B", str(script), "--craftsman-soak-v1"], cwd=root,
                capture_output=True, timeout=20)
            self.assertEqual(marked.returncode, 7)
            self.assertEqual(marked.stdout, scenario["expected_answer"]["stdout"].encode())
            self.assertEqual(marked.stderr, scenario["expected_answer"]["stderr"].encode())
            self.assertEqual((root / "probe_calls.txt").read_bytes(), b"invoked\n")

    def test_real_marked_create_only_effect_uses_the_original_api_and_refuses_replay(self):
        scenario = EXTENSION["scenarios"][1]
        task = next(row for row in CORPUS["tasks"] if row["id"] == scenario["base_task"])
        with tempfile.TemporaryDirectory(prefix="craftsman-soak-effect-") as directory:
            root = Path(directory)
            for path, value in {**task["files"], **task["reference"], **scenario["fixture_files"]}.items():
                (root / path).write_text(value)
            script = root / "effect_once.py"
            unmarked = subprocess.run([sys.executable, "-I", "-B", str(script)], cwd=root, capture_output=True, timeout=20)
            self.assertEqual(unmarked.returncode, 0)
            self.assertFalse((root / "effect.jsonl").exists())
            marked = subprocess.run([sys.executable, "-I", "-B", str(script), "--craftsman-soak-v1"], cwd=root,
                                    capture_output=True, timeout=20)
            self.assertEqual(marked.returncode, 0)
            self.assertEqual(marked.stdout, b"created I1\n")
            self.assertEqual(marked.stderr, b"")
            effect = (root / "effect.jsonl").read_bytes()
            self.assertEqual(json.loads(effect), {"ticket": "I1", "count": 1})
            repeated = subprocess.run([sys.executable, "-I", "-B", str(script), "--craftsman-soak-v1"], cwd=root,
                                      capture_output=True, timeout=20)
            self.assertNotEqual(repeated.returncode, 0)
            self.assertEqual((root / "effect.jsonl").read_bytes(), effect)


if __name__ == "__main__":
    unittest.main()
