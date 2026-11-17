"""Independent outcome checks for three prepared owned-terminal scenarios.

Only operator captures from an isolated, stopped agent are admissible evidence.
The model must never receive this module, the capture bundle, reference fixes,
or held-out graders. Passing synthetic capture tests validates this grader; it
does not establish a runtime outcome or authenticate an operator's captures.
"""
import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent))
from coding_eval_anchors import ORIGINAL_CORPUS_SHA256
from preflight_continuation import (
    Refused, canonical, canonical_sha256, digest, integer, keys, read_json, require,
    text_size, valid_relative_path,
)


SCENARIOS = {
    "compaction-v1": "py-config-merge",
    "interruption-reopen-v1": "py-create-only-export",
    "retained-results-v1": "py-page-boundaries",
}
LONG_LIMITS = {
    "max_model_requests_total": 40,
    "max_output_tokens": 8192,
    "scenario_wall_seconds": 900,
    "max_user_turns": 14,
    "update_turns": 10,
    "update_bytes_each": 8192,
    "checkpoint_capture_bytes": 8388608,
    "idle_reopen_observation_seconds": 5,
}
STAGES = ("before", "after", "reopened", "final")
REQUIRED_ACTIONS = {
    "compaction-v1": ["original_task", "ten_updates", "idle_manual_compact", "quit_and_reopen", "idle_observation", "explicit_followup"],
    "interruption-reopen-v1": ["original_task", "create_effect_once", "ten_updates", "interrupt_pending_model", "quit_and_reopen", "idle_observation", "explicit_followup"],
    "retained-results-v1": ["original_task", "run_marked_probe_once", "operator_change_probe_input", "ten_updates", "quit_and_reopen", "idle_observation", "explicit_followup"],
}


def validate_extension(extension, corpus, corpus_hash):
    keys(extension, ("version", "kind", "base_corpus_sha256", "source_commit", "source_tree",
                     "feature_options_sha256", "effective_model_settings_sha256", "limits", "scenarios"))
    require(type(extension["version"]) is int and extension["version"] == 1, "unsupported_extension_version")
    require(extension["kind"] == "owned_terminal_long_session_preparation"
            and extension["base_corpus_sha256"] == corpus_hash, "changed_extension_base")
    require(corpus_hash == ORIGINAL_CORPUS_SHA256, "original_corpus_anchor_changed")
    digest(extension["source_commit"], 40)
    digest(extension["source_tree"], 40)
    digest(extension["feature_options_sha256"])
    require(extension["effective_model_settings_sha256"] == canonical_sha256({
        "generation": {"max_output_tokens": 8192}, "reasoning": {"type": "effort", "value": "xhigh"}}),
        "changed_long_session_model_settings")
    require(extension["limits"] == LONG_LIMITS and all(type(value) is int for value in extension["limits"].values()),
            "changed_long_session_limits")
    rows = extension["scenarios"]
    require(isinstance(rows, list) and [row.get("id") for row in rows if isinstance(row, dict)] == list(SCENARIOS),
            "changed_long_session_selection")
    known = {task["id"]: task for task in corpus["tasks"]}
    for row in rows:
        keys(row, ("id", "base_task", "original_task_sha256", "setup_prompt", "update_template",
                   "final_prompt", "expected_answer", "effect_file", "expected_effect", "actions",
                   "fixture_files", "probe_command"))
        require(row["base_task"] == SCENARIOS[row["id"]] and row["base_task"] in known, "changed_long_session_task")
        require(row["original_task_sha256"] == canonical_sha256(known[row["base_task"]]),
                "changed_original_long_session_task")
        require(row["actions"] == REQUIRED_ACTIONS[row["id"]], "changed_long_session_actions")
        for name in ("setup_prompt", "update_template", "final_prompt"):
            require(0 < text_size(row[name]) <= 8192, "invalid_scenario_prompt")
        require("{index}" in row["update_template"] and "{padding}" in row["update_template"],
                "invalid_update_template")
        valid_relative_path(row["effect_file"])
        require(text_size(row["expected_effect"]) <= 65536 and isinstance(row["expected_answer"], dict),
                "invalid_scenario_outcome")
        require(isinstance(row["fixture_files"], dict) and len(row["fixture_files"]) <= 8,
                "invalid_scenario_fixtures")
        for path, value in row["fixture_files"].items():
            valid_relative_path(path)
            require(text_size(value) <= 65536, "invalid_scenario_fixture")
        if row["id"] == "retained-results-v1":
            require(row["probe_command"] == "python3 -B probe_once.py --craftsman-soak-v1", "changed_probe_command")
        elif row["id"] == "interruption-reopen-v1":
            require(row["probe_command"] == "python3 -B effect_once.py --craftsman-soak-v1", "changed_effect_command")
        else:
            require(row["probe_command"] is None, "unexpected_probe_command")
    return extension


