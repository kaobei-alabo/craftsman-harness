"""Driver-held receipt authority; caller JSON alone is never a runtime receipt.

The operator and Docker daemon are trusted. Keys and retained workspace
authority remain outside the task container. This is not protection against
an operator who modifies the driver or reads its private signing key.
"""
import hashlib
import hmac
import json
import os
from pathlib import Path
import secrets
import stat
import subprocess

from preflight_continuation import canonical, read_json, require, Refused


_ISSUED = object()


def validate_isolation(info, bindings, network_info, collector=False):
    from coding_eval_anchors import SYNTHETIC_IMAGE_ID
    require(info["Image"] == SYNTHETIC_IMAGE_ID and info["HostConfig"]["ReadonlyRootfs"] is True,
            "actual_isolated_image_required")
    host = info["HostConfig"]
    require(host["PidsLimit"] == 128 and host["Memory"] == 805306368 and host["NanoCpus"] == 2000000000
            and host.get("Privileged") is False and host.get("NetworkMode") == bindings["network_name"]
            and "ALL" in host.get("CapDrop", []) and "no-new-privileges" in host.get("SecurityOpt", []),
            "actual_container_limits_changed")
    require(network_info.get("Internal") is True and network_info["Name"] == bindings["network_name"]
            and info["NetworkSettings"]["Networks"][bindings["network_name"]]["NetworkID"] == network_info["Id"],
            "actual_internal_network_required")
    expected = {"/workspace": (bindings["mount_sources"]["workspace"], True),
                "/driver/collector.py" if collector else "/driver/client.py":
                    (bindings["mount_sources"]["collector" if collector else "client"], False)}
    require(len(info["Mounts"]) == 2 and {mount["Destination"] for mount in info["Mounts"]} == set(expected),
            "task_only_mount_boundary_required")
    for mount in info["Mounts"]:
        source, writable = expected[mount["Destination"]]
        require(mount.get("Type") == "bind" and mount["Source"] == source and mount["RW"] is writable,
                "actual_mount_source_or_mode_changed")


def require_container_absent(identity):
    # A failed inspect may be a daemon/query error. Only a successful targeted
    # container-list query with no matching owned ID proves observed absence.
    result = subprocess.run(["docker", "container", "ls", "--all", "--no-trunc", "--filter", "id=" + identity,
                             "--format", "{{.ID}}"], stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=15)
    require(result.returncode == 0, "container_absence_query_failed")
    require(len(result.stdout) <= 65536 and result.stdout.strip() == b"", "actual_container_removal_required")
    return {"query_exit": result.returncode, "matching_container_count": 0}


