"""Offline continuation regressions with isolated ledgers, never live credentials."""
from copy import deepcopy
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
import preflight_continuation as preflight


def synthetic_ledger():
    rows = []
    for index in range(preflight.ORIGINAL_REQUESTS):
        if index < 39:
            upper, actual_input, actual_output, state = 39040, None, None, "unsettled"
        elif index == 39:
            upper, actual_input, actual_output, state = 132381, None, None, "reserved"
        elif index < 854:
            upper, actual_input, actual_output, state = 10000, 5000, 1000, "settled"
        else:
            upper, actual_input, actual_output, state = 132381, 120001, 4659, "settled"
        reservation = upper * 2 + 8192 * 10
        charged = reservation if state != "settled" else actual_input * 2 + actual_output * 10
        rows.append({"ordinal": index, "run": "fixture-" + str(index), "state": state,
                     "input_upper": upper, "output_max": 8192, "reserved_micros": reservation,
                     "charged_micros": charged, "input_tokens": actual_input, "output_tokens": actual_output,
                     "http_status": None if state == "reserved" else 200})
    return {"version": 1, "binding": deepcopy(preflight.BINDING), "closed": False, "requests": rows}


def synthetic_corpus():
    tasks = []
    for kind in sorted(preflight.KINDS):
        for language in sorted(preflight.LANGUAGES):
            for number in range(2):
                task_id = language + "-" + kind + "-" + str(number)
                tasks.append({"id": task_id, "category": kind, "language": language,
                              "prompt": "Fix the source and keep the existing checks.",
                              "files": {"source.txt": "before"}, "reference": {"source.txt": "after"},
                              "public_command": ["check", "source.txt"], "held_out": "independent check"})
    next(task for task in tasks if task["category"] == "multi-file" and task["language"] == "rust")["id"] = "rs-report-skips"
    next(task for task in tasks if task["category"] == "multi-file" and task["language"] == "python")["id"] = "py-create-only-export"
    return {"version": 4, "tasks": tasks}


def appended_row(ordinal, unknown=False, upper=100):
    row = {"ordinal": ordinal, "run": "fixture-new-" + str(ordinal),
           "state": "unsettled" if unknown else "settled", "input_upper": upper,
           "output_max": 8192, "reserved_micros": upper * 2 + 8192 * 10,
           "charged_micros": upper * 2 + 8192 * 10 if unknown else 20,
           "input_tokens": None if unknown else 10, "output_tokens": None if unknown else 0,
           "http_status": 502 if unknown else 200}
    return row