def updates(scenario):
    result = []
    for index in range(1, LONG_LIMITS["update_turns"] + 1):
        prefix = scenario["update_template"].format(index=index, padding="")
        padding = "z" * (8192 - len(prefix.encode("utf-8")))
        value = scenario["update_template"].format(index=index, padding=padding)
        require(len(value.encode("utf-8")) == 8192, "invalid_update_size")
        result.append(value)
    return result


def checkpoint(value):
    keys(value, ("thread", "seq", "state"))
    require(isinstance(value["thread"], str) and 0 < text_size(value["thread"]) <= 512, "invalid_checkpoint_thread")
    integer(value["seq"], 0, 2**63 - 1)
    state = value["state"]
    require(isinstance(state, dict) and isinstance(state.get("messages"), list)
            and isinstance(state.get("files"), dict) and isinstance(state.get("todos"), list)
            and isinstance(state.get("custom"), dict), "invalid_checkpoint_state")
    require(len(state["messages"]) <= 4096, "checkpoint_messages_exceeded")
    for message in state["messages"]:
        require(isinstance(message, dict), "invalid_checkpoint_message")
        role = message.get("role")
        require(isinstance(role, str) and role in {"system", "user", "assistant", "tool"}
                and isinstance(message.get("content"), str), "invalid_checkpoint_message")
        if role == "tool":
            require(isinstance(message.get("tool_call_id"), str) and type(message.get("is_error")) is bool,
                    "invalid_checkpoint_tool_result")
        calls = message.get("tool_calls", [])
        require(isinstance(calls, list) and len(calls) <= 256, "invalid_checkpoint_calls")
        require(role == "assistant" or not calls, "invalid_checkpoint_calls")
        seen = set()
        for call in calls:
            require(isinstance(call, dict) and isinstance(call.get("id"), str)
                    and isinstance(call.get("name"), str) and isinstance(call.get("args"), dict),
                    "invalid_checkpoint_call")
            require(call["id"] not in seen, "duplicate_checkpoint_call_id")
            seen.add(call["id"])
    return value


def completed_groups(state):
    """Bind results within actual assistant groups, including reused call IDs."""
    groups = []
    messages = state["messages"]
    for index, message in enumerate(messages):
        require(isinstance(message, dict), "invalid_checkpoint_message")
        if message.get("role") != "assistant" or not message.get("tool_calls"):
            continue
        calls = message["tool_calls"]
        require(isinstance(calls, list) and len(calls) <= 256, "invalid_checkpoint_calls")
        group_results = []
        for following in messages[index + 1:]:
            if following.get("role") != "tool":
                break
            group_results.append(following)
        for call in calls:
            require(isinstance(call, dict) and isinstance(call.get("id"), str), "invalid_checkpoint_call")
            matches = [result for result in group_results if result.get("tool_call_id") == call["id"]]
            require(len(matches) <= 1, "duplicate_group_tool_result")
            if matches:
                groups.append(canonical_sha256({"call": call, "result": matches[0]}))
    return Counter(groups)


def command_count(state, command):
    count = 0
    for message in state["messages"]:
        if message.get("role") == "assistant":
            for call in message.get("tool_calls", []):
                if call.get("name") == "execute" and call.get("args", {}).get("command") == command:
                    count += 1
    return count


def results_for_command(state, command):
    found = []
    messages = state["messages"]
    for index, message in enumerate(messages):
        if message.get("role") != "assistant":
            continue
        for ordinal, call in enumerate(message.get("tool_calls", [])):
            if call.get("name") != "execute" or call.get("args", {}).get("command") != command:
                continue
            for following in messages[index + 1:]:
                if following.get("role") != "tool":
                    break
                if following.get("tool_call_id") == call["id"]:
                    found.append(following.get("content"))
    return found