def verify_driver_sources(root, bindings):
    from owned_tui_files import read_leaf
    manifest, manifest_digest = read_json(Path(root) / "trusted-source-manifest.json", 65536)
    require(manifest == bindings["driver_files"] and manifest_digest == bindings["driver_source_snapshot_sha256"],
            "trusted_driver_source_manifest_changed")
    directory = os.open(Path(root) / "trusted-driver-source", os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        require(set(os.listdir(directory)) == set(manifest), "trusted_driver_source_members_changed")
        for name, expected in manifest.items():
            require(name.endswith(".py") and hashlib.sha256(read_leaf(directory, name)).hexdigest() == expected,
                    "trusted_driver_source_bytes_changed")
    finally:
        os.close(directory)
    directory = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        require(hashlib.sha256(read_leaf(directory, "trusted-client.py")).hexdigest() == bindings["mounted_client_sha256"],
                "trusted_client_bundle_changed")
        if bindings["mounted_collector_sha256"] is not None:
            require(hashlib.sha256(read_leaf(directory, "trusted-collector.py")).hexdigest() == bindings["mounted_collector_sha256"],
                    "trusted_collector_bundle_changed")
    finally:
        os.close(directory)
    return manifest_digest


def validate_native_child(facts, intent, bindings, snapshots, fixture, capture):
    from native_probe_wrapper import CHILD_SOURCE_SHA256
    require(set(facts) == {"version", "kind", "complete", "child_instance", "child_source_sha256",
        "source_instance_binding_acknowledged", "actual_wait_returned", "actual_child_exit", "stdout_eof", "stderr_eof", "ack_pipe_eof", "source_authority",
        "exact_executed_buffer_verified", "stdout_bytes", "stderr_bytes", "stdout_sha256", "stderr_sha256",
        "outer_toolmeta_observed", "execution_limit_seconds", "cleanup_limit_seconds", "stream_limit_bytes", "cleanup",
        "classification", "collector_binding_sha256", "producer_source_sha256", "local_forward_write_returned",
        "forward_bytes", "forward_sha256", "receiver_exactly_once_verified"}, "native_complete_contract_changed")
    require(type(facts["version"]) is int and facts["version"] == 3 and facts["kind"] == "native_child_completed"
            and facts["classification"] == "independently_requested_fixed_fixture_child"
            and facts["complete"] is True and facts["actual_wait_returned"] is True
            and facts["exact_executed_buffer_verified"] is True and facts["source_instance_binding_acknowledged"] is True
            and facts["local_forward_write_returned"] is True and facts["outer_toolmeta_observed"] is False
            and facts["stdout_eof"] is True and facts["stderr_eof"] is True and facts["ack_pipe_eof"] is True
            and facts["receiver_exactly_once_verified"] is False and facts["child_source_sha256"] == CHILD_SOURCE_SHA256
            and facts["source_authority"] == "verified_bounded_snapshot" and type(facts["actual_child_exit"]) is int
            and facts["actual_child_exit"] == 7 and facts["execution_limit_seconds"] == 20
            and facts["cleanup_limit_seconds"] == 5 and facts["stream_limit_bytes"] == 65536,
            "native_source_wait_stream_or_scope_incomplete")
    instance = facts["child_instance"]
    require(isinstance(instance, str) and len(instance) == 32 and all(char in "0123456789abcdef" for char in instance)
            and instance != "0" * 32, "native_child_instance_refused")
    require(facts["cleanup"] == {"needed": False, "actual_wait_returned": True, "actual_child_exit": 7},
            "native_actual_cleanup_incomplete")
    require(all(intent.get(key) == expected for key, expected in bindings["collector_bootstrap"].items())
            and intent["call_sha256"] == bindings["native_expected_call_sha256"]
            and facts["producer_source_sha256"] == bindings["native_wrapper_sha256"]
            and facts["collector_binding_sha256"] == hashlib.sha256(canonical(intent)).hexdigest(),
            "native_collector_source_or_intent_binding_changed")
    rows = [row for row in snapshots if row.get("snapshot_id") == intent["snapshot_id"]]
    require(len(rows) == 1 and rows[0]["kind"] == "private_snapshot_verified"
            and rows[0]["root_state"] == "blocked" and rows[0]["root_kind"] == "permission"
            and rows[0]["session_sha256"] == intent["session_sha256"] and rows[0]["seq"] == intent["checkpoint_seq"]
            and any(group["call_sha256"] == intent["call_sha256"] and group["assistant_index"] == intent["assistant_index"]
                    and group["call_ordinal"] == intent["call_ordinal"] and group["result_index"] is None
                    for group in rows[0]["pending_group_instances"]), "native_intended_pending_group_not_observed")
    requests = [value for value in fixture if value.get("kind") == "fixture_request_observed"
                and value.get("phase") == "setup" and value.get("phase_index") == 0]
    require(len(requests) == 1 and requests[0]["ordinal"] == intent["request_ordinal"]
            and requests[0]["request_sha256"] == intent["request_sha256"], "native_actual_request_binding_changed")
    expected = bindings["native_expected_streams"]
    require(all(facts[name] == value for name, value in expected.items()), "native_actual_streams_changed")
    results = [value for value in fixture if value.get("kind") == "fixture_native_relay_result_verified"]
    require(len(results) == 1 and results[0]["call_sha256"] == intent["call_sha256"], "native_saved_relay_result_unobserved")
    before = capture["checkpoints"]["before"]
    groups = [group for group in before["group_positions"] if group["call_sha256"] == intent["call_sha256"]]
    require(len(groups) == 1 and groups[0]["result_content_sha256"] == results[0]["output_sha256"],
            "native_saved_relay_result_changed")
    # A stored intended group and actual relay result are not independently
    # authenticated outer approval/dispatch or ToolMeta authority.
    return facts


def validate_negative_condition(capture, fixture, bindings, files):
    """Require a new specific saved result, not a label or sequence increase.

    These are stored-result/provider-request observations. They do not assert
    a typed outer ToolMeta, a private callback or arbitrary-shell read absence.
    """
    from grade_long_session import safe_checkpoint
    variant = capture.get("variant")
    require(variant in {"duplicate-create", "forbidden-reread"}, "specific_negative_condition_required")
    expected = bindings["negative_conditions"][variant]
    before = safe_checkpoint(capture["checkpoints"]["before"])
    after = safe_checkpoint(capture["checkpoints"]["after"])
    require(before["session_sha256"] == after["session_sha256"] and after["seq"] > before["seq"]
            and before["compactions"] == after["compactions"]
            and before["durable_in_progress"] is False and after["durable_in_progress"] is False,
            "negative_checkpoint_transition_not_observed")
    require(after["ordered_message_sha256"][:before["message_count"]] == before["ordered_message_sha256"]
            and after["group_positions"][:len(before["group_positions"])] == before["group_positions"],
            "negative_original_instance_moved_or_lost")
    owned = after["owned_input"]
    require(owned["scope_sha256"] == before["owned_input"]["scope_sha256"]
            and owned["last_revision"] > before["owned_input"]["last_revision"]
            and owned["last_sha256"] == expected["input_sha256"], "specific_negative_input_not_observed")
    receipts = [value for value in capture["owned_input_receipts"] if value.get("sha256") == expected["input_sha256"]]
    require(len(receipts) == 1, "specific_negative_input_receipt_missing")
    receipt = receipts[0]
    require(receipt.get("source") == "actual_owned_context_stamp"
            and receipt["session_sha256"] == after["session_sha256"]
            and receipt["scope_sha256"] == owned["scope_sha256"]
            and receipt["revision"] == owned["last_revision"]
            and before["seq"] < receipt["checkpoint_seq"] <= after["seq"]
            and receipt["message_index"] >= before["message_count"]
            and {"index": receipt["message_index"], "sha256": expected["input_sha256"]} in after["user_content_sha256"],
            "specific_negative_input_instance_invalid")
    call = expected["call_sha256"]
    require(not any(group["call_sha256"] == call for group in before["group_positions"]),
            "negative_call_is_stale")
    groups = [group for group in after["group_positions"] if group["call_sha256"] == call]
    require(len(groups) == 1 and groups[0]["assistant_index"] > receipt["message_index"]
            and groups[0]["call_ordinal"] == 0 and groups[0]["result_index"] is not None,
            "specific_new_negative_result_missing_or_relocated")
    phase = expected["phase"]
    requests = [value for value in fixture if value.get("kind") == "fixture_request_observed" and value.get("phase") == phase]
    verified = [value for value in fixture if value.get("kind") == "fixture_negative_result_verified" and value.get("phase") == phase]
    require(len(requests) == 2 and [value["phase_index"] for value in requests] == [0, 1]
            and requests[1]["ordinal"] == requests[0]["ordinal"] + 1 and len(verified) == 1,
            "new_negative_provider_result_not_observed")
    result = verified[0]
    require(result.get("phase_index") == 1 and result.get("ordinal") == requests[1]["ordinal"]
            and result.get("call_sha256") == call and result.get("condition") == expected["condition"]
            and result.get("output_sha256") == groups[0]["result_content_sha256"],
            "specific_negative_result_binding_mismatch")
    if variant == "duplicate-create":
        require(before["selected_once_command_count"] == 1 and after["selected_once_command_count"] == 2
                and len(before["selected_once_result_sha256"]) == 1 and len(after["selected_once_result_sha256"]) == 2
                and after["selected_once_result_sha256"][-1] == result["output_sha256"],
                "actual_duplicate_command_delta_missing")
    else:
        require(before["observed_direct_probe_reads"] == 0 and after["observed_direct_probe_reads"] == 1,
                "actual_direct_read_delta_missing")
    effect = files[expected["effect_file"]]["sha256"]
    require(capture["effect_sha256_by_stage"] == {"before": effect, "after": effect},
            "negative_original_effect_changed")
    return {"condition": expected["condition"], "call_sha256": call,
            "assistant_index": groups[0]["assistant_index"], "result_index": groups[0]["result_index"],
            "result_content_sha256": result["output_sha256"], "provider_ordinal": result["ordinal"],
            "input_revision": receipt["revision"], "checkpoint_seq": after["seq"]}


class DockerRunIssuer:
    """Authority retained from actual driver-owned Docker processes."""
    def __init__(self, client, gateway, fixture, container, bindings, workspace, workspace_fd,
                 collector=None, collector_container=None):
        from coding_eval_anchors import SYNTHETIC_IMAGE_ID, ORIGINAL_CORPUS_COMMIT, SYNTHETIC_BINARY_SHA256
        require(bindings["image_id"] == SYNTHETIC_IMAGE_ID and bindings["source_commit"] == ORIGINAL_CORPUS_COMMIT
                and bindings["binary_sha256"] == SYNTHETIC_BINARY_SHA256, "frozen_runtime_authority_required")
        for process in (client, gateway, fixture):
            require(type(process.process) is subprocess.Popen and process.process.poll() is None,
                    "actual_owned_process_authority_required")
        require(client.process.args[:4] == ["docker", "exec", "--interactive", container]
                and client.process.args[-1] == "--owned-tui-client-v1", "actual_owned_client_required")
        from owned_tui_snapshot import SnapshotTransport
        require(type(client.snapshot_service) is SnapshotTransport, "actual_private_snapshot_authority_required")
        info = json.loads(subprocess.check_output(["docker", "inspect", container], stderr=subprocess.DEVNULL, timeout=15))[0]
        network_info = json.loads(subprocess.check_output(["docker", "network", "inspect", bindings["network_name"]],
                                                        stderr=subprocess.DEVNULL, timeout=15))[0]
        validate_isolation(info, bindings, network_info)
        self.client, self.gateway, self.fixture = client, gateway, fixture
        self.container, self.bindings, self.workspace = container, bindings, Path(workspace)
        self.container_id = info["Id"]
        self.collector, self.collector_container_id = collector, None
        if collector is not None:
            require(type(collector.process) is subprocess.Popen and collector.process.poll() is None
                    and collector.process.args[-1] == "--owned-native-collector-v3", "actual_inner_collector_channel_required")
            collector_info = json.loads(subprocess.check_output(["docker", "inspect", collector_container],
                                                               stderr=subprocess.DEVNULL, timeout=15))[0]
            validate_isolation(collector_info, bindings, network_info, collector=True)
            self.collector_container_id = collector_info["Id"]
            ready = [value for value in collector.history if value.get("kind") == "collector_ready"]
            require(len(ready) == 1 and ready[0]["collector_source_sha256"] == bindings["mounted_collector_sha256"]
                    and ready[0]["actual_binary_sha256"] == SYNTHETIC_BINARY_SHA256, "actual_collector_producer_binding_changed")
        self.workspace_fd = os.dup(workspace_fd)
        try:
            actual_workspace = os.fstat(self.workspace_fd)
            named_workspace = os.stat(workspace, follow_symlinks=False)
            require((actual_workspace.st_dev, actual_workspace.st_ino) == (named_workspace.st_dev, named_workspace.st_ino)
                    and stat.S_ISDIR(named_workspace.st_mode), "workspace_authority_changed")
        except BaseException:
            self.close()
            raise

    def close(self):
        if self.workspace_fd is not None:
            os.close(self.workspace_fd)
            self.workspace_fd = None

    def validate_completion(self, capture, gateway, fixture, returns, files):
        require(self.client.process.poll() in (None, 0), "owned_client_failed")
        observed_returns = [value for value in self.client.history if value.get("kind") == "owner_returned"]
        require(observed_returns == [{"kind": "owner_returned", **value} if "kind" not in value else value for value in returns],
                "caller_return_facts_rejected")
        require(any(value.get("kind") == "client_finished" for value in self.client.history), "actual_client_finish_required")
        for stage, expected in capture["checkpoints"].items():
            actual = [value for value in self.client.history if value.get("kind") == "checkpoint_captured" and value.get("stage") == stage]
            require(len(actual) == 1 and actual[0]["checkpoint"] == expected, "caller_checkpoint_facts_rejected")
            require(any(value.get("kind") == "private_snapshot_verified"
                        and value.get("checkpoint_sha256") == hashlib.sha256(canonical(expected)).hexdigest()
                        for value in self.client.history), "actual_verified_private_snapshot_required")
            if capture.get("kind") == "operator_long_session_capture":
                require(actual[0].get("root_state") in {"done", "idle"} and actual[0].get("durable_in_progress") is False,
                        "error_or_unfinished_stage_is_not_positive")
            elif capture.get("kind") == "negative_external_observation_capture" and stage == "after":
                required_state = "error" if capture["variant"] in {"save-refusal", "provider-eof"} else "done"
                require(actual[0].get("root_state") == required_state, "negative_return_channel_not_observed")
                if capture["variant"] in {"duplicate-create", "forbidden-reread"}:
                    require(actual[0].get("after_owned_return") is True, "negative_completed_owned_return_required")
                    position = self.client.history.index(actual[0])
                    require(any(value.get("kind") == "owner_returned" and value.get("child_exit") == 0
                                and value.get("keeper_exit") == 0 and value.get("modes_restored") is True
                                and value.get("root_state") == "done" for value in self.client.history[:position]),
                            "negative_owned_return_not_observed_before_snapshot")
            else:
                require(capture.get("kind") == "negative_external_observation_capture", "unsupported_capture_kind")
        finished = next(value for value in self.client.history if value.get("kind") == "client_finished")
        require(finished["owned_input_receipts"] == capture["owned_input_receipts"], "caller_input_facts_rejected")
        require(gateway == [value for value in self.gateway.history if value.get("kind") == "private_gateway_observation"],
                "caller_gateway_facts_rejected")
        require(fixture == self.fixture.history, "caller_fixture_facts_rejected")
        if capture.get("kind") == "negative_external_observation_capture":
            require(capture["variant"] != "save-refusal", "live_writer_connection_unverified_quarantined")
            if capture["variant"] == "save-refusal":
                require(any(value.get("kind") == "owned_store_writer_hold_started" for value in self.client.history)
                        and any(value.get("kind") == "owned_store_writer_hold_released" for value in self.client.history),
                        "actual_writer_hold_not_observed")
            elif capture["variant"] == "provider-eof":
                require(any(value.get("phase") == "provider_eof" and value.get("terminal_event") is None for value in gateway),
                        "actual_provider_eof_not_observed")
            elif capture["variant"] in {"duplicate-create", "forbidden-reread"}:
                validate_negative_condition(capture, fixture, self.bindings, files)
        require_container_absent(self.container_id)
        if self.collector is not None:
            require_container_absent(self.collector_container_id)
            completed = [value for value in self.collector.history if value.get("kind") == "native_child_completed"]
            intents = [value for value in self.collector.history if value.get("kind") == "collector_intent_reserved"]
            require(len(completed) == len(intents) == 1 and not any(value.get("kind") == "native_child_incomplete"
                    for value in self.collector.history), "actual_complete_owned_child_channel_required")
            validate_native_child(completed[0], intents[0]["binding"], self.bindings,
                [value for value in self.client.history if value.get("kind") == "private_snapshot_verified"], fixture, capture)
        from owned_tui_files import snapshot_retained
        actual = snapshot_retained(self.workspace_fd)
        expected = {name: {"bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()} for name, data in actual.items()}
        require(expected == files, "caller_workspace_facts_rejected")


class AuthenticatedRun:
    __slots__ = ("_root", "_key", "_envelope", "_workspace", "_identity")

    def __init__(self, root, key, envelope, workspace, identity, issuer):
        require(issuer is _ISSUED, "driver_authority_required")
        self._root, self._key, self._envelope = root, key, envelope
        self._workspace, self._identity = workspace, identity

    def verified_capture(self, expected):
        require(type(self) is AuthenticatedRun, "driver_authority_required")
        envelope, _ = read_json(self._root / "receipt.json", 8 * 1024 * 1024)
        require(envelope == self._envelope, "driver_receipt_changed")
        document = envelope["document"]
        mac = hmac.new(self._key, canonical(document), hashlib.sha256).hexdigest()
        require(hmac.compare_digest(mac, envelope["hmac_sha256"]), "driver_receipt_authentication_failed")
        require(document["bindings"] == expected, "driver_receipt_binding_changed")
        require(document["producer"] == "actual_owned_tui_synthetic_driver"
                and document["actual_agent_container_removed"] is True, "actual_driver_completion_required")
        require(document["paid_model_calls"] == 0, "synthetic_receipt_required")
        if document.get("version") == 3:
            verify_driver_sources(self._root, document["bindings"])
        for name, expected_digest in document["observations"].items():
            _, actual = read_json(self._root / name, 8 * 1024 * 1024)
            require(actual == expected_digest, "driver_observation_changed")
        info = os.fstat(self._workspace)
        require((info.st_dev, info.st_ino) == self._identity, "retained_workspace_changed")
        from owned_tui_files import snapshot_retained
        actual = snapshot_retained(self._workspace)
        file_manifest, _ = read_json(self._root / "files.json", 8 * 1024 * 1024)
        require(file_manifest == {name: {"bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()} for name, data in actual.items()},
                "signed_task_snapshot_changed")
        capture, _ = read_json(self._root / "capture.json", 8 * 1024 * 1024)
        return capture

    def close(self):
        if self._workspace is not None:
            os.close(self._workspace)
            self._workspace = None


def seal_actual_driver(issuer, root, bindings, capture, gateway, fixture, returns, workspace, files):
    """Called only after the runner observes real returns and removes the container."""
    require(type(issuer) is DockerRunIssuer and issuer.bindings == bindings, "driver_authority_required")
    try:
        return _seal_actual_driver(issuer, root, bindings, capture, gateway, fixture, returns, workspace, files)
    finally:
        issuer.close()


def _seal_actual_driver(issuer, root, bindings, capture, gateway, fixture, returns, workspace, files):
    issuer.validate_completion(capture, gateway, fixture, returns, files)
    manifest_digest = verify_driver_sources(root, bindings)
    require(bool(returns) and all(value["child_exit"] == 0 and value["keeper_exit"] == 0
                                 and value["modes_restored"] is True for value in returns),
            "owned_returns_required")
    require(bool(gateway) and any(value.get("phase") == "admitted" for value in gateway),
            "actual_gateway_observations_required")
    root = Path(root)
    encoded = [(name, canonical(value)) for name, value in (("capture.json", capture), ("gateway.json", gateway),
                        ("fixture.json", fixture), ("returns.json", returns), ("files.json", files),
                        ("snapshots.json", [value for value in issuer.client.history if value.get("kind") == "private_snapshot_verified"]),
                        ("collector.json", [] if issuer.collector is None else issuer.collector.history))]
    require(sum(len(data) for _, data in encoded) <= 262144, "whole_driver_metadata_bound")
    key = secrets.token_bytes(32)
    with (root / "issuer.key").open("xb") as output:
        os.chmod(root / "issuer.key", 0o600)
        output.write(key)
    observations = {}
    for name, data in encoded:
        with (root / name).open("xb") as output:
            output.write(data)
        observations[name] = hashlib.sha256(data).hexdigest()
    observations["trusted-source-manifest.json"] = manifest_digest
    document = {"version": 3, "producer": "actual_owned_tui_synthetic_driver", "bindings": bindings,
                "observations": observations, "actual_agent_container_removed": True,
                "paid_model_calls": 0, "is_live_quality_score": False}
    envelope = {"document": document, "hmac_sha256": hmac.new(key, canonical(document), hashlib.sha256).hexdigest()}
    with (root / "receipt.json").open("xb") as output:
        output.write(canonical(envelope))
    fd = os.dup(issuer.workspace_fd)
    try:
        info = os.fstat(fd)
        require(stat.S_ISDIR(info.st_mode), "retained_workspace_required")
        return AuthenticatedRun(root, key, envelope, fd, (info.st_dev, info.st_ino), _ISSUED)
    except BaseException:
        os.close(fd)
        raise


def require_authenticated(value, bindings):
    require(type(value) is AuthenticatedRun, "caller_json_is_not_a_driver_receipt")
    return value.verified_capture(bindings)
