"""Transient retained-SQLite channel; raw rows/bodies are never report events."""
import base64
import hashlib
import json

from grade_long_session import safe_checkpoint_observation
from preflight_continuation import canonical, require, Refused
from sqlite_snapshot import MAXIMUM, NAMES, PrivateReader, SnapshotFailure, validate_pair


PRIVATE_KINDS = {"private_snapshot_begin", "private_snapshot_chunk", "private_snapshot_end"}
TRANSFER_TOTAL_LIMIT = 536870912
TRANSFER_CHUNK_LIMIT = 49152


class SnapshotTransport:
    def __init__(self, parent, workspace, scenario, native_hook=None):
        self.reader = PrivateReader(parent, [workspace])
        self.scenario = scenario
        self.native_hook = native_hook
        self.active = None
        self.seen = set()
        self.total_bytes = 0
        self.thread = None
        self.scope = None
        self.sequence = -1
        self.last = None
        self.current = None
        self.root_identity = None
        self.main_identity = None
        self.members = None
        self.source_epoch = 0
        self.wal_generation = None

    def expect_input(self, phase, text, calls=()):
        require(self.active is None, "snapshot_transfer_in_progress")
        digest = hashlib.sha256(text.encode()).hexdigest()
        self.current = {"phase": phase, "input_sha256": digest, "base_seq": self.sequence,
                        "calls": [hashlib.sha256(canonical(call)).hexdigest() for call in calls]}
        self.current["done_seq"] = self.sequence + (len(calls) + 6 if calls else 4)
        if phase in {"pending", "eof"}:
            self.current["done_seq"] = self.sequence + (3 if phase == "pending" else 2)

    def expect_control(self, phase):
        require(self.last is not None and self.active is None, "snapshot_control_without_bound_row")
        self.current = {"phase": phase, "input_sha256": self.last["owned_input"]["last_sha256"],
                        "base_seq": self.sequence, "calls": [],
                        "done_seq": self.sequence + (2 if phase == "compact" else 0)}

    def finish_expected_step(self):
        require(self.current is not None and self.last is not None
                and self.last["seq"] == self.current["done_seq"], "current_step_final_sequence_not_observed")
        self.sequence = self.current["done_seq"]

    def validate_current_row(self, observation, root_state):
        require(self.current is not None, "snapshot_without_owned_operation_expectations")
        current = self.current
        owned = observation["owned_input"]
        require(owned["last_sha256"] == current["input_sha256"], "snapshot_current_step_input_mismatch")
        if self.scope is not None:
            require(owned["scope_sha256"] == self.scope, "snapshot_owned_scope_changed")
        if root_state == "blocked":
            require(current["calls"] and current["base_seq"] + 3 <= observation["seq"]
                    < current["base_seq"] + 3 + len(current["calls"]), "snapshot_pending_step_sequence_mismatch")
            call_hashes = [group["call_sha256"] for group in observation["group_positions"]
                           if group["assistant_index"] > next(
                               row["index"] for row in reversed(observation["user_content_sha256"])
                               if row["sha256"] == current["input_sha256"])]
            require(call_hashes == current["calls"], "snapshot_current_step_call_body_mismatch")
        else:
            require(observation["seq"] == current["done_seq"], "snapshot_current_step_sequence_mismatch")
        self.scope = owned["scope_sha256"]

    def receive(self, value, send):
        kind = value.get("kind")
        if kind == "private_snapshot_begin":
            require(self.active is None and len(self.seen) < 256, "snapshot_duplicate_or_excess_transfer")
            identity = value["snapshot_id"]
            require(isinstance(identity, str) and len(identity) == 32
                    and all(char in "0123456789abcdef" for char in identity) and identity not in self.seen,
                    "snapshot_transfer_instance_rejected")
            require(value["root_state"] in {"idle", "done", "error", "blocked"}
                    and (value["root_state"] != "blocked" or value["root_kind"] == "permission"),
                    "snapshot_not_at_observed_scoped_quiescent_stage")
            require(set(value["digests"]) <= set(NAMES) and "threads.db" in value["digests"]
                    and set(value["versions"]) == set(NAMES) and type(value["raw_bytes"]) is int
                    and 0 < value["raw_bytes"] <= MAXIMUM, "snapshot_transfer_manifest_invalid")
            require(all((value["versions"][name] is None) == (name not in value["digests"]) for name in NAMES),
                    "snapshot_optional_members_unbound")
            require(self.thread is None or value["thread"] == self.thread, "snapshot_client_thread_changed")
            require(isinstance(value["root_version"], list) and len(value["root_version"]) == 7
                    and all(type(item) is int for item in value["root_version"])
                    and all(member is None or isinstance(member, list) and len(member) == 7
                            and all(type(item) is int for item in member) for member in value["versions"].values()),
                    "snapshot_full_versions_missing")
            root_identity = tuple(value["root_version"][:3])
            main_identity = tuple(value["versions"]["threads.db"][:3])
            require(self.root_identity in (None, root_identity) and self.main_identity in (None, main_identity),
                    "snapshot_source_authority_rebound")
            members = {name: None if item is None else tuple(item[:3]) for name, item in value["versions"].items()}
            wal_generation = value["pair"]["wal_generation_sha256"]
            expected_epoch = self.source_epoch + (members != self.members or wal_generation != self.wal_generation)
            require(type(value["source_epoch"]) is int and value["source_epoch"] == expected_epoch,
                    "snapshot_source_epoch_mismatch")
            self.root_identity, self.main_identity, self.members = root_identity, main_identity, members
            self.source_epoch = expected_epoch
            self.wal_generation = wal_generation
            self.seen.add(identity)
            self.active = {**value, "files": {name: bytearray() for name in value["digests"]}}
            return None
        require(self.active is not None and value.get("snapshot_id") == self.active["snapshot_id"],
                "snapshot_foreign_or_missing_transfer")
        active = self.active
        if kind == "private_snapshot_chunk":
            name = value["name"]
            require(name in active["files"] and type(value["offset"]) is int
                    and value["offset"] == len(active["files"][name]), "snapshot_chunk_order_invalid")
            data = base64.b64decode(value["data"], validate=True)
            require(0 < len(data) <= TRANSFER_CHUNK_LIMIT and len(active["files"][name]) + len(data) <= MAXIMUM,
                    "snapshot_chunk_bound")
            self.total_bytes += len(data)
            require(self.total_bytes <= TRANSFER_TOTAL_LIMIT
                    and sum(map(len, active["files"].values())) + len(data) <= active["raw_bytes"],
                    "snapshot_whole_transfer_bound")
            active["files"][name].extend(data)
            return None
        require(kind == "private_snapshot_end", "snapshot_unknown_private_record")
        self.active = None
        files = {name: bytes(data) for name, data in active["files"].items()}
        require(sum(map(len, files.values())) == active["raw_bytes"]
                and all(hashlib.sha256(data).hexdigest() == active["digests"][name]
                        and len(data) == active["versions"][name][3] for name, data in files.items()),
                "snapshot_transfer_bytes_mismatch")
        acquisition = {key: active[key] for key in ("versions", "digests", "raw_bytes", "pair")}
        acquisition["files"] = files
        require(validate_pair(files) == acquisition["pair"], "snapshot_transfer_pairing_mismatch")
        try:
            row = self.reader.query(acquisition, self.thread, expected_workspace="/workspace")
            observation = safe_checkpoint_observation(
                {"thread": row[0], "seq": row[1], "state": json.loads(row[2])}, self.scenario)
            self.validate_current_row(observation, active["root_state"])
            self.thread = row[0]
            self.last = observation
            if self.native_hook is not None:
                self.native_hook(observation, active)
            reply = {"ok": True, "thread": row[0], "checkpoint": observation,
                     "pending_call_sha256": [group["call_sha256"] for group in observation["group_positions"]
                                             if group["result_index"] is None],
                     "durable_in_progress": observation["durable_in_progress"], "suspension_present": row[3] is not None}
        except (SnapshotFailure, Refused) as error:
            # Only the new empty store's setup is a tolerated pre-input state.
            initial_codes = {"checkpoint_initializing", "checkpoint_missing", "owned_input_provenance_missing",
                             "owned_original_input_missing", "owned_input_stamp_missing"}
            if self.thread is not None or str(error) not in initial_codes:
                send({"op": "private_snapshot_reply", "snapshot_id": active["snapshot_id"], "ok": False,
                      "reason": "snapshot_authority_refused"})
                raise Refused(str(error)) from None
            send({"op": "private_snapshot_reply", "snapshot_id": active["snapshot_id"], "ok": False, "reason": str(error)})
            return {"kind": "private_snapshot_initializing", "snapshot_id": active["snapshot_id"],
                    "raw_bytes": active["raw_bytes"], "source_digests": active["digests"]}
        send({"op": "private_snapshot_reply", "snapshot_id": active["snapshot_id"], **reply})
        return {"kind": "private_snapshot_verified", "snapshot_id": active["snapshot_id"],
                "checkpoint_sha256": hashlib.sha256(canonical(observation)).hexdigest(),
                "session_sha256": observation["session_sha256"], "seq": observation["seq"],
                "root_state": active["root_state"], "root_kind": active["root_kind"],
                "source_versions": active["versions"], "source_digests": active["digests"],
                "root_version": active["root_version"], "raw_bytes": active["raw_bytes"],
                "read_attempts": active["read_attempts"], "pair": active["pair"],
                "source_epoch": active["source_epoch"], "shm_commit_authority": False,
                "after_owned_return": active.get("after_owned_return") is True,
                "pending_group_instances": [group for group in observation["group_positions"] if group["result_index"] is None],
                "query_authority": "ordinary_private_verified_copy"}

    def close(self):
        self.active = None
        self.reader.close()
