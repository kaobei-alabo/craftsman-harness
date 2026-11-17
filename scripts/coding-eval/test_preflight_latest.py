"""Offline preparation checks using owned Git repositories and synthetic ledgers."""
from copy import deepcopy
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
import preflight_latest as latest
import preflight_continuation as original
from test_preflight_continuation import appended_row, synthetic_ledger


ROOT = Path(__file__).resolve().parents[2]
PLAN = ROOT / "evals/coding/latest-runtime-v1/plan.json"
CORPUS_PATH = "evals/coding/tasks.json"


def frozen_corpus_fixture(root):
    """Seed source33 bytes from the current blob, never its checkout projection."""
    data = latest.git(root, "cat-file", "blob", "HEAD:" + CORPUS_PATH)
    header = b"blob " + str(len(data)).encode("ascii") + b"\0"
    # Independently verified against 33a80d9; no caller/working-copy rehash.
    original.require(len(data) == 80137
                     and hashlib.sha256(data).hexdigest() == latest.ORIGINAL_CORPUS_SHA256
                     and hashlib.sha1(header + data).hexdigest() == "95f8dd7a6336c68f844ec7decb61e92f189bb955",
                     "source_corpus_differs")
    return data


class Cases(unittest.TestCase):
    def write(self, path, value):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(value, ensure_ascii=False) + "\n")

    def hash(self, path):
        return hashlib.sha256(path.read_bytes()).hexdigest()

    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="craftsman-latest-preflight-")
        self.root = Path(self.temporary.name)
        self.plan = json.loads(PLAN.read_text())
        self.plan_path = self.root / "plan.json"
        self.corpus_path = self.root / self.plan["corpus"]["path"]
        self.corpus_path.parent.mkdir(parents=True)
        self.corpus_path.write_bytes(frozen_corpus_fixture(ROOT))
        self.corpus = json.loads(self.corpus_path.read_bytes())
        self.options_path = self.root / self.plan["feature_options"]["path"]
        self.options_path.parent.mkdir(parents=True)
        options = latest.git(ROOT, "cat-file", "blob", "HEAD:" + self.plan["feature_options"]["path"])
        original.require(hashlib.sha256(options).hexdigest() == self.plan["feature_options"]["sha256"],
                         "feature_options_digest_changed")
        self.options_path.write_bytes(options)
        self.extension_path = self.root / self.plan["long_session_corpus"]["path"]
        self.extension = json.loads((ROOT / self.plan["long_session_corpus"]["path"]).read_text())
        self.runtime_file = self.root / "crates/fixture/src.txt"
        self.runtime_file.parent.mkdir(parents=True)
        self.runtime_file.write_text("original runtime fixture\n")
        commands = [
            ["git", "init", "--quiet", str(self.root)],
            ["git", "-C", str(self.root), "config", "core.autocrlf", "false"],
            ["git", "-C", str(self.root), "add", "--all"],
            ["git", "-C", str(self.root), "-c", "core.hooksPath=/dev/null", "-c", "commit.gpgsign=false",
             "-c", "user.name=Evaluation fixture", "-c", "user.email=fixture@localhost",
             "commit", "--quiet", "--no-gpg-sign", "-m", "Synthetic fixture source"],
        ]
        for command in commands:
            subprocess.run(command, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=20)
        commit = latest.git(self.root, "rev-parse", "HEAD").decode().strip()
        tree = latest.git(self.root, "rev-parse", "HEAD^{tree}").decode().strip()
        self.plan["source"].update(commit=commit, tree=tree,
            runtime_inventory_sha256=original.canonical_sha256(latest.runtime_inventory(self.root, commit)))
        self.extension.update(source_commit=commit, source_tree=tree)
        self.ledger_path = self.root / "synthetic-budget/ledger.json"
        self.ledger = synthetic_ledger()
        self.write(self.ledger_path, self.ledger)
        self.checkpoint_path = self.root / "original-checkpoint.json"
        self.write(self.checkpoint_path, original.make_original_checkpoint(self.ledger_path))
        for index in range(60):
            row = appended_row(855 + index)
            row.update(input_upper=200000, reserved_micros=481920)
            if index < 59:
                row.update(input_tokens=1000, output_tokens=1000, charged_micros=12000)
            else:
                row.update(input_tokens=171370, output_tokens=8192, charged_micros=424660)
            self.ledger["requests"].append(row)
        self.plan["ledger"]["path"] = str(self.ledger_path)
        self.plan["checkpoint"]["path"] = str(self.checkpoint_path)
        self.refresh()

    def tearDown(self):
        self.temporary.cleanup()

    def refresh(self):
        self.write(self.ledger_path, self.ledger)
        self.write(self.extension_path, self.extension)
        facts = original.validate_ledger(self.ledger)
        self.plan["ledger"].update(sha256=self.hash(self.ledger_path), request_count=facts["requests"],
                                   accounted_micros=facts["accounted_micros"], unknown_requests=facts["unknown_requests"])
        self.plan["checkpoint"]["sha256"] = self.hash(self.checkpoint_path)
        self.plan["long_session_corpus"]["sha256"] = self.hash(self.extension_path)
        self.plan["feature_options"]["sha256"] = self.hash(self.options_path)
        self.plan["corpus"]["sha256"] = self.hash(self.corpus_path)
        self.write(self.plan_path, self.plan)

    def prepare(self, build_path=None):
        return latest.prepare(self.root, self.plan_path, build_path)

    def test_read_only_preparation_has_no_live_grade_lease_or_new_allowance(self):
        paths = [self.plan_path, self.corpus_path, self.extension_path, self.options_path,
                 self.ledger_path, self.checkpoint_path, self.runtime_file]
        before = {path: path.read_bytes() for path in paths}
        report = self.prepare()
        self.assertTrue(report["offline_checks_passed"])
        self.assertFalse(report["is_live_authorization"])
        self.assertFalse(report["live_execution_ready"])
        self.assertFalse(report["configuration_or_credentials_loaded"])
        self.assertFalse(report["execution_lease_established"])
        self.assertFalse(report["ledger_modified"])
        self.assertIsNone(report["new_live_grades"])
        self.assertIsNone(report["source"]["image_id"])
        self.assertEqual(report["paid_model_calls"], 0)
        self.assertEqual(report["accounting"]["accounted_micros"], 24285934)
        self.assertEqual(report["accounting"]["unknown_requests"], 40)
        for path in paths:
            self.assertEqual(path.read_bytes(), before[path])
        self.assertFalse(list(self.root.rglob("*.next")))
        self.assertFalse((self.ledger_path.parent / "gateway-owner.json").exists())

    def test_pricing_separates_empirical_plan_from_fully_reserved_requests(self):
        costs = self.prepare()["pricing_plan"]
        self.assertFalse(costs["proposal_authorized"])
        self.assertFalse(costs["present_allowance_can_fund_plan"])
        self.assertEqual(costs["planning_total_micros"], 24500000)
        self.assertEqual(costs["planning_buffer_micros"], 1214066)
        self.assertEqual(costs["maximum_one_request_reservation_micros"], 621920)
        self.assertEqual(costs["all_cohorts_fully_reserved_micros"], 522412800)
        self.assertEqual(sum(costs["maximum_requests_by_cohort"].values()), 840)

    def test_persisted_admissions_do_not_fabricate_attempts_refusals_or_usage(self):
        ledger = deepcopy(self.ledger)
        for row in ledger["requests"][:20]:
            row["run"] = "synthetic-at-twenty"
        original.validate_ledger(ledger)
        observations = latest.admission_observations(ledger)
        row = next(item for item in observations["per_run"] if item["run"] == "synthetic-at-twenty")
        self.assertEqual(row["admitted"], 20)
        self.assertEqual(row["unknown_usage"], 20)
        self.assertIsNone(row["attempted"])
        self.assertIsNone(row["refused"])
        self.assertFalse(observations["twenty_admissions_prove_a_twenty_first_attempt"])
        self.assertFalse(observations["attempts_recorded"])
        self.assertFalse(observations["refusals_recorded"])

    def test_full_suite_and_repeats_keep_every_case_and_declared_order(self):
        rows = self.prepare()["planned_runner_invocations"]
        self.assertEqual(len(rows[0]["tasks"]), 30)
        self.assertEqual([row["tasks"][0] for row in rows[1:]], latest.NOISE_TASKS + list(reversed(latest.NOISE_TASKS)))
        self.plan["full_suite"]["tasks"].pop()
        self.write(self.plan_path, self.plan)
        with self.assertRaisesRegex(original.Refused, "incomplete_or_reordered_full_suite"):
            self.prepare()

    def test_manifest_rehash_cannot_change_an_original_grader_from_recorded_source(self):
        self.corpus["tasks"][0]["held_out"] = "print('weakened grader')\n"
        self.write(self.corpus_path, self.corpus)
        self.extension["base_corpus_sha256"] = self.hash(self.corpus_path)
        for scenario in self.extension["scenarios"]:
            task = next(row for row in self.corpus["tasks"] if row["id"] == scenario["base_task"])
            scenario["original_task_sha256"] = original.canonical_sha256(task)
        self.refresh()
        with self.assertRaisesRegex(original.Refused, "source_corpus_differs"):
            self.prepare()

    def test_changed_working_runtime_refuses_without_touching_ledger(self):
        before = self.ledger_path.read_bytes()
        self.runtime_file.write_text("changed runtime fixture\n")
        with self.assertRaisesRegex(original.Refused, "current_runtime_differs"):
            self.prepare()
        self.assertEqual(self.ledger_path.read_bytes(), before)

    def test_original_history_change_is_rejected_even_after_full_digest_refresh(self):
        self.ledger["requests"][40]["run"] = "rewritten-original-history"
        self.refresh()
        with self.assertRaisesRegex(original.Refused, "changed_original_rows"):
            self.prepare()

    def test_missing_ledger_is_never_created(self):
        self.ledger_path.unlink()
        with self.assertRaises(original.Refused):
            self.prepare()
        self.assertFalse(self.ledger_path.exists())

    def test_active_gateway_owner_refuses_read_only_plan(self):
        (self.ledger_path.parent / "gateway-owner.json").write_text("synthetic owner")
        with self.assertRaisesRegex(original.Refused, "gateway_owner_active"):
            self.prepare()

    def test_budget_proposal_cannot_turn_into_authorization(self):
        self.plan["budget_proposal"]["authorized"] = True
        self.write(self.plan_path, self.plan)
        with self.assertRaisesRegex(original.Refused, "changed_or_authorized_budget_proposal"):
            self.prepare()
        self.assertEqual(self.ledger["binding"], original.BINDING)

    def test_model_and_original_case_limits_are_fixed(self):
        for name, value in [("effort", "max"), ("max_requests_per_run", 40), ("task_wall_seconds", 900)]:
            plan = deepcopy(self.plan)
            plan["settings"][name] = value
            self.write(self.plan_path, plan)
            with self.subTest(name=name), self.assertRaisesRegex(original.Refused, "changed_experiment_settings"):
                self.prepare()

    def test_options_and_long_session_source_bindings_must_match(self):
        self.extension["source_commit"] = "0" * 40
        self.refresh()
        with self.assertRaisesRegex(original.Refused, "different_long_session_settings"):
            self.prepare()

    def test_operator_build_record_cannot_substitute_historical_image_source(self):
        path = self.root / "build-record.json"
        self.write(path, {"source_commit": "0" * 40, "source_tree": "1" * 40, "image_id": "sha256:" + "2" * 64})
        with self.assertRaisesRegex(original.Refused, "different_evaluation_source_build"):
            self.prepare(path)
        self.write(path, {"source_commit": self.plan["source"]["commit"], "source_tree": self.plan["source"]["tree"],
                          "image_id": "sha256:" + "2" * 64})
        report = self.prepare(path)
        self.assertFalse(report["source"]["image_build_verified"])
        self.assertFalse(report["live_execution_ready"])

    def cli(self, output):
        return subprocess.run([sys.executable, "-I", "-B", str(Path(latest.__file__)),
            "--root", str(self.root), "--plan", str(self.plan_path), "--output", str(output)],
            capture_output=True, text=True, timeout=20)

    def test_cli_creates_only_a_report_and_refuses_replacement(self):
        before = self.ledger_path.read_bytes()
        output = self.root / "preparation.json"
        first = self.cli(output)
        self.assertEqual(first.returncode, 0, first.stderr)
        self.assertEqual(json.loads(first.stdout), json.loads(output.read_text()))
        self.assertEqual(self.cli(output).returncode, 2)
        self.assertEqual(self.cli(self.ledger_path).returncode, 2)
        self.assertEqual(self.ledger_path.read_bytes(), before)

    def test_cli_refusal_cannot_echo_unreviewed_sensitive_fields(self):
        self.plan["api_key"] = "synthetic-private-marker"
        self.write(self.plan_path, self.plan)
        output = self.root / "must-not-exist.json"
        result = self.cli(output)
        self.assertEqual(result.returncode, 2)
        self.assertNotIn("synthetic-private-marker", result.stdout + result.stderr)
        self.assertFalse(output.exists())