def safe_checkpoint_observation(value, scenario):
    """Derive bounded metadata in memory; never export checkpoint/body text."""
    require(len(canonical(value)) <= LONG_LIMITS["checkpoint_capture_bytes"], "checkpoint_capture_exceeded")
    value = checkpoint(value)
    state = value["state"]
    groups = completed_groups(state)
    owned = owned_input_observation(state)
    inputs = [record["sha256"] for record in owned["retained_inputs"]]
    command = scenario["probe_command"]
    results = [] if command is None else results_for_command(state, command)
    require(all(isinstance(result, str) for result in results), "invalid_checkpoint_result")
    compactions = state["custom"].get("_craftsman_context_v1", {}).get("compactions")
    require(compactions is None or type(compactions) is int and compactions >= 0, "invalid_compaction_observation")
    return {
        "version": 1, "session_sha256": hashlib.sha256(value["thread"].encode()).hexdigest(),
        "seq": value["seq"], "message_count": len(state["messages"]),
        "messages_sha256": canonical_sha256(state["messages"]),
        "files_sha256": canonical_sha256(state["files"]), "todos_sha256": canonical_sha256(state["todos"]),
        "compactions": compactions, "accepted_input_sha256": inputs,
        "completed_groups": [{"sha256": key, "count": groups[key]} for key in sorted(groups)],
        "selected_once_command_count": 0 if command is None else command_count(state, command),
        "selected_once_result_sha256": [hashlib.sha256(result.encode()).hexdigest() for result in results],
        "ordered_message_sha256": [canonical_sha256(message) for message in state["messages"]],
        "message_roles": [message["role"] for message in state["messages"]],
        "user_content_sha256": [{"index": index, "sha256": hashlib.sha256(message["content"].encode()).hexdigest()}
                                for index, message in enumerate(state["messages"]) if message["role"] == "user"],
        "durable_in_progress": durable_observation(state), "owned_input": owned,
        "group_positions": group_positions(state),
        "observed_direct_probe_reads": sum(call.get("name") == "read_file" and call.get("args", {}).get("file_path") == "probe_input.txt"
            for message in state["messages"] if message["role"] == "assistant" for call in message.get("tool_calls", [])),
    }


def durable_observation(state):
    marker = state["custom"].get("_craftsman_durable_turn_v1")
    require(isinstance(marker, dict) and set(marker) == {"version", "in_progress"}
            and type(marker["version"]) is int and marker["version"] == 1
            and type(marker["in_progress"]) is bool, "durable_observation_missing_or_invalid")
    return marker["in_progress"]


def owned_input_observation(state):
    record = state["custom"].get("_craftsman_context_v1")
    require(isinstance(record, dict) and type(record.get("version")) is int and record["version"] == 1, "owned_input_provenance_missing")
    digest(record.get("scope"))
    integer(record.get("sequence"), 1)
    stamp = record.get("last_input")
    require(isinstance(stamp, dict) and set(stamp) == {"revision", "digest"}, "owned_input_stamp_missing")
    revision = integer(stamp["revision"], 1, record["sequence"])
    bits = stamp["digest"]
    require(isinstance(bits, list) and len(bits) == 32 and all(type(value) is int and 0 <= value <= 255 for value in bits), "invalid_owned_input_digest")
    last = bytes(bits).hex()
    require(isinstance(record.get("updates"), list), "invalid_owned_input_records")
    records = [record.get("initial_input"), *record["updates"]]
    require(records[0] is not None and len(records) <= 5, "owned_original_input_missing")
    retained = []
    previous = 0
    for item in records:
        require(isinstance(item, dict), "invalid_owned_input_record")
        event = integer(item.get("event"), 1, record["sequence"])
        require(event > previous, "owned_input_order_invalid")
        previous = event
        digest(item.get("digest"))
        integer(item.get("original_bytes"), 0, 1024 * 1024)
        retained.append({"revision": event, "sha256": item["digest"], "original_bytes": item["original_bytes"]})
    require(retained[-1]["revision"] <= revision, "owned_input_stamp_order_invalid")
    if retained[-1]["revision"] == revision:
        require(retained[-1]["sha256"] == last, "owned_input_stamp_digest_mismatch")
    users = [hashlib.sha256(message["content"].encode()).hexdigest() for message in state["messages"] if message["role"] == "user"]
    require(last in users, "owned_last_input_not_in_checkpoint")
    if record.get("compactions") == 0:
        require(retained[0]["sha256"] in users, "owned_original_input_not_in_checkpoint")
    omissions = record.get("omissions")
    require(isinstance(omissions, dict), "owned_input_omission_evidence_missing")
    omitted = integer(omissions.get("input_events"))
    return {"scope_sha256": record["scope"], "last_revision": revision, "last_sha256": last,
            "initial_sha256": retained[0]["sha256"], "retained_inputs": retained, "omitted_input_events": omitted}


