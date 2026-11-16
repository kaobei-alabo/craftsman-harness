"""Read-only continuation checks; this module never starts a model or reserves money.

An operator-supplied first-cycle checkpoint binds the existing ledger's original
855 rows. Current build records bind each requested source/tree/image. These
checks do not establish an execution lease, live authorization, model quality,
or an OS filesystem boundary.
"""
import argparse
from copy import deepcopy
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import sys


SETTINGS = {
    "model": "gpt-6.1-sol",
    "protocol": "openai_responses",
    "effort": "xhigh",
    "max_output_tokens": 8192,
    "max_requests_per_run": 20,
    "task_wall_seconds": 300,
}
BINDING = {
    "limit": "25.00",
    "prices": {"input_per_million": "2", "output_per_million": "10", "request_fee": "0"},
}
ORIGINAL_REQUESTS = 855
ORIGINAL_UNKNOWN_REQUESTS = 40
ORIGINAL_ACCOUNTED_MICROS = 23153274
ORIGINAL_UNKNOWN_MICROS = 6586682
CAP_MICROS = 25000000
MAX_INPUT = 270000
MAX_OUTPUT = 8192
MAX_REQUESTS = 10000
KINDS = {"bug", "feature", "refactor", "failing-tests", "multi-file"}
LANGUAGES = {"python", "javascript", "rust"}
FORBIDDEN_INPUT_NAMES = {"config.json", "config.toml", "settings.json", "settings.local.json",
                         "auth.json", "credentials.json", "tokens.json", ".env"}


class Refused(ValueError):
    """A fixed diagnostic code; input values and credentials are never echoed."""


def require(condition, code):
    if not condition:
        raise Refused(code)


def keys(value, required, optional=()):
    require(isinstance(value, dict), "invalid_object")
    require(set(required) <= set(value) <= set(required) | set(optional), "invalid_fields")


def integer(value, minimum=0, maximum=2**63 - 1):
    require(type(value) is int and minimum <= value <= maximum, "invalid_integer")
    return value


def digest(value, length=64):
    require(isinstance(value, str) and re.fullmatch(r"[0-9a-f]{" + str(length) + r"}", value), "invalid_digest")
    return value


def text_size(value):
    require(isinstance(value, str), "invalid_text")
    try:
        return len(value.encode("utf-8"))
    except UnicodeError:
        raise Refused("invalid_text_encoding") from None


def canonical(value):
    try:
        return json.dumps(value, sort_keys=True, ensure_ascii=False, separators=(",", ":"), allow_nan=False).encode()
    except (TypeError, ValueError, UnicodeError, RecursionError):
        raise Refused("invalid_canonical_json") from None


def canonical_sha256(value):
    return hashlib.sha256(canonical(value)).hexdigest()


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, "duplicate_json_key")
        result[key] = value
    return result


def no_constant(_value):
    raise Refused("nonfinite_json")


