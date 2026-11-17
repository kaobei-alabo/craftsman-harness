"""Prepare a fresh evaluation offline, preserving the original admission ledger.

This command reads reviewed evaluation records and Git objects. It never loads
operator configuration, resolves a credential, creates a ledger, starts Docker,
or grants live authorization. The proposed extra allowance is arithmetic only.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
from coding_eval_anchors import ORIGINAL_CORPUS_SHA256
from preflight_continuation import (
    BINDING, CAP_MICROS, MAX_INPUT, MAX_OUTPUT, SETTINGS, Refused,
    canonical_sha256, digest, integer, keys, read_json, require,
    valid_relative_path, validate_build, validate_checkpoint, validate_corpus,
    validate_feature_options, validate_ledger,
)


LIMITS = {
    "request_body_bytes": 262144,
    "input_reservation_tokens": MAX_INPUT,
    "response_bytes": 8388608,
    "event_bytes": 1048576,
    "captured_output_bytes": 8388608,
    "agent_memory_bytes": 805306368,
    "agent_pids": 128,
    "agent_cpus": 2,
    "grader_wall_seconds": 90,
}
NOISE_TASKS = ["py-page-boundaries", "js-json-line-chunks", "rs-report-skips"]
LONG_SCENARIOS = ["compaction-v1", "interruption-reopen-v1", "retained-results-v1"]
PROPOSAL_MICROS = 25000000
ALLOCATIONS = {"full_suite": 18500000, "selected_repeats": 3000000, "long_sessions": 3000000}
IMMUTABLE_RUNTIME_EXCLUSIONS = ("docs/", "evals/coding/", "scripts/coding-eval/")
ADDITIVE_VALIDATION_PATHS = {"scripts/verify.sh", ".github/workflows/ci.yml"}


def validate_additive_wiring(original, actual, path):
    commands = {"scripts/verify.sh": {
        'run_check coding-eval-latest "$verification_python" -I -B scripts/coding-eval/test_preflight_latest.py\n',
        'run_check coding-eval-long-session "$verification_python" -I -B scripts/coding-eval/test_long_session.py\n',
        'run_check coding-eval-owned-tui "$verification_python" -I -B scripts/coding-eval/test_owned_tui.py\n'},
        ".github/workflows/ci.yml": {
        '          python -I -B scripts/coding-eval/test_preflight_latest.py\n',
        '          python -I -B scripts/coding-eval/test_long_session.py\n',
        '          python -I -B scripts/coding-eval/test_owned_tui.py\n'}}
    before = original.decode().splitlines(keepends=True)
    after = actual.decode().splitlines(keepends=True)
    position = 0
    additions = []
    for line in after:
        if position < len(before) and line == before[position]:
            position += 1
        else:
            require(line in commands[path], "validation_wiring_not_additive")
            additions.append(line)
    require(position == len(before), "original_validation_removed")
    if additions:
        expected = 1 if path == "scripts/verify.sh" else 2
        require(all(additions.count(command) == expected for command in commands[path]), "incomplete_additive_validation_wiring")


def git(root, *arguments):
    try:
        result = subprocess.run(
            ["git", "-C", str(root), *arguments], stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, timeout=20,
        )
        require(result.returncode == 0 and len(result.stdout) <= 16 * 1024 * 1024,
                "source_record_unavailable")
        return result.stdout
    except (OSError, subprocess.TimeoutExpired):
        raise Refused("source_record_unavailable") from None


def runtime_inventory(root, revision):
    """Bind every tracked path outside the explicitly allowed preparation scope."""
    records = git(root, "ls-tree", "-r", "-z", "--full-tree", revision).split(b"\0")
    inventory = []
    for record in records:
        if not record:
            continue
        header, raw_path = record.split(b"\t", 1)
        mode, kind, blob = header.decode("ascii").split(" ")
        path = raw_path.decode("utf-8")
        if path.startswith(IMMUTABLE_RUNTIME_EXCLUSIONS):
            continue
        inventory.append({"path": path, "mode": mode, "kind": kind, "object": blob})
    return inventory


def check_source(root, source):
    keys(source, ("commit", "tree", "runtime_inventory_sha256", "image_id"))
    digest(source["commit"], 40)
    digest(source["tree"], 40)
    digest(source["runtime_inventory_sha256"])
    # A later image must have a separate real build record, never a placeholder.
    require(source["image_id"] is None, "preparation_image_must_be_absent")
    tree = git(root, "rev-parse", source["commit"] + "^{tree}").decode().strip()
    require(tree == source["tree"], "source_tree_changed")
    inventory = runtime_inventory(root, source["commit"])
    untracked = git(root, "ls-files", "--others", "--exclude-standard", "-z").split(b"\0")
    require(not any(path and (path.decode().endswith("/build.rs") or path == b"build.rs"
                or path.decode().startswith(".cargo/")) for path in untracked), "untracked_automatic_build_input_refused")
    require(canonical_sha256(inventory) == source["runtime_inventory_sha256"],
            "runtime_inventory_changed")
    current = runtime_inventory(root, "HEAD")
    require([item for item in current if item["path"] not in ADDITIVE_VALIDATION_PATHS]
            == [item for item in inventory if item["path"] not in ADDITIVE_VALIDATION_PATHS], "current_runtime_differs")
    # Git objects alone cannot establish that the working source remains exact.
    for item in inventory:
        if item["kind"] != "blob" or item["mode"] not in {"100644", "100755"}:
            continue
        path = root / item["path"]
        try:
            require(not path.is_symlink() and path.is_file(), "current_runtime_differs")
            data = path.read_bytes()
            if item["path"] in ADDITIVE_VALIDATION_PATHS:
                validate_additive_wiring(git(root, "cat-file", "blob", item["object"]), data, item["path"])
                continue
            header = b"blob " + str(len(data)).encode("ascii") + b"\0"
            require(hashlib.sha1(header + data).hexdigest() == item["object"], "current_runtime_differs")
        except OSError:
            raise Refused("current_runtime_differs") from None
    return {"commit": source["commit"], "tree": tree,
            "runtime_inventory_sha256": source["runtime_inventory_sha256"],
            "current_runtime_matches": True, "image_id": None,
            "image_build_verified": False}


def validate_plan(plan, corpus):
    keys(plan, ("version", "kind", "is_live_authorization", "source", "settings", "limits",
                "corpus", "feature_options", "long_session_corpus", "ledger", "checkpoint",
                "full_suite", "noise_check", "budget_proposal", "historical_scores"))
    require(type(plan["version"]) is int and plan["version"] == 1, "unsupported_plan_version")
    require(plan["kind"] == "latest_runtime_offline_plan" and plan["is_live_authorization"] is False,
            "not_an_offline_plan")
    require(plan["settings"] == SETTINGS and all(
        type(plan["settings"][key]) is type(value) for key, value in SETTINGS.items()),
        "changed_experiment_settings")
    require(plan["limits"] == LIMITS and all(type(value) is int for value in plan["limits"].values()),
            "changed_runner_limits")
    for name in ("corpus", "feature_options", "long_session_corpus"):
        keys(plan[name], ("path", "sha256"))
        valid_relative_path(plan[name]["path"])
        digest(plan[name]["sha256"])
    keys(plan["full_suite"], ("label", "tasks", "report_as_new_run", "edit_historical_grades"))
    require(plan["full_suite"]["label"] == "latest-full-v1"
            and plan["full_suite"]["report_as_new_run"] is True
            and plan["full_suite"]["edit_historical_grades"] is False, "changed_full_cohort")
    task_ids = [task["id"] for task in corpus["tasks"]]
    require(plan["full_suite"]["tasks"] == task_ids, "incomplete_or_reordered_full_suite")
    validate_corpus(corpus, {"corpus_version": 5, "cohort": "full_suite_forecast", "tasks": task_ids})
    keys(plan["noise_check"], ("kind", "selected_tasks", "batches", "use_full_suite_as_first_observation"))
    expected_batches = [
        {"label": "latest-aa1-v1", "tasks": NOISE_TASKS},
        {"label": "latest-aa2-v1", "tasks": list(reversed(NOISE_TASKS))},
    ]
    require(plan["noise_check"] == {
        "kind": "repeated_selected_cases", "selected_tasks": NOISE_TASKS,
        "batches": expected_batches, "use_full_suite_as_first_observation": True,
    }, "changed_noise_schedule")
    require(plan["historical_scores"] == {"successful_turns": "17/30", "behavior_checks": "21/30",
                                          "modified_by_this_plan": False}, "changed_historical_scores")
    proposal = plan["budget_proposal"]
    keys(proposal, ("additional_micros", "proposed_total_micros", "authorized", "allocations_micros"))
    require(proposal == {"additional_micros": PROPOSAL_MICROS,
                         "proposed_total_micros": CAP_MICROS + PROPOSAL_MICROS,
                         "authorized": False, "allocations_micros": ALLOCATIONS},
            "changed_or_authorized_budget_proposal")
    keys(plan["ledger"], ("path", "sha256", "request_count", "accounted_micros", "unknown_requests"))
    require(isinstance(plan["ledger"]["path"], str) and Path(plan["ledger"]["path"]).is_absolute()
            and Path(plan["ledger"]["path"]).name == "ledger.json", "invalid_original_ledger_path")
    digest(plan["ledger"]["sha256"])
    for name in ("request_count", "accounted_micros", "unknown_requests"):
        integer(plan["ledger"][name])
    keys(plan["checkpoint"], ("path", "sha256"))
    require(isinstance(plan["checkpoint"]["path"], str) and Path(plan["checkpoint"]["path"]).is_absolute(),
            "invalid_checkpoint_path")
    digest(plan["checkpoint"]["sha256"])


def planning_costs(facts):
    maximum_request = MAX_INPUT * 2 + MAX_OUTPUT * 10
    requests = {"full_suite": 30 * 20, "selected_repeats": 6 * 20, "long_sessions": 3 * 40}
    projected_remaining = facts["remaining_micros"] + PROPOSAL_MICROS
    return {
        "confirmed_prior_rates": BINDING["prices"],
        "currently_authorized_cap_micros": CAP_MICROS,
        "additional_allowance_proposed_micros": PROPOSAL_MICROS,
        "proposed_total_micros": CAP_MICROS + PROPOSAL_MICROS,
        "proposal_authorized": False,
        "projected_capacity_after_explicit_approval_micros": projected_remaining,
        "planning_allocations_micros": dict(ALLOCATIONS),
        "planning_total_micros": sum(ALLOCATIONS.values()),
        "planning_buffer_micros": projected_remaining - sum(ALLOCATIONS.values()),
        "present_allowance_can_fund_plan": facts["remaining_micros"] >= sum(ALLOCATIONS.values()),
        "maximum_one_request_reservation_micros": maximum_request,
        "maximum_20_request_case_reservation_micros": maximum_request * 20,
        "maximum_requests_by_cohort": requests,
        "all_requests_fully_reserved_micros": {key: value * maximum_request for key, value in requests.items()},
        "all_cohorts_fully_reserved_micros": sum(requests.values()) * maximum_request,
        "is_invoice_or_completion_guarantee": False,
        "new_unknowns_retain_their_entire_reservations": True,
    }


def admission_observations(ledger):
    """Report persisted admissions, never infer absent attempts or refusals."""
    groups = {}
    for row in ledger["requests"]:
        group = groups.setdefault(row["run"], {"run": row["run"], "attempted": None, "admitted": 0,
            "refused": None, "unknown_usage": 0, "request_ordinals": []})
        group["admitted"] += 1
        group["unknown_usage"] += int(row["state"] != "settled")
        group["request_ordinals"].append(row["ordinal"])
    return {"source": "validated_original_ledger",
            "admitted_means": "persisted_budget_reservation_not_a_provider_send_receipt",
            "attempts_recorded": False, "refusals_recorded": False,
            "twenty_admissions_prove_a_twenty_first_attempt": False,
            "per_run": list(groups.values()),
            "future_private_attempt_and_refusal_observer": "proposed_not_implemented"}


def prepare(root, plan_path, build_path=None):
    root = Path(root).resolve()
    plan, plan_hash = read_json(plan_path, 256 * 1024)
    keys(plan, ("version", "kind", "is_live_authorization", "source", "settings", "limits",
                "corpus", "feature_options", "long_session_corpus", "ledger", "checkpoint",
                "full_suite", "noise_check", "budget_proposal", "historical_scores"))
    inputs = {}
    for name, maximum in (("corpus", 8 * 1024 * 1024), ("feature_options", 32768),
                          ("long_session_corpus", 256 * 1024)):
        keys(plan[name], ("path", "sha256"))
        valid_relative_path(plan[name]["path"])
        value, actual_hash = read_json(root / plan[name]["path"], maximum)
        require(actual_hash == plan[name]["sha256"], name + "_digest_changed")
        inputs[name] = value
    corpus = inputs["corpus"]
    require(plan["corpus"]["sha256"] == ORIGINAL_CORPUS_SHA256, "source_corpus_differs")
    validate_plan(plan, corpus)
    options = validate_feature_options(inputs["feature_options"])
    require(options == {"context": {"enabled": True, "max_bytes": 12288},
                        "failure_control": {"repeat_limit": 5, "test_review_signals": False}},
            "changed_selected_features")
    from grade_long_session import validate_extension
    extension = validate_extension(inputs["long_session_corpus"], corpus, plan["corpus"]["sha256"])
    require(extension["source_commit"] == plan["source"]["commit"]
            and extension["source_tree"] == plan["source"]["tree"]
            and extension["feature_options_sha256"] == plan["feature_options"]["sha256"],
            "different_long_session_settings")
    source = check_source(root, plan["source"])
    original_corpus = git(root, "show", plan["source"]["commit"] + ":" + plan["corpus"]["path"])
    require(hashlib.sha256(original_corpus).hexdigest() == plan["corpus"]["sha256"],
            "source_corpus_differs")
    build_hash = None
    if build_path is not None:
        build, build_hash = read_json(build_path, 65536)
        checked = validate_build(build)
        require(checked["source_commit"] == source["commit"] and checked["source_tree"] == source["tree"],
                "different_evaluation_source_build")
        source.update(image_id=checked["image_id"], image_build_verified=False)
        source["image_record_check"] = "matches_operator_build_record_only"
    ledger_path = Path(plan["ledger"]["path"])
    require(not os.path.lexists(ledger_path.parent / "gateway-owner.json"), "gateway_owner_active")
    ledger, ledger_hash = read_json(ledger_path, 4 * 1024 * 1024)
    require(ledger_hash == plan["ledger"]["sha256"], "ledger_digest_changed")
    facts = validate_ledger(ledger)
    for plan_key, fact_key in (("request_count", "requests"), ("accounted_micros", "accounted_micros"),
                               ("unknown_requests", "unknown_requests")):
        require(plan["ledger"][plan_key] == facts[fact_key], "changed_current_ledger_facts")
    checkpoint, checkpoint_hash = read_json(Path(plan["checkpoint"]["path"]), 65536)
    require(checkpoint_hash == plan["checkpoint"]["sha256"], "checkpoint_digest_changed")
    validate_checkpoint(checkpoint, ledger_path, facts, ledger_hash)
    invocations = [{"label": plan["full_suite"]["label"], "tasks": plan["full_suite"]["tasks"],
                    "runner": "scripts/coding-eval/run_suite.py", "live_execution_ready": False}]
    for batch in plan["noise_check"]["batches"]:
        # The existing runner retains corpus order for multi-task selections.
        # Separate single-task invocations preserve the declared repeat order.
        invocations.extend({"label": batch["label"], "tasks": [task],
                            "runner": "scripts/coding-eval/run_suite.py", "live_execution_ready": False}
                           for task in batch["tasks"])
    return {
        "version": 1, "kind": "latest_runtime_offline_preflight", "offline_checks_passed": True,
        "is_live_authorization": False, "live_execution_ready": False, "paid_model_calls": 0,
        "new_live_grades": None, "configuration_or_credentials_loaded": False,
        "ledger_modified": False, "execution_lease_established": False,
        "working_tree_build_attested": False, "required_build_authority": "tracked_git_archive_only",
        "plan_sha256": plan_hash, "corpus": plan["corpus"],
        "feature_options": {**plan["feature_options"], "value": options},
        "long_session_corpus": plan["long_session_corpus"], "source": source,
        "operator_build_record_sha256": build_hash,
        "settings": dict(SETTINGS), "limits": dict(LIMITS),
        "effective_model_settings_sha256": canonical_sha256({
            "generation": {"max_output_tokens": 8192}, "reasoning": {"type": "effort", "value": "xhigh"}}),
        "ledger_path": str(ledger_path), "ledger_sha256": ledger_hash,
        "checkpoint_sha256": checkpoint_hash, "accounting": facts,
        "existing_private_observations": admission_observations(ledger),
        "pricing_plan": planning_costs(facts), "planned_runner_invocations": invocations,
        "long_session_scenarios": [{"id": row["id"], "base_task": row["base_task"],
                                    "limits": extension["limits"]} for row in extension["scenarios"]],
        "corpus_contracts": [{"id": row["id"], "held_out_sha256": hashlib.sha256(row["held_out"].encode()).hexdigest(),
                              "reference_sha256": canonical_sha256(row["reference"]),
                              "original_files_sha256": canonical_sha256(row["files"])} for row in corpus["tasks"]],
        "remaining_gates": ["additional_cap_not_authorized", "existing_admission_cap_is_25_usd",
                            "full_suite_forecasts_refuse_live_execution",
                            "latest_source_image_and_native_gates_pending",
                            "isolated_owned_terminal_driver_and_capture_proof_pending"],
    }


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[2])
    parser.add_argument("--plan", type=Path, required=True)
    parser.add_argument("--build-record", type=Path)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args(argv)
    try:
        report = prepare(args.root, args.plan, args.build_record)
        encoded = json.dumps(report, indent=2, allow_nan=False) + "\n"
        if args.output:
            with args.output.open("x") as output:
                output.write(encoded)
        print(encoded, end="")
        return 0
    except Refused as error:
        print("Latest evaluation preflight refused: " + str(error), file=sys.stderr)
        return 2
    except (OSError, KeyError, TypeError, ValueError):
        print("Latest evaluation preflight refused: invalid_or_unavailable_record", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