def group_positions(state):
    values = []
    messages = state["messages"]
    for index, message in enumerate(messages):
        if message["role"] != "assistant":
            continue
        following = []
        for position in range(index + 1, len(messages)):
            if messages[position]["role"] != "tool":
                break
            following.append((position, messages[position]))
        for ordinal, call in enumerate(message.get("tool_calls", [])):
            matched = [(position, result) for position, result in following if result["tool_call_id"] == call["id"]]
            require(len(matched) <= 1, "duplicate_group_tool_result")
            values.append({"assistant_index": index, "call_ordinal": ordinal, "call_sha256": canonical_sha256(call),
                           "result_index": matched[0][0] if matched else None,
                           "result_sha256": canonical_sha256(matched[0][1]) if matched else None,
                           "is_error": matched[0][1]["is_error"] if matched else None})
    return values


def safe_checkpoint(value):
    keys(value, ("version", "session_sha256", "seq", "message_count", "messages_sha256", "files_sha256",
                 "todos_sha256", "compactions", "accepted_input_sha256", "completed_groups",
                 "selected_once_command_count", "selected_once_result_sha256", "ordered_message_sha256",
                 "message_roles", "user_content_sha256", "durable_in_progress", "owned_input", "group_positions", "observed_direct_probe_reads"))
    require(type(value["version"]) is int and value["version"] == 1, "invalid_observation_version")
    for name in ("session_sha256", "messages_sha256", "files_sha256", "todos_sha256"):
        digest(value[name])
    integer(value["seq"], 0, 2**63 - 1)
    integer(value["message_count"], 0, 4096)
    require(type(value["durable_in_progress"]) is bool, "invalid_durable_observation")
    require(isinstance(value["ordered_message_sha256"], list) and isinstance(value["message_roles"], list)
            and len(value["ordered_message_sha256"]) == len(value["message_roles"]) == value["message_count"], "incoherent_checkpoint_counts")
    for item in value["ordered_message_sha256"]:
        digest(item)
    require(all(role in {"system", "user", "assistant", "tool"} for role in value["message_roles"]), "invalid_checkpoint_roles")
    owned = value["owned_input"]
    keys(owned, ("scope_sha256", "last_revision", "last_sha256", "initial_sha256", "retained_inputs", "omitted_input_events"))
    for key in ("scope_sha256", "last_sha256", "initial_sha256"):
        digest(owned[key])
    integer(owned["last_revision"], 1)
    integer(owned["omitted_input_events"])
    require(isinstance(owned["retained_inputs"], list) and 1 <= len(owned["retained_inputs"]) <= 5, "invalid_owned_input_records")
    previous = 0
    for item in owned["retained_inputs"]:
        keys(item, ("revision", "sha256", "original_bytes"))
        integer(item["original_bytes"], 0, 1048576)
        integer(item["revision"], previous + 1, owned["last_revision"])
        digest(item["sha256"])
        previous = item["revision"]
    require(owned["retained_inputs"][0]["sha256"] == owned["initial_sha256"], "owned_initial_input_mismatch")
    require(value["accepted_input_sha256"] == [item["sha256"] for item in owned["retained_inputs"]], "owned_input_digest_mismatch")
    require(isinstance(value["user_content_sha256"], list), "message_input_coherence_missing")
    users = []
    for item in value["user_content_sha256"]:
        keys(item, ("index", "sha256"))
        position = integer(item["index"], 0, value["message_count"] - 1)
        digest(item["sha256"])
        require(value["message_roles"][position] == "user", "message_input_coherence_invalid")
        users.append(item["sha256"])
    require(len(users) == value["message_roles"].count("user") and owned["last_sha256"] in users,
            "owned_input_not_present")
    integer(value["observed_direct_probe_reads"], 0, 10000)
    integer(value["selected_once_command_count"], 0, 10000)
    if value["compactions"] is not None:
        integer(value["compactions"])
    for name in ("accepted_input_sha256", "selected_once_result_sha256"):
        require(isinstance(value[name], list) and len(value[name]) <= 4096, "invalid_observation_digests")
        for item in value[name]:
            digest(item)
    require(isinstance(value["completed_groups"], list) and len(value["completed_groups"]) <= 10000,
            "invalid_observation_groups")
    require(sum(group.get("count", 0) for group in value["completed_groups"]) <= value["message_roles"].count("tool"), "original_tool_result_lost_or_replayed")
    require(isinstance(value["group_positions"], list), "group_instance_lineage_missing")
    for group in value["group_positions"]:
        keys(group, ("assistant_index", "call_ordinal", "call_sha256", "result_index", "result_sha256", "is_error"))
        integer(group["call_ordinal"], 0, 255)
        position = integer(group["assistant_index"], 0, value["message_count"] - 1)
        require(value["message_roles"][position] == "assistant", "invalid_group_instance")
        digest(group["call_sha256"])
        if group["result_index"] is not None:
            result = integer(group["result_index"], position + 1, value["message_count"] - 1)
            require(value["message_roles"][result] == "tool" and type(group["is_error"]) is bool, "invalid_group_instance")
            digest(group["result_sha256"])
    seen = set()
    for group in value["completed_groups"]:
        keys(group, ("sha256", "count"))
        digest(group["sha256"])
        integer(group["count"], 1, 10000)
        require(group["sha256"] not in seen, "duplicate_observation_group")
        seen.add(group["sha256"])
    return value