def read_json(path, maximum):
    """Bounded regular-file reads with leaf identity checks, never config loading."""
    path = Path(path)
    require(path.name.casefold() not in FORBIDDEN_INPUT_NAMES and not path.name.casefold().startswith(".env."),
            "configuration_input_refused")
    try:
        before = path.lstat()
        require(stat.S_ISREG(before.st_mode) and before.st_nlink == 1, "nonregular_input")
        require(before.st_size <= maximum, "input_too_large")
        fd = os.open(path, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_NONBLOCK", 0))
        try:
            actual = os.fstat(fd)
            require(stat.S_ISREG(actual.st_mode) and actual.st_nlink == 1, "nonregular_input")
            require((before.st_dev, before.st_ino) == (actual.st_dev, actual.st_ino), "input_changed")
            data = bytearray()
            while True:
                chunk = os.read(fd, min(65536, maximum + 1 - len(data)))
                if not chunk:
                    break
                data.extend(chunk)
                require(len(data) <= maximum, "input_too_large")
            after = os.fstat(fd)
        finally:
            os.close(fd)
        final = path.lstat()
        identity = lambda info: (info.st_dev, info.st_ino, info.st_size, info.st_mtime_ns,
                                 info.st_ctime_ns, info.st_nlink)
        require(identity(before) == identity(after) == identity(final), "input_changed")
        value = json.loads(bytes(data).decode("utf-8"), object_pairs_hook=unique_object, parse_constant=no_constant)
        return value, hashlib.sha256(data).hexdigest()
    except Refused:
        raise
    except (OSError, UnicodeError, ValueError, RecursionError):
        raise Refused("input_unavailable_or_invalid") from None


def validate_feature_options(value):
    """Return only reviewed explicit fields; preserve absence and caller input."""
    keys(value, (), ("context", "failure_control"))
    result = {}
    if "context" in value:
        context = value["context"]
        keys(context, ("enabled",), ("max_bytes",))
        require(type(context["enabled"]) is bool, "invalid_context_enabled")
        result["context"] = {"enabled": context["enabled"]}
        if "max_bytes" in context:
            result["context"]["max_bytes"] = integer(context["max_bytes"], 2048, 32768)
    if "failure_control" in value:
        failure = value["failure_control"]
        keys(failure, ("repeat_limit",), ("test_review_signals",))
        result["failure_control"] = {"repeat_limit": integer(failure["repeat_limit"], 3, 20)}
        if "test_review_signals" in failure:
            require(type(failure["test_review_signals"]) is bool, "invalid_test_review_signals")
            result["failure_control"]["test_review_signals"] = failure["test_review_signals"]
    return result


def validate_build(value):
    keys(value, ("source_commit", "source_tree", "image_id"))
    digest(value["source_commit"], 40)
    digest(value["source_tree"], 40)
    require(isinstance(value["image_id"], str) and re.fullmatch(r"sha256:[0-9a-f]{64}", value["image_id"]), "invalid_image_id")
    return dict(value)


def validate_manifest(value):
    keys(value, ("version", "cohort", "tasks", "corpus_version", "corpus_sha256", "ledger_sha256", "settings", "arms"))
    require(type(value["version"]) is int and value["version"] == 1, "unsupported_manifest_version")
    require(isinstance(value["cohort"], str) and value["cohort"] in {"selected_pair", "full_suite_forecast"}, "invalid_cohort")
    integer(value["corpus_version"], 1, 1000)
    digest(value["corpus_sha256"])
    digest(value["ledger_sha256"])
    tasks = value["tasks"]
    require(isinstance(tasks, list) and 1 <= len(tasks) <= 30, "invalid_task_selection")
    require(all(isinstance(task, str) and re.fullmatch(r"[a-z0-9-]{1,64}", task) for task in tasks), "invalid_task_id")
    require(len(set(tasks)) == len(tasks), "duplicate_task_id")
    keys(value["settings"], SETTINGS)
    for name, expected in SETTINGS.items():
        actual = value["settings"][name]
        require(type(actual) is type(expected) and actual == expected, "changed_experiment_settings")
    keys(value["arms"], ("control", "candidate"))
    arms = {}
    for name in ("control", "candidate"):
        arm = value["arms"][name]
        keys(arm, ("source_commit", "source_tree", "image_id", "options"))
        arms[name] = {**validate_build({key: arm[key] for key in ("source_commit", "source_tree", "image_id")}),
                      "options": validate_feature_options(arm["options"])}
    return {**value, "tasks": list(tasks), "settings": dict(SETTINGS), "arms": arms}


def valid_relative_path(value):
    require(1 <= text_size(value) <= 512, "invalid_task_path")
    require(not any(character in value for character in ("\\", ":", "\x00")), "invalid_task_path")
    parts = value.split("/")
    require(all(part not in {"", ".", "..", ".git"} for part in parts), "invalid_task_path")
    require(not any(ord(character) < 32 or ord(character) == 127 for character in value), "invalid_task_path")


def validate_corpus(value, manifest):
    keys(value, ("version", "tasks"))
    require(type(value["version"]) is int and value["version"] == manifest["corpus_version"], "corpus_version_changed")
    require(isinstance(value["tasks"], list) and len(value["tasks"]) == 30, "invalid_corpus_size")
    seen = set()
    balance = {}
    for task in value["tasks"]:
        keys(task, ("id", "category", "language", "prompt", "files", "reference", "public_command", "held_out"))
        require(isinstance(task["id"], str) and re.fullmatch(r"[a-z0-9-]{1,64}", task["id"]), "invalid_task_id")
        require(task["id"] not in seen, "duplicate_corpus_task")
        seen.add(task["id"])
        require(isinstance(task["category"], str) and isinstance(task["language"], str)
                and task["category"] in KINDS and task["language"] in LANGUAGES, "invalid_task_category")
        group = (task["category"], task["language"])
        balance[group] = balance.get(group, 0) + 1
        for key in ("prompt", "held_out"):
            require(0 < text_size(task[key]) <= 256 * 1024, "invalid_task_text")
        for key in ("files", "reference"):
            require(isinstance(task[key], dict) and 1 <= len(task[key]) <= 64, "invalid_task_files")
            for path, text in task[key].items():
                valid_relative_path(path)
                require(text_size(text) <= 1024 * 1024, "invalid_task_text")
        command = task["public_command"]
        require(isinstance(command, list) and 1 <= len(command) <= 32, "invalid_public_command")
        require(all(0 < text_size(part) <= 4096 and "\x00" not in part for part in command), "invalid_public_command")
    require(balance == {(kind, language): 2 for kind in KINDS for language in LANGUAGES}, "unbalanced_corpus")
    require(set(manifest["tasks"]) <= seen, "unknown_selected_task")
    if manifest["cohort"] == "full_suite_forecast":
        require(set(manifest["tasks"]) == seen, "incomplete_full_suite")


def validate_ledger(value):
    keys(value, ("version", "binding", "closed", "requests"))
    require(type(value["version"]) is int and value["version"] == 1, "unsupported_ledger_version")
    require(value["binding"] == BINDING, "changed_ledger_binding")
    require(value["closed"] is False, "closed_ledger")
    rows = value["requests"]
    require(isinstance(rows, list) and ORIGINAL_REQUESTS <= len(rows) <= MAX_REQUESTS, "missing_original_history")
    for index, row in enumerate(rows):
        keys(row, ("ordinal", "run", "state", "input_upper", "output_max", "reserved_micros", "charged_micros", "input_tokens", "output_tokens", "http_status"),
             ("response_observation",))
        require(integer(row["ordinal"]) == index, "invalid_ledger_ordinal")
        require(isinstance(row["run"], str) and re.fullmatch(r"[a-z0-9-]{1,100}", row["run"]), "invalid_run_id")
        require(isinstance(row["state"], str) and row["state"] in {"reserved", "settled", "unsettled"}, "invalid_ledger_state")
        upper = integer(row["input_upper"], 0, MAX_INPUT)
        reply = integer(row["output_max"], 1, MAX_OUTPUT)
        reserved = integer(row["reserved_micros"])
        charged = integer(row["charged_micros"])
        require(reserved == upper * 2 + reply * 10 and charged <= reserved, "invalid_ledger_charge")
        if row["http_status"] is not None:
            integer(row["http_status"], 100, 599)
        if row["state"] == "settled":
            actual_input = integer(row["input_tokens"], 0, upper)
            actual_output = integer(row["output_tokens"], 0, reply)
            require(charged == actual_input * 2 + actual_output * 10, "invalid_settled_charge")
        else:
            require(row["input_tokens"] is None and row["output_tokens"] is None, "invalid_unknown_usage")
            require(charged == reserved, "refunded_unknown_usage")
    original = rows[:ORIGINAL_REQUESTS]
    unknown = [row for row in original if row["state"] != "settled"]
    require(sum(row["charged_micros"] for row in original) == ORIGINAL_ACCOUNTED_MICROS, "changed_original_accounting")
    require(len(unknown) == ORIGINAL_UNKNOWN_REQUESTS, "changed_original_unknown_count")
    require(sum(row["charged_micros"] for row in unknown) == ORIGINAL_UNKNOWN_MICROS, "changed_original_unknown_charge")
    accounted = sum(row["charged_micros"] for row in rows)
    require(accounted <= CAP_MICROS, "exceeded_approved_cap")
    return {"cap_micros": CAP_MICROS, "accounted_micros": accounted,
            "remaining_micros": CAP_MICROS - accounted, "requests": len(rows),
            "unknown_requests": sum(row["state"] != "settled" for row in rows),
            "original_rows_sha256": canonical_sha256(original)}


def make_original_checkpoint(ledger_path):
    """Capture an existing initial ledger for operator review; never save or create it."""
    ledger_path = Path(ledger_path)
    require(ledger_path.is_absolute() and ledger_path.name == "ledger.json", "invalid_original_ledger_path")
    ledger, raw_digest = read_json(ledger_path, 4 * 1024 * 1024)
    facts = validate_ledger(ledger)
    require(facts["requests"] == ORIGINAL_REQUESTS, "checkpoint_requires_original_snapshot")
    return {"version": 1, "ledger_path": str(ledger_path), "binding": deepcopy(BINDING),
            "original_request_count": ORIGINAL_REQUESTS,
            "original_accounted_micros": ORIGINAL_ACCOUNTED_MICROS,
            "original_unknown_requests": ORIGINAL_UNKNOWN_REQUESTS,
            "original_unknown_micros": ORIGINAL_UNKNOWN_MICROS,
            "original_rows_sha256": facts["original_rows_sha256"],
            "initial_ledger_sha256": raw_digest}


def validate_checkpoint(value, ledger_path, facts, ledger_hash):
    keys(value, ("version", "ledger_path", "binding", "original_request_count", "original_accounted_micros",
                 "original_unknown_requests", "original_unknown_micros", "original_rows_sha256", "initial_ledger_sha256"))
    require(type(value["version"]) is int and value["version"] == 1, "unsupported_checkpoint_version")
    require(value["binding"] == BINDING, "changed_checkpoint_binding")
    for key, expected in (("original_request_count", ORIGINAL_REQUESTS), ("original_accounted_micros", ORIGINAL_ACCOUNTED_MICROS),
                          ("original_unknown_requests", ORIGINAL_UNKNOWN_REQUESTS), ("original_unknown_micros", ORIGINAL_UNKNOWN_MICROS)):
        require(type(value[key]) is int and value[key] == expected, "changed_checkpoint_accounting")
    path = value["ledger_path"]
    require(isinstance(path, str) and Path(path).is_absolute(), "invalid_original_ledger_path")
    require(os.path.normcase(os.path.abspath(path)) == os.path.normcase(os.path.abspath(ledger_path)), "different_ledger_path")
    digest(value["initial_ledger_sha256"])
    require(digest(value["original_rows_sha256"]) == facts["original_rows_sha256"], "changed_original_rows")
    if facts["requests"] == ORIGINAL_REQUESTS:
        require(value["initial_ledger_sha256"] == ledger_hash, "changed_initial_ledger_digest")


def forecast(manifest, remaining):
    selected = set(manifest["tasks"])
    if manifest["cohort"] == "full_suite_forecast":
        lower, upper = 10000000, 18500000
    elif selected <= {"rs-report-skips", "py-create-only-export"}:
        lower = (277314 if "rs-report-skips" in selected else 0) + (496950 if "py-create-only-export" in selected else 0)
        upper = (395120 if "rs-report-skips" in selected else 0) + (622184 if "py-create-only-export" in selected else 0)
    else:
        lower, upper = 166000 * 2 * len(selected), 309000 * 2 * len(selected)
    return {"paired_attempts": 2 * len(selected), "observed_forecast_range_micros": [lower, upper],
            "upper_forecast_exceeds_remaining": upper > remaining,
            "maximum_request_reservation_micros": MAX_INPUT * 2 + MAX_OUTPUT * 10,
            "maximum_20_request_task_reservation_micros": (MAX_INPUT * 2 + MAX_OUTPUT * 10) * 20,
            "is_billing_or_completion_guarantee": False, "is_live_authorization": False}


def preflight(manifest_path, builds_path, checkpoint_path, ledger_path, corpus_path):
    manifest, manifest_hash = read_json(manifest_path, 256 * 1024)
    manifest = validate_manifest(manifest)
    corpus, corpus_hash = read_json(corpus_path, 8 * 1024 * 1024)
    require(corpus_hash == manifest["corpus_sha256"], "corpus_digest_changed")
    validate_corpus(corpus, manifest)
    builds, builds_hash = read_json(builds_path, 256 * 1024)
    keys(builds, ("version", "control", "candidate"))
    require(type(builds["version"]) is int and builds["version"] == 1, "unsupported_build_version")
    for name in ("control", "candidate"):
        actual = validate_build(builds[name])
        require(actual == {key: manifest["arms"][name][key] for key in actual}, "source_image_binding_changed")
    checkpoint, checkpoint_hash = read_json(checkpoint_path, 64 * 1024)
    ledger_path = Path(ledger_path)
    require(ledger_path.is_absolute() and ledger_path.name == "ledger.json", "invalid_original_ledger_path")
    require(not os.path.lexists(ledger_path.parent / "gateway-owner.json"), "gateway_owner_active")
    ledger, ledger_hash = read_json(ledger_path, 4 * 1024 * 1024)
    require(ledger_hash == manifest["ledger_sha256"], "ledger_digest_changed")
    facts = validate_ledger(ledger)
    validate_checkpoint(checkpoint, ledger_path, facts, ledger_hash)
    return {"version": 1, "kind": "offline_continuation_preflight", "offline_checks_passed": True,
            "is_live_authorization": False, "paid_model_calls": 0, "new_live_grades": None,
            "cohort": manifest["cohort"],
            "manifest_sha256": manifest_hash, "corpus_sha256": corpus_hash,
            "build_records_sha256": builds_hash, "checkpoint_sha256": checkpoint_hash,
            "ledger_sha256": ledger_hash, "ledger_path": str(ledger_path),
            "tasks": manifest["tasks"], "settings": manifest["settings"], "arms": manifest["arms"],
            "accounting": facts, "forecast": forecast(manifest, facts["remaining_micros"]),
            "source_image_check": "matches_operator_build_records",
            "ledger_modified": False, "configuration_or_credentials_loaded": False,
            "execution_lease_established": False}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("manifest", "builds", "checkpoint", "ledger", "corpus"):
        parser.add_argument("--" + name, required=True, type=Path)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args(argv)
    try:
        report = preflight(args.manifest, args.builds, args.checkpoint, args.ledger, args.corpus)
        encoded = json.dumps(report, indent=2, allow_nan=False) + "\n"
        if args.output:
            with args.output.open("x") as output:
                output.write(encoded)
        print(encoded, end="")
        return 0
    except Refused as error:
        print("Continuation preflight refused: " + str(error), file=sys.stderr)
        return 2
    except OSError:
        print("Continuation preflight refused: output_unavailable", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
