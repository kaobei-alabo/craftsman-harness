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


class DockerRunIssuer:
    """Authority retained from actual driver-owned Docker processes."""
    def __init__(self, client, gateway, fixture, container, bindings, workspace, workspace_fd):
        from coding_eval_anchors import SYNTHETIC_IMAGE_ID, ORIGINAL_CORPUS_COMMIT, SYNTHETIC_BINARY_SHA256
        require(bindings["image_id"] == SYNTHETIC_IMAGE_ID and bindings["source_commit"] == ORIGINAL_CORPUS_COMMIT
                and bindings["binary_sha256"] == SYNTHETIC_BINARY_SHA256, "frozen_runtime_authority_required")
        for process in (client, gateway, fixture):
            require(type(process.process) is subprocess.Popen and process.process.poll() is None,
                    "actual_owned_process_authority_required")
        require(client.process.args[:4] == ["docker", "exec", "--interactive", container]
                and client.process.args[-1] == "--owned-tui-client-v1", "actual_owned_client_required")
        info = json.loads(subprocess.check_output(["docker", "inspect", container], stderr=subprocess.DEVNULL))[0]
        require(info["Image"] == SYNTHETIC_IMAGE_ID and info["HostConfig"]["ReadonlyRootfs"] is True,
                "actual_isolated_image_required")
        host = info["HostConfig"]
        require(host["PidsLimit"] == 128 and host["Memory"] == 805306368 and host["NanoCpus"] == 2000000000
                and host.get("Privileged") is False and host.get("NetworkMode", "").startswith("owned-eval-")
                and "ALL" in host.get("CapDrop", []) and "no-new-privileges" in host.get("SecurityOpt", []),
                "actual_container_limits_changed")
        require(len(info["Mounts"]) == 2 and any(mount["Destination"] == "/workspace" for mount in info["Mounts"])
                and any(mount["Destination"] == "/driver/client.py" and mount["RW"] is False for mount in info["Mounts"]),
                "task_only_mount_boundary_required")
        self.client, self.gateway, self.fixture = client, gateway, fixture
        self.container, self.bindings, self.workspace = container, bindings, Path(workspace)
        self.container_id = info["Id"]
        self.workspace_fd = os.dup(workspace_fd)
        actual_workspace = os.fstat(self.workspace_fd)
        named_workspace = os.stat(workspace)
        require((actual_workspace.st_dev, actual_workspace.st_ino) == (named_workspace.st_dev, named_workspace.st_ino), "workspace_authority_changed")

    def validate_completion(self, capture, gateway, fixture, returns, files):
        require(self.client.process.poll() in (None, 0), "owned_client_failed")
        observed_returns = [value for value in self.client.history if value.get("kind") == "owner_returned"]
        require(observed_returns == [{"kind": "owner_returned", **value} if "kind" not in value else value for value in returns],
                "caller_return_facts_rejected")
        require(any(value.get("kind") == "client_finished" for value in self.client.history), "actual_client_finish_required")
        for stage, expected in capture["checkpoints"].items():
            actual = [value for value in self.client.history if value.get("kind") == "checkpoint_captured" and value.get("stage") == stage]
            require(len(actual) == 1 and actual[0]["checkpoint"] == expected, "caller_checkpoint_facts_rejected")
            if capture.get("kind") == "operator_long_session_capture":
                require(actual[0].get("root_state") in {"done", "idle"} and actual[0].get("durable_in_progress") is False,
                        "error_or_unfinished_stage_is_not_positive")
            elif capture.get("kind") == "negative_external_observation_capture" and stage == "after":
                required_state = "error" if capture["variant"] in {"save-refusal", "provider-eof"} else "done"
                require(actual[0].get("root_state") == required_state, "negative_return_channel_not_observed")
            else:
                require(capture.get("kind") == "negative_external_observation_capture", "unsupported_capture_kind")
        finished = next(value for value in self.client.history if value.get("kind") == "client_finished")
        require(finished["owned_input_receipts"] == capture["owned_input_receipts"], "caller_input_facts_rejected")
        require(gateway == [value for value in self.gateway.history if value.get("kind") == "private_gateway_observation"],
                "caller_gateway_facts_rejected")
        require(fixture == self.fixture.history, "caller_fixture_facts_rejected")
        if capture.get("kind") == "negative_external_observation_capture":
            if capture["variant"] == "save-refusal":
                require(any(value.get("kind") == "owned_store_writer_hold_started" for value in self.client.history)
                        and any(value.get("kind") == "owned_store_writer_hold_released" for value in self.client.history),
                        "actual_writer_hold_not_observed")
            elif capture["variant"] == "provider-eof":
                require(any(value.get("phase") == "provider_eof" and value.get("terminal_event") is None for value in gateway),
                        "actual_provider_eof_not_observed")
        result = subprocess.run(["docker", "inspect", self.container_id], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        require(result.returncode != 0, "actual_container_removal_required")
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
        os.close(self._workspace)


def seal_actual_driver(issuer, root, bindings, capture, gateway, fixture, returns, workspace, files):
    """Called only after the runner observes real returns and removes the container."""
    require(type(issuer) is DockerRunIssuer and issuer.bindings == bindings, "driver_authority_required")
    issuer.validate_completion(capture, gateway, fixture, returns, files)
    require(bool(returns) and all(value["child_exit"] == 0 and value["keeper_exit"] == 0
                                 and value["modes_restored"] is True for value in returns),
            "owned_returns_required")
    require(bool(gateway) and any(value.get("phase") == "admitted" for value in gateway),
            "actual_gateway_observations_required")
    root = Path(root)
    encoded = [(name, canonical(value)) for name, value in (("capture.json", capture), ("gateway.json", gateway),
                        ("fixture.json", fixture), ("returns.json", returns), ("files.json", files))]
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
    document = {"version": 1, "producer": "actual_owned_tui_synthetic_driver", "bindings": bindings,
                "observations": observations, "actual_agent_container_removed": True,
                "paid_model_calls": 0, "is_live_quality_score": False}
    envelope = {"document": document, "hmac_sha256": hmac.new(key, canonical(document), hashlib.sha256).hexdigest()}
    with (root / "receipt.json").open("xb") as output:
        output.write(canonical(envelope))
    fd = os.dup(issuer.workspace_fd)
    os.close(issuer.workspace_fd)
    info = os.fstat(fd)
    require(stat.S_ISDIR(info.st_mode), "retained_workspace_required")
    return AuthenticatedRun(root, key, envelope, fd, (info.st_dev, info.st_ino), _ISSUED)


def require_authenticated(value, bindings):
    require(type(value) is AuthenticatedRun, "caller_json_is_not_a_driver_receipt")
    return value.verified_capture(bindings)