class Cases(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.ledger_path = self.root / "ledger.json"
        self.corpus_path = self.root / "tasks.json"
        self.manifest_path = self.root / "experiment.json"
        self.builds_path = self.root / "build-records.json"
        self.checkpoint_path = self.root / "original-checkpoint.json"
        self.ledger = synthetic_ledger()
        self.corpus = synthetic_corpus()
        self.write(self.ledger_path, self.ledger)
        self.write(self.corpus_path, self.corpus)
        self.checkpoint = preflight.make_original_checkpoint(self.ledger_path)
        self.write(self.checkpoint_path, self.checkpoint)
        self.builds = {"version": 1,
                       "control": {"source_commit": "a" * 40, "source_tree": "b" * 40, "image_id": "sha256:" + "c" * 64},
                       "candidate": {"source_commit": "d" * 40, "source_tree": "e" * 40, "image_id": "sha256:" + "f" * 64}}
        self.manifest = {"version": 1, "cohort": "selected_pair", "tasks": ["rs-report-skips"],
                         "corpus_version": 4, "corpus_sha256": self.hash(self.corpus_path),
                         "ledger_sha256": self.hash(self.ledger_path), "settings": dict(preflight.SETTINGS),
                         "arms": {"control": {**self.builds["control"], "options": {}},
                                  "candidate": {**self.builds["candidate"], "options": {
                                      "context": {"enabled": True, "max_bytes": 12288},
                                      "failure_control": {"repeat_limit": 5, "test_review_signals": False}}}}}
        self.refresh()

    def tearDown(self):
        self.temp.cleanup()

    def write(self, path, value):
        path.write_text(json.dumps(value) + "\n")

    def hash(self, path):
        return hashlib.sha256(path.read_bytes()).hexdigest()

    def refresh(self, ledger=True, corpus=True):
        self.write(self.ledger_path, self.ledger)
        self.write(self.corpus_path, self.corpus)
        if ledger:
            self.manifest["ledger_sha256"] = self.hash(self.ledger_path)
        if corpus:
            self.manifest["corpus_sha256"] = self.hash(self.corpus_path)
        self.write(self.builds_path, self.builds)
        self.write(self.manifest_path, self.manifest)
        self.write(self.checkpoint_path, self.checkpoint)

    def run_preflight(self):
        return preflight.preflight(self.manifest_path, self.builds_path, self.checkpoint_path,
                                   self.ledger_path, self.corpus_path)

    def refuses(self, code=None):
        if code:
            return self.assertRaisesRegex(preflight.Refused, "^" + code + "$")
        return self.assertRaises(preflight.Refused)

    def test_valid_preflight_retains_every_original_row_and_has_no_live_result(self):
        before = {path: path.read_bytes() for path in [self.ledger_path, self.corpus_path, self.manifest_path,
                                                      self.builds_path, self.checkpoint_path]}
        report = self.run_preflight()
        self.assertTrue(report["offline_checks_passed"])
        self.assertFalse(report["is_live_authorization"])
        self.assertIsNone(report["new_live_grades"])
        self.assertEqual(report["paid_model_calls"], 0)
        self.assertEqual(report["accounting"]["requests"], 855)
        self.assertEqual(report["accounting"]["unknown_requests"], 40)
        self.assertEqual(report["accounting"]["accounted_micros"], 23153274)
        self.assertEqual(report["accounting"]["remaining_micros"], 1846726)
        self.assertEqual(report["source_image_check"], "matches_operator_build_records")
        for path, original in before.items():
            self.assertEqual(path.read_bytes(), original)
        self.assertFalse(list(self.root.glob("*.next")))

    def test_known_and_unknown_appends_preserve_original_prefix_and_charges(self):
        for unknown in [False, True]:
            with self.subTest(unknown=unknown):
                self.ledger = synthetic_ledger()
                self.ledger["requests"].append(appended_row(855, unknown))
                self.refresh()
                report = self.run_preflight()
                self.assertEqual(report["accounting"]["requests"], 856)
                self.assertEqual(report["accounting"]["unknown_requests"], 40 + int(unknown))
                self.assertEqual(report["accounting"]["original_rows_sha256"], self.checkpoint["original_rows_sha256"])
                self.assertEqual(report["accounting"]["accounted_micros"], 23153274 + (82120 if unknown else 20))

    def test_original_row_change_is_refused_even_with_fresh_full_digest(self):
        self.ledger["requests"][40]["run"] = "changed-history"
        self.refresh()
        with self.refuses("changed_original_rows"):
            self.run_preflight()

    def test_unknown_refund_or_settlement_does_not_erase_retained_charge(self):
        for change in [{"charged_micros": 0}, {"state": "settled", "input_tokens": 0, "output_tokens": 0, "charged_micros": 0}]:
            with self.subTest(change=change):
                self.ledger = synthetic_ledger()
                self.ledger["requests"][0].update(change)
                self.refresh()
                with self.refuses():
                    self.run_preflight()

    def test_missing_or_empty_ledger_is_never_created_or_accepted(self):
        self.ledger_path.unlink()
        with self.refuses():
            self.run_preflight()
        self.assertFalse(self.ledger_path.exists())
        with self.refuses():
            preflight.make_original_checkpoint(self.ledger_path)
        self.assertFalse(self.ledger_path.exists())
        self.ledger["requests"] = []
        self.refresh()
        with self.refuses("missing_original_history"):
            self.run_preflight()

    def test_price_cap_fee_and_closed_state_cannot_change(self):
        changes = [{"limit": "26.00"}, {"limit": "0.00"},
                   {"prices": {**preflight.BINDING["prices"], "input_per_million": "1"}},
                   {"prices": {**preflight.BINDING["prices"], "request_fee": "0.01"}}]
        for change in changes:
            with self.subTest(change=change):
                self.ledger = synthetic_ledger()
                self.ledger["binding"].update(change)
                self.refresh()
                with self.refuses("changed_ledger_binding"):
                    self.run_preflight()
        self.ledger = synthetic_ledger()
        self.ledger["closed"] = True
        self.refresh()
        with self.refuses("closed_ledger"):
            self.run_preflight()

    def test_over_cap_is_refused_and_at_cap_forecast_grants_nothing(self):
        self.ledger["requests"].extend([appended_row(855, True, 270000), appended_row(856, True, 270000),
                                        appended_row(857, True, 260483)])
        self.refresh()
        report = self.run_preflight()
        self.assertEqual(report["accounting"]["remaining_micros"], 0)
        self.assertTrue(report["forecast"]["upper_forecast_exceeds_remaining"])
        self.assertFalse(report["forecast"]["is_live_authorization"])
        self.ledger["requests"].append(appended_row(858, True))
        self.refresh()
        with self.refuses("exceeded_approved_cap"):
            self.run_preflight()

    def test_invalid_accounting_shapes_fail_with_fixed_diagnostics(self):
        mutations = [{"ordinal": False}, {"ordinal": 7}, {"state": []}, {"run": []},
                     {"charged_micros": -1}, {"charged_micros": 999999}, {"input_upper": 270001},
                     {"output_max": 8193}, {"output_max": True}, {"http_status": False},
                     {"input_tokens": False}, {"input_tokens": 50000}]
        for change in mutations:
            with self.subTest(change=change):
                self.ledger = synthetic_ledger()
                self.ledger["requests"][40].update(change)
                self.refresh()
                with self.refuses():
                    self.run_preflight()

    def test_current_digest_and_original_checkpoint_both_bind_ledger(self):
        self.ledger["requests"].append(appended_row(855))
        self.refresh(ledger=False)
        with self.refuses("ledger_digest_changed"):
            self.run_preflight()
        self.refresh()
        self.checkpoint["original_rows_sha256"] = "0" * 64
        self.write(self.checkpoint_path, self.checkpoint)
        with self.refuses("changed_original_rows"):
            self.run_preflight()

    def test_different_ledger_path_and_active_owner_are_refused(self):
        self.checkpoint["ledger_path"] = str(self.root / "other" / "ledger.json")
        self.write(self.checkpoint_path, self.checkpoint)
        with self.refuses("different_ledger_path"):
            self.run_preflight()
        self.checkpoint["ledger_path"] = str(self.ledger_path)
        self.write(self.checkpoint_path, self.checkpoint)
        (self.root / "gateway-owner.json").write_text("owned")
        with self.refuses("gateway_owner_active"):
            self.run_preflight()

    def test_unknown_duplicate_and_partial_full_selections_refuse_before_ledger_read(self):
        for selection in [["rs-report-skips", "unknown-task"], ["rs-report-skips", "rs-report-skips"], []]:
            with self.subTest(selection=selection):
                self.manifest["tasks"] = selection
                self.write(self.manifest_path, self.manifest)
                with mock.patch.object(preflight, "read_json", wraps=preflight.read_json) as reader:
                    with self.refuses():
                        self.run_preflight()
                    self.assertNotIn(self.ledger_path, [Path(call.args[0]) for call in reader.call_args_list])
        self.manifest["cohort"] = "full_suite_forecast"
        self.manifest["tasks"] = [task["id"] for task in self.corpus["tasks"]][:-1]
        self.write(self.manifest_path, self.manifest)
        with mock.patch.object(preflight, "read_json", wraps=preflight.read_json) as reader:
            with self.refuses("incomplete_full_suite"):
                self.run_preflight()
            self.assertNotIn(self.ledger_path, [Path(call.args[0]) for call in reader.call_args_list])

    def test_selection_order_is_explicit_and_full_forecast_is_advisory(self):
        self.manifest["tasks"] = ["py-create-only-export", "rs-report-skips"]
        self.refresh()
        report = self.run_preflight()
        self.assertEqual(report["tasks"], self.manifest["tasks"])
        self.assertEqual(report["forecast"]["observed_forecast_range_micros"], [774264, 1017304])
        self.assertFalse(report["forecast"]["is_live_authorization"])
        self.manifest["cohort"] = "full_suite_forecast"
        self.manifest["tasks"] = [task["id"] for task in self.corpus["tasks"]]
        self.refresh()
        full = self.run_preflight()
        self.assertEqual(full["forecast"]["paired_attempts"], 60)
        self.assertTrue(full["forecast"]["upper_forecast_exceeds_remaining"])
        self.assertIsNone(full["new_live_grades"])

    def test_corpus_changes_duplicates_and_unsafe_paths_refuse(self):
        for change in ["version", "duplicate", "unbalanced", "parent", "git", "windows", "surrogate"]:
            with self.subTest(change=change):
                self.corpus = synthetic_corpus()
                if change == "version":
                    self.corpus["version"] = 5
                elif change == "duplicate":
                    self.corpus["tasks"][2]["id"] = self.corpus["tasks"][0]["id"]
                elif change == "unbalanced":
                    self.corpus["tasks"][2]["category"] = "feature"
                elif change == "surrogate":
                    self.corpus["tasks"][2]["prompt"] = "\ud800"
                else:
                    name = {"parent": "../escape", "git": ".git/hooks/pre-commit", "windows": "C:\\escape"}[change]
                    self.corpus["tasks"][2]["files"] = {name: "invalid"}
                self.refresh()
                with self.refuses():
                    self.run_preflight()
        self.corpus = synthetic_corpus()
        self.refresh()
        self.corpus["tasks"][0]["prompt"] += " changed"
        self.refresh(corpus=False)
        with self.refuses("corpus_digest_changed"):
            self.run_preflight()

    def test_source_tree_image_records_and_immutable_ids_are_checked(self):
        for key, value in [("source_commit", "0" * 40), ("source_tree", "1" * 40), ("image_id", "sha256:" + "2" * 64)]:
            with self.subTest(key=key):
                builds = deepcopy(self.builds)
                builds["candidate"][key] = value
                self.write(self.builds_path, builds)
                with self.refuses("source_image_binding_changed"):
                    self.run_preflight()
        self.write(self.builds_path, self.builds)
        for value in [None, "latest", "sha256:short"]:
            self.manifest["arms"]["candidate"]["image_id"] = value
            self.write(self.manifest_path, self.manifest)
            with self.refuses("invalid_image_id"):
                self.run_preflight()

    def test_model_effort_and_all_execution_limits_remain_identical(self):
        for key, value in [("model", "other"), ("protocol", "anthropic_messages"), ("effort", "max"),
                           ("max_output_tokens", 4096), ("max_requests_per_run", 21), ("task_wall_seconds", 900)]:
            with self.subTest(key=key):
                changed = deepcopy(self.manifest)
                changed["settings"][key] = value
                self.write(self.manifest_path, changed)
                with self.refuses("changed_experiment_settings"):
                    self.run_preflight()

    def test_feature_whitelist_preserves_absence_and_input(self):
        self.assertEqual(preflight.validate_feature_options({}), {})
        options = {"context": {"enabled": True}, "failure_control": {"repeat_limit": 3}}
        original = deepcopy(options)
        checked = preflight.validate_feature_options(options)
        self.assertEqual(checked, options)
        checked["context"]["enabled"] = False
        self.assertEqual(options, original)
        for maximum in [2048, 32768]:
            self.assertEqual(preflight.validate_feature_options({"context": {"enabled": True, "max_bytes": maximum}})["context"]["max_bytes"], maximum)
        for count in [3, 20]:
            self.assertEqual(preflight.validate_feature_options({"failure_control": {"repeat_limit": count, "test_review_signals": False}})["failure_control"]["repeat_limit"], count)

    def test_feature_whitelist_refuses_credentials_services_permissions_and_wrong_types(self):
        values = [{"api_key": "must-not-be-forwarded"}, {"mcp_servers": []}, {"permission_mode": "bypass"},
                  {"context": {"enabled": True, "env": {}}}, {"context": {"enabled": 1}},
                  {"context": {"enabled": True, "max_bytes": True}}, {"context": {"enabled": True, "max_bytes": 2047}},
                  {"context": {"enabled": True, "max_bytes": 32769}}, {"context": None},
                  {"failure_control": {"repeat_limit": 2}}, {"failure_control": {"repeat_limit": 21}},
                  {"failure_control": {"repeat_limit": False}}, {"failure_control": {"repeat_limit": 5, "test_review_signals": 1}}]
        for value in values:
            with self.subTest(value=value), self.refuses():
                preflight.validate_feature_options(value)

    def test_duplicate_json_nonfinite_and_unhashable_values_have_fixed_errors(self):
        self.manifest_path.write_text('{"version":1,"version":2}')
        with self.refuses("duplicate_json_key"):
            self.run_preflight()
        self.manifest_path.write_text('{"value":NaN}')
        with self.refuses("nonfinite_json"):
            self.run_preflight()
        for field, value in [("cohort", []), ("tasks", {}), ("arms", []), ("version", True)]:
            changed = deepcopy(self.manifest)
            changed[field] = value
            self.write(self.manifest_path, changed)
            with self.refuses():
                self.run_preflight()

    def test_configuration_named_inputs_are_refused_before_open(self):
        for name in ["config.json", "CONFIG.JSON", "config.toml", "settings.local.json", ".env.private"]:
            with self.subTest(name=name):
                config = self.root / name
                config.write_text('{"api_key":"must-not-be-read"}')
                with mock.patch.object(preflight.os, "open", side_effect=AssertionError("input must not be opened")):
                    with self.refuses("configuration_input_refused"):
                        preflight.read_json(config, 1024)

    def test_initial_checkpoint_digest_and_returned_binding_are_independent(self):
        captured = preflight.make_original_checkpoint(self.ledger_path)
        captured["binding"]["limit"] = "26.00"
        self.assertEqual(preflight.BINDING["limit"], "25.00")
        self.checkpoint["initial_ledger_sha256"] = "0" * 64
        self.write(self.checkpoint_path, self.checkpoint)
        with self.refuses("changed_initial_ledger_digest"):
            self.run_preflight()

    def test_symlink_and_hardlink_inputs_are_refused_before_open(self):
        link = self.root / "linked.json"
        try:
            link.symlink_to(self.ledger_path)
        except OSError:
            self.skipTest("Native symlink creation is unavailable for this isolated fixture")
        with mock.patch.object(preflight.os, "open", side_effect=AssertionError("link must not be opened")):
            with self.refuses("nonregular_input"):
                preflight.read_json(link, 1024 * 1024)
        hardlink = self.root / "hardlinked.json"
        os.link(self.corpus_path, hardlink)
        with self.refuses("nonregular_input"):
            preflight.read_json(hardlink, 1024 * 1024)

    def test_exact_read_bound_and_actual_changed_file_are_detected(self):
        source = self.root / "small.json"
        source.write_bytes(b'{}')
        self.assertEqual(preflight.read_json(source, 2)[0], {})
        with self.refuses("input_too_large"):
            preflight.read_json(source, 1)
        real_read = preflight.os.read
        changed = False
        def mutate(fd, count):
            nonlocal changed
            value = real_read(fd, count)
            if not changed:
                changed = True
                source.write_bytes(b'{"changed":true}')
            return value
        with mock.patch.object(preflight.os, "read", side_effect=mutate):
            with self.refuses("input_changed"):
                preflight.read_json(source, 1024)

    def cli(self, output):
        command = [sys.executable, "-I", "-B", str(Path(preflight.__file__)),
                   "--manifest", str(self.manifest_path), "--builds", str(self.builds_path),
                   "--checkpoint", str(self.checkpoint_path), "--ledger", str(self.ledger_path),
                   "--corpus", str(self.corpus_path), "--output", str(output)]
        return subprocess.run(command, capture_output=True, text=True, timeout=20)

    def test_actual_cli_report_is_create_only_and_cannot_replace_ledger(self):
        output = self.root / "preflight-report.json"
        before = self.ledger_path.read_bytes()
        result = self.cli(output)
        self.assertEqual(result.returncode, 0, result.stderr)
        report = json.loads(output.read_text())
        self.assertEqual(report, json.loads(result.stdout))
        self.assertEqual(self.ledger_path.read_bytes(), before)
        second = self.cli(output)
        self.assertEqual(second.returncode, 2)
        self.assertIn("output_unavailable", second.stderr)
        self.assertEqual(json.loads(output.read_text()), report)
        collision = self.cli(self.ledger_path)
        self.assertEqual(collision.returncode, 2)
        self.assertEqual(self.ledger_path.read_bytes(), before)

    def test_actual_cli_invalid_manifest_does_not_echo_sensitive_values(self):
        self.manifest["api_key"] = "sensitive-marker-must-not-appear"
        self.write(self.manifest_path, self.manifest)
        result = self.cli(self.root / "should-not-exist.json")
        self.assertEqual(result.returncode, 2)
        self.assertNotIn("sensitive-marker-must-not-appear", result.stdout + result.stderr)
        self.assertFalse((self.root / "should-not-exist.json").exists())


if __name__ == "__main__":
    unittest.main()