class FixtureSeeding(unittest.TestCase):
    def setUp(self):
        self.corpus = frozen_corpus_fixture(ROOT)
        self.plan = json.loads(PLAN.read_bytes())
        self.options = latest.git(ROOT, "cat-file", "blob", "HEAD:" + self.plan["feature_options"]["path"])
        self.temporary = tempfile.TemporaryDirectory(prefix="craftsman-corpus-checkout-")
        self.addCleanup(self.temporary.cleanup)
        self.checkout = Path(self.temporary.name)
        files = {
            CORPUS_PATH: self.corpus,
            str(PLAN.relative_to(ROOT)): PLAN.read_bytes(),
            self.plan["feature_options"]["path"]: self.options,
            self.plan["long_session_corpus"]["path"]: (ROOT / self.plan["long_session_corpus"]["path"]).read_bytes(),
        }
        for path, data in files.items():
            target = self.checkout / path
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(data)
        self.command("init", "--quiet")
        self.command("config", "core.autocrlf", "false")
        self.commit()

    def command(self, *arguments):
        return subprocess.run(["git", "-C", str(self.checkout), *arguments],
            check=True, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=20)

    def commit(self):
        self.command("add", "--all")
        self.command("-c", "core.hooksPath=/dev/null", "-c", "commit.gpgsign=false",
            "-c", "user.name=Evaluation fixture", "-c", "user.email=fixture@localhost",
            "commit", "--quiet", "--no-gpg-sign", "-m", "Owned corpus fixture")

    def case(self):
        case = Cases("test_read_only_preparation_has_no_live_grade_lease_or_new_allowance")
        with mock.patch.multiple(sys.modules[__name__], ROOT=self.checkout,
                PLAN=self.checkout / "evals/coding/latest-runtime-v1/plan.json"):
            case.setUp()
        self.addCleanup(case.tearDown)
        return case

    def test_actual_crlf_checkout_seeds_frozen_lf_corpus_and_options(self):
        for path in (CORPUS_PATH, self.plan["feature_options"]["path"]):
            (self.checkout / path).unlink()
        self.command("-c", "core.autocrlf=true", "checkout-index", "--force", "--",
            CORPUS_PATH, self.plan["feature_options"]["path"])
        corpus_path = self.checkout / CORPUS_PATH
        options_path = self.checkout / self.plan["feature_options"]["path"]
        self.assertEqual(corpus_path.read_bytes(), self.corpus.replace(b"\n", b"\r\n"))
        self.assertEqual(options_path.read_bytes(), self.options.replace(b"\n", b"\r\n"))
        before = {path: path.read_bytes() for path in (corpus_path, options_path)}
        case = self.case()
        self.assertEqual(case.corpus_path.read_bytes(), self.corpus)
        self.assertEqual(case.options_path.read_bytes(), self.options)
        self.assertEqual(case.plan["corpus"]["sha256"], latest.ORIGINAL_CORPUS_SHA256)
        report = case.prepare()
        self.assertTrue(report["offline_checks_passed"])
        self.assertFalse(report["live_execution_ready"])
        self.assertEqual(report["paid_model_calls"], 0)
        for path, data in before.items():self.assertEqual(path.read_bytes(), data)

    def test_wrong_current_blob_refuses_with_rehashed_plan_and_original_working_bytes(self):
        changed = json.loads(self.corpus)
        changed["tasks"][0]["held_out"] = "print('changed synthetic grader')\n"
        for data in (self.corpus.replace(b"\n", b"\r\n"), json.dumps(changed).encode()):
            with self.subTest(sha256=hashlib.sha256(data).hexdigest()):
                (self.checkout / CORPUS_PATH).write_bytes(data)
                self.plan["corpus"]["sha256"] = hashlib.sha256(data).hexdigest()
                (self.checkout / "evals/coding/latest-runtime-v1/plan.json").write_bytes(json.dumps(self.plan).encode())
                self.commit()
                (self.checkout / CORPUS_PATH).write_bytes(self.corpus)
                with self.assertRaisesRegex(original.Refused, "source_corpus_differs"):
                    frozen_corpus_fixture(self.checkout)
                self.assertEqual((self.checkout / CORPUS_PATH).read_bytes(), self.corpus)

    def test_rehashed_fixture_crlf_or_extra_byte_still_refuses(self):
        for data in (self.corpus.replace(b"\n", b"\r\n"), self.corpus + b"\n"):
            with self.subTest(sha256=hashlib.sha256(data).hexdigest()):
                case = self.case()
                before = case.ledger_path.read_bytes()
                case.corpus_path.write_bytes(data)
                case.refresh()
                self.assertEqual(case.plan["corpus"]["sha256"], hashlib.sha256(data).hexdigest())
                with self.assertRaisesRegex(original.Refused, "source_corpus_differs"):
                    case.prepare()
                self.assertEqual(case.corpus_path.read_bytes(), data)
                self.assertEqual(case.ledger_path.read_bytes(), before)

    def test_missing_current_blob_has_no_working_copy_fallback(self):
        self.command("rm", "--quiet", "--", CORPUS_PATH)
        self.commit()
        (self.checkout / CORPUS_PATH).write_bytes(self.corpus)
        with self.assertRaisesRegex(original.Refused, "source_record_unavailable"):
            frozen_corpus_fixture(self.checkout)
        self.assertEqual((self.checkout / CORPUS_PATH).read_bytes(), self.corpus)

    def test_options_current_blob_must_match_declared_fixture_digest(self):
        path = self.checkout / self.plan["feature_options"]["path"]
        changed = self.options + b"\n"
        path.write_bytes(changed)
        self.commit()
        case = Cases("test_read_only_preparation_has_no_live_grade_lease_or_new_allowance")
        try:
            with mock.patch.multiple(sys.modules[__name__], ROOT=self.checkout,
                    PLAN=self.checkout / "evals/coding/latest-runtime-v1/plan.json"), \
                    self.assertRaisesRegex(original.Refused, "feature_options_digest_changed"):
                case.setUp()
            self.assertFalse((case.root / "synthetic-budget/ledger.json").exists())
        finally:case.tearDown()
        self.assertEqual(path.read_bytes(), changed)


if __name__ == "__main__":
    unittest.main()