def grade_capture(scenario, task, capture, files, extension):
    """Deterministic checks over independently captured checkpoints and real files."""
    keys(capture, ("version", "kind", "scenario_id", "checkpoints", "gateway_requests",
                   "completed_update_turns", "observed_action", "effect_sha256_by_stage", "idle_reopen_elapsed_millis", "source_commit", "source_tree",
                   "feature_options_sha256", "effective_model_settings_sha256", "owned_input_receipts"))
    require(type(capture["version"]) is int and capture["version"] == 1
            and capture["kind"] == "operator_long_session_capture"
            and capture["scenario_id"] == scenario["id"], "invalid_capture_binding")
    for field in ("source_commit", "source_tree", "feature_options_sha256", "effective_model_settings_sha256"):
        require(capture[field] == extension[field], "different_long_session_capture_settings")
    keys(capture["checkpoints"], STAGES)
    snapshots = {name: safe_checkpoint(capture["checkpoints"][name]) for name in STAGES}
    before, after, reopened, final = (snapshots[name] for name in STAGES)
    require(len({row["session_sha256"] for row in snapshots.values()}) == 1, "different_session_reopened")
    require(all(row["durable_in_progress"] is False for row in snapshots.values()), "in_progress_is_not_completed")
    require(before["owned_input"].get("initial_sha256") == hashlib.sha256(task["prompt"].encode()).hexdigest(), "original_task_input_provenance_missing")
    require(before["seq"] <= after["seq"] <= reopened["seq"] < final["seq"], "checkpoint_sequence_not_retained")
    require(capture["completed_update_turns"] == LONG_LIMITS["update_turns"] and type(capture["completed_update_turns"]) is int,
            "incomplete_long_session_updates")
    receipts = capture["owned_input_receipts"]
    require(isinstance(receipts, list) and 12 <= len(receipts) <= 14, "missing_original_update_input")
    previous = 0
    for item in receipts:
        keys(item, ("sha256", "revision", "checkpoint_seq", "session_sha256", "scope_sha256", "message_index", "original_bytes", "source"))
        integer(item["original_bytes"], 0, 1048576)
        require(item["source"] == "actual_owned_context_stamp", "unowned_input_receipt")
        integer(item["revision"], previous + 1)
        integer(item["checkpoint_seq"])
        integer(item["message_index"], 0, 4095)
        previous = item["revision"]
        require(item["session_sha256"] == before["session_sha256"] and item["scope_sha256"] == before["owned_input"]["scope_sha256"], "input_receipt_scope_changed")
        digest(item["sha256"])
    expected_inputs = [hashlib.sha256(task["prompt"].encode()).hexdigest(), hashlib.sha256(scenario["setup_prompt"].encode()).hexdigest(),
                       *[hashlib.sha256(value.encode()).hexdigest() for value in updates(scenario)]]
    require([item["sha256"] for item in receipts[:12]] == expected_inputs, "missing_original_update_input")
    require(receipts[11]["revision"] == before["owned_input"]["last_revision"] and receipts[11]["sha256"] == before["owned_input"]["last_sha256"], "input_receipt_stage_mismatch")
    integer(capture["idle_reopen_elapsed_millis"], 5000, 10000)
    counts = capture["gateway_requests"]
    keys(counts, ("before_reopen", "after_idle_reopen", "final"))
    for count in counts.values():
        integer(count, 0, LONG_LIMITS["max_model_requests_total"])
    require(counts["before_reopen"] == counts["after_idle_reopen"] < counts["final"],
            "reopen_sampled_or_followup_absent")
    require(after["messages_sha256"] == reopened["messages_sha256"], "reopen_changed_messages")
    for channel in ("files_sha256", "todos_sha256"):
        require(after[channel] == reopened[channel], "reopen_lost_retained_state")
    for path, original in task["files"].items():
        if path in {"test_public.py", "test.cjs", "tests/public.rs", "Cargo.toml", ".gitignore"}:
            require(files.get(path) == original.encode("utf-8"), "protected_fixture_changed")
    for path, original in scenario["fixture_files"].items():
        if path != "probe_input.txt":
            require(files.get(path) == original.encode("utf-8"), "scenario_fixture_changed")
    effect = files.get(scenario["effect_file"])
    require(isinstance(effect, bytes), "effect_missing_changed_or_replayed")
    if scenario["id"] == "interruption-reopen-v1":
        try:
            actual_rows = [json.loads(line) for line in effect.decode("utf-8").splitlines()]
            expected_rows = [json.loads(line) for line in scenario["expected_effect"].splitlines()]
            require(canonical_sha256(actual_rows) == canonical_sha256(expected_rows),
                    "effect_missing_changed_or_replayed")
        except (ValueError, UnicodeError):
            raise Refused("effect_missing_changed_or_replayed") from None
    else:
        require(effect == scenario["expected_effect"].encode("utf-8"), "effect_missing_changed_or_replayed")
    keys(capture["effect_sha256_by_stage"], STAGES)
    require(all(value == hashlib.sha256(effect).hexdigest() for value in capture["effect_sha256_by_stage"].values()),
            "effect_changed_since_original_result")
    try:
        answer = json.loads(files["session_result.json"].decode("utf-8"))
    except (KeyError, UnicodeError, ValueError):
        raise Refused("missing_or_invalid_session_answer") from None
    require(answer == scenario["expected_answer"] and canonical_sha256(answer) == canonical_sha256(scenario["expected_answer"]),
            "incorrect_retained_answer")
    if scenario["id"] == "compaction-v1":
        require(capture["observed_action"] == "manual_compaction_saved", "compaction_return_not_observed")
        require(before["message_count"] >= 28 and after["messages_sha256"] != before["messages_sha256"],
                "compaction_not_observed")
        prior = before["compactions"]
        current = after["compactions"]
        require(type(prior) is int and type(current) is int and current == prior + 1, "context_compaction_not_retained")
        require(after["ordered_message_sha256"][-4:] == before["ordered_message_sha256"][-4:]
                and len(after["ordered_message_sha256"]) >= 5, "compaction_kept_tail_not_retained")
        require(after["owned_input"]["initial_sha256"] == before["owned_input"]["initial_sha256"], "compaction_original_intent_not_retained")
        for channel in ("files_sha256", "todos_sha256"):
            require(before[channel] == after[channel], "compaction_lost_retained_state")
    else:
        expected = "cancelled_return_then_reopen" if scenario["id"] == "interruption-reopen-v1" else "probe_result_saved_then_reopen"
        require(capture["observed_action"] == expected, "required_return_not_observed")
        for later in (after, reopened, final):
            require(later["ordered_message_sha256"][:before["message_count"]] == before["ordered_message_sha256"], "original_group_or_input_instance_moved_or_lost")
            require(later["group_positions"][:len(before["group_positions"])] == before["group_positions"], "original_group_instance_changed")
        original_groups = {row["sha256"]: row["count"] for row in before["completed_groups"]}
        require(bool(original_groups), "no_original_tool_result")
        for later in (after, reopened, final):
            retained = {row["sha256"]: row["count"] for row in later["completed_groups"]}
            require(all(retained.get(group) == count for group, count in original_groups.items()), "original_tool_result_lost_or_replayed")
        if scenario["id"] == "interruption-reopen-v1":
            require(all(row["selected_once_command_count"] == 1 for row in snapshots.values()), "effect_command_missing_or_replayed")
            require(before["selected_once_result_sha256"] == [hashlib.sha256(b"created I1\n").hexdigest()],
                    "actual_effect_result_not_observed")
        if scenario["id"] == "retained-results-v1":
            require(final["observed_direct_probe_reads"] == before["observed_direct_probe_reads"] == 0, "forbidden_direct_probe_reread")
            require(all(row["selected_once_command_count"] == 1 for row in snapshots.values()), "probe_missing_or_replayed")
            answer = scenario["expected_answer"]
            original_result = answer["stdout"] + "\n--- stderr ---\n" + answer["stderr"] + "\n[exit code 7]"
            require(before["selected_once_result_sha256"] == [hashlib.sha256(original_result.encode()).hexdigest()],
                    "actual_probe_result_not_observed")
            require(files.get("probe_input.txt") == b"changed after the original observation\n", "probe_input_was_not_changed")
    return {"scenario": scenario["id"], "observation_contract_passed": True,
            "operator_capture_provenance_authenticated": False,
            "synthetic_capture_checks_are_runtime_evidence": False}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--corpus", type=Path, required=True)
    parser.add_argument("--extension", type=Path, required=True)
    parser.add_argument("--scenario", choices=SCENARIOS, required=True)
    parser.add_argument("--capture", type=Path, required=True)
    parser.add_argument("--workspace", type=Path, required=True, help="Operator snapshot after agent processes stopped")
    parser.add_argument("--image", required=True, help="Immutable local image used for independent no-network grading")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--synthetic-metadata-selfcheck", action="store_true", help="Explicitly validate data fixtures without claiming a runtime receipt")
    args = parser.parse_args(argv)
    try:
        require(args.synthetic_metadata_selfcheck, "unsigned_caller_metadata_is_not_a_runtime_receipt")
        from preflight_continuation import validate_build
        validate_build({"source_commit": "0" * 40, "source_tree": "0" * 40, "image_id": args.image})
        corpus, corpus_hash = read_json(args.corpus, 8 * 1024 * 1024)
        extension, extension_hash = read_json(args.extension, 256 * 1024)
        validate_extension(extension, corpus, corpus_hash)
        scenario = next(row for row in extension["scenarios"] if row["id"] == args.scenario)
        task = next(row for row in corpus["tasks"] if row["id"] == scenario["base_task"])
        capture, capture_hash = read_json(args.capture, 8 * 1024 * 1024)
        from run_suite import snapshot_files
        files, rejected = snapshot_files(args.workspace)
        require(not rejected, "unsupported_snapshot_file")
        report = grade_capture(scenario, task, capture, files, extension)
        from check_corpus import grade
        with tempfile.TemporaryDirectory(prefix="craftsman-soak-grader-") as temporary:
            root = Path(temporary)
            root.chmod(0o755)
            candidate, grader = root / "candidate", root / "grader"
            candidate.mkdir()
            grader.mkdir()
            for relative, data in files.items():
                path = candidate / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(data)
            scored = grade(task, candidate, grader, args.image)
        report.update(original_held_out_passed=scored["passed"], original_grader_exit=scored["exit"],
                      corpus_sha256=corpus_hash, extension_sha256=extension_hash,
                      capture_sha256=capture_hash, paid_model_calls=0,
                      passed=scored["passed"], is_fresh_30_task_grade=False, is_runtime_result=False,
                      runtime_receipt_authenticated=False, metadata_selfcheck_only=True)
        with args.output.open("x") as output:
            json.dump(report, output, indent=2)
            output.write("\n")
        print(json.dumps(report, indent=2))
        return 0 if scored["passed"] else 1
    except (Refused, OSError, ValueError, KeyError, TypeError):
        print("Long-session grader refused: invalid_or_failed_observation", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
