"""Bounded owned-data regressions for the two independent capture reviews.

Native cases execute only the unchanged marked hash-verified fixture or a
declared trusted pipe-holder control. No candidate source or real provider runs.
Synthetic data checks below do not mint runtime receipts.
"""
import ast
import base64
import errno
from copy import deepcopy
import hashlib
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import sys
import tempfile
import time
from types import SimpleNamespace
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
import grade_long_session as grader
import native_probe_wrapper as native
import owned_tui_collector as collector_source
import owned_tui_files as retained_files
import owned_tui_receipt as receipts
import sqlite_snapshot as snapshots
from owned_tui_snapshot import SnapshotTransport
from owned_tui_client import TuiOwner, ClientFailure
from preflight_continuation import canonical, Refused
from test_long_session import CORPUS, EXTENSION, synthetic_capture
from run_owned_tui import NEGATIVE_PROMPTS, client_bundle, collector_bundle, scenario_actions
import run_owned_tui as runtime_source

ROOT = Path(__file__).resolve().parents[2]
V3 = json.loads((ROOT / "evals/coding/owned-terminal-v3/scenarios.json").read_bytes())


def raw_capture(scenario):
    values = []
    observe = grader.safe_checkpoint_observation
    def capture(value, selected):
        values.append(deepcopy(value))
        return observe(value, selected)
    with mock.patch.object(grader, "safe_checkpoint_observation", side_effect=capture):
        task, metadata, files = synthetic_capture(scenario)
    return task, metadata, files, dict(zip(grader.STAGES, values))


def negative_data(variant):
    scenario = V3["scenarios"][1 if variant == "duplicate-create" else 2]
    task, metadata, files, raw = raw_capture(scenario)
    before = metadata["checkpoints"]["before"]
    value = deepcopy(raw["before"])
    value["seq"] = before["seq"] + 7
    state = value["state"]
    prompt = NEGATIVE_PROMPTS[variant]
    phase = "duplicate" if variant == "duplicate-create" else "reread"
    call = scenario_actions(task, scenario)[phase][0]
    output = "Traceback: FileExistsError: create-only refused\n[exit code 1]" if phase == "duplicate" else "changed after the original observation\n"
    input_index = len(state["messages"])
    state["messages"].extend([{"role": "user", "content": prompt},
        {"role": "assistant", "content": "", "tool_calls": [call]},
        {"role": "tool", "tool_call_id": call["id"], "content": output, "is_error": False}])
    context = state["custom"]["_craftsman_context_v1"]
    revision = context["last_input"]["revision"] + 1
    digest = hashlib.sha256(prompt.encode()).hexdigest()
    context["sequence"] = revision
    context["last_input"] = {"revision": revision, "digest": list(bytes.fromhex(digest))}
    context["updates"] = [*context["updates"], {"event": revision, "digest": digest, "original_bytes": len(prompt.encode())}][-4:]
    after = grader.safe_checkpoint_observation(value, scenario)
    effect = hashlib.sha256(files[scenario["effect_file"]]).hexdigest()
    capture = {"kind": "negative_external_observation_capture", "variant": variant,
        "checkpoints": {"before": before, "after": after},
        "owned_input_receipts": [*metadata["owned_input_receipts"][:12],
            {"sha256": digest, "revision": revision, "checkpoint_seq": after["seq"],
             "session_sha256": after["session_sha256"], "scope_sha256": after["owned_input"]["scope_sha256"],
             "message_index": input_index, "original_bytes": len(prompt.encode()), "source": "actual_owned_context_stamp"}],
        "effect_sha256_by_stage": {"before": effect, "after": effect}}
    condition = "recorded_file_exists_refusal" if phase == "duplicate" else "recorded_current_probe_read"
    call_hash = hashlib.sha256(canonical(call)).hexdigest()
    fixture = [{"kind": "fixture_request_observed", "ordinal": ordinal, "phase": phase, "phase_index": index}
               for index, ordinal in enumerate((14, 15))]
    fixture.append({"kind": "fixture_negative_result_verified", "ordinal": 15, "phase": phase, "phase_index": 1,
        "call_sha256": call_hash, "output_sha256": hashlib.sha256(output.encode()).hexdigest(), "condition": condition})
    bindings = {"negative_conditions": {variant: {"input_sha256": digest, "call_sha256": call_hash,
        "effect_file": scenario["effect_file"], "phase": phase, "condition": condition}}}
    manifest = {name: {"bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()} for name, data in files.items()}
    return capture, fixture, bindings, manifest


class NegativeIssuanceBoundaries(unittest.TestCase):
    def test_both_lawful_new_negative_instances_pass_the_scoped_gate(self):
        for variant in NEGATIVE_PROMPTS:
            with self.subTest(variant=variant):
                data = negative_data(variant)
                facts = receipts.validate_negative_condition(*data)
                self.assertEqual(facts["condition"], data[2]["negative_conditions"][variant]["condition"])

    def test_missing_wrong_stale_relocated_results_and_count_deltas_refuse(self):
        for variant in NEGATIVE_PROMPTS:
            for change in ("missing_result", "wrong_call", "stale_group", "relocated_group", "missing_observation",
                           "wrong_output", "stale_ordinal", "wrong_condition", "unchanged_count",
                           "stale_input", "changed_effect"):
                capture, fixture, bindings, files = negative_data(variant)
                after = capture["checkpoints"]["after"]
                group = after["group_positions"][-1]
                if change == "missing_result":
                    group.update(result_index=None, result_sha256=None, result_content_sha256=None, is_error=None)
                elif change == "wrong_call":
                    group["call_sha256"] = "1" * 64
                elif change == "stale_group":
                    capture["checkpoints"]["before"]["group_positions"][0]["call_sha256"] = group["call_sha256"]
                elif change == "relocated_group":
                    group["assistant_index"] = 1
                elif change == "missing_observation":
                    fixture.pop()
                elif change == "wrong_output":
                    fixture[-1]["output_sha256"] = "2" * 64
                elif change == "stale_ordinal":
                    fixture[-1]["ordinal"] = 13
                elif change == "wrong_condition":
                    fixture[-1]["condition"] = "label_only"
                elif change == "unchanged_count":
                    field = "selected_once_command_count" if variant == "duplicate-create" else "observed_direct_probe_reads"
                    after[field] = capture["checkpoints"]["before"][field]
                elif change == "stale_input":
                    capture["owned_input_receipts"][-1]["revision"] -= 1
                else:
                    capture["effect_sha256_by_stage"]["after"] = "3" * 64
                with self.subTest(variant=variant, change=change), self.assertRaises(Refused):
                    receipts.validate_negative_condition(capture, fixture, bindings, files)

    def test_real_issuer_completion_validator_rejects_label_only_history_before_docker(self):
        for variant in NEGATIVE_PROMPTS:
            capture, fixture, bindings, files = negative_data(variant)
            fixture.pop()
            history = [{"kind": "checkpoint_captured", "stage": stage, "checkpoint": value,
                        "root_state": "done", "durable_in_progress": False,
                        "after_owned_return": stage == "after"} for stage, value in capture["checkpoints"].items()]
            returned = {"kind": "owner_returned", "child_exit": 0, "keeper_exit": 0, "modes_restored": True, "root_state": "done"}
            history.insert(1, returned)
            history += [{"kind": "private_snapshot_verified", "checkpoint_sha256": hashlib.sha256(canonical(value)).hexdigest()}
                        for value in capture["checkpoints"].values()]
            history.append({"kind": "client_finished", "owned_input_receipts": capture["owned_input_receipts"]})
            issuer = receipts.DockerRunIssuer.__new__(receipts.DockerRunIssuer)
            issuer.client = SimpleNamespace(process=SimpleNamespace(poll=lambda: 0), history=history)
            issuer.fixture = SimpleNamespace(history=fixture)
            issuer.gateway = SimpleNamespace(history=[])
            issuer.bindings = bindings
            with self.subTest(variant=variant), mock.patch.object(receipts, "require_container_absent", side_effect=AssertionError("must reject before Docker")):
                with self.assertRaisesRegex(Refused, "new_negative_provider_result_not_observed"):
                    issuer.validate_completion(capture, [], fixture, [returned], files)


@unittest.skipUnless(os.name == "posix", "The owned PTY/retained-fd driver refuses non-POSIX; no platform claim")
class RetainedWriteBoundaries(unittest.TestCase):
    def test_normal_owned_write_uses_exact_retained_leaf(self):
        with tempfile.TemporaryDirectory(prefix="owned-leaf-positive-") as tmp:
            root = Path(tmp)
            leaf = root / "probe_input.txt"
            leaf.write_bytes(b"original")
            fd = os.open(root, os.O_RDONLY | os.O_DIRECTORY)
            try:
                retained_files.write_retained_leaf(fd, leaf.name, b"original", b"changed", named_root=root)
                self.assertEqual(leaf.read_bytes(), b"changed")
            finally:
                os.close(fd)

    def test_link_or_leaf_replacement_during_open_does_not_touch_external_sentinel(self):
        for mode in ("symlink", "leaf"):
            with tempfile.TemporaryDirectory(prefix="owned-leaf-race-") as tmp:
                parent = Path(tmp)
                root = parent / "task"
                root.mkdir()
                leaf = root / "probe_input.txt"
                leaf.write_bytes(b"original")
                sentinel = parent / "outside-sentinel"
                sentinel.write_bytes(b"untouched")
                fd = os.open(root, os.O_RDONLY | os.O_DIRECTORY)
                actual_open = retained_files.os.open
                triggered = [False]
                def race(name, flags, *args, **kwargs):
                    if name == leaf.name and not triggered[0]:
                        triggered[0] = True
                        leaf.unlink()
                        if mode == "symlink":
                            leaf.symlink_to(sentinel)
                        else:
                            leaf.write_bytes(b"foreign!")
                    return actual_open(name, flags, *args, **kwargs)
                try:
                    with self.subTest(mode=mode), mock.patch.object(retained_files.os, "open", side_effect=race):
                        with self.assertRaises((Refused, OSError)):
                            retained_files.write_retained_leaf(fd, leaf.name, b"original", b"changed", named_root=root)
                    self.assertEqual(sentinel.read_bytes(), b"untouched")
                    if mode == "leaf":
                        self.assertEqual(leaf.read_bytes(), b"foreign!")
                finally:
                    os.close(fd)

    def test_root_replacement_at_effect_window_is_refused_without_redirected_io(self):
        with tempfile.TemporaryDirectory(prefix="owned-root-race-") as tmp:
            parent = Path(tmp)
            root = parent / "task"
            root.mkdir()
            (root / "probe_input.txt").write_bytes(b"original")
            sentinel = parent / "outside-sentinel"
            sentinel.write_bytes(b"untouched")
            fd = os.open(root, os.O_RDONLY | os.O_DIRECTORY)
            actual_write = retained_files.os.write
            changed = [False]
            def race(descriptor, value):
                if not changed[0]:
                    changed[0] = True
                    root.rename(parent / "old-task")
                    root.mkdir()
                    (root / "probe_input.txt").symlink_to(sentinel)
                return actual_write(descriptor, value)
            try:
                with mock.patch.object(retained_files.os, "write", side_effect=race), self.assertRaisesRegex(Refused, "root_rebound"):
                    retained_files.write_retained_leaf(fd, "probe_input.txt", b"original", b"changed", named_root=root)
                self.assertEqual(sentinel.read_bytes(), b"untouched")
                self.assertEqual((parent / "old-task/probe_input.txt").read_bytes(), b"changed")
            finally:
                os.close(fd)

    def test_changed_original_content_or_hardlink_refuses_before_write(self):
        with tempfile.TemporaryDirectory(prefix="owned-write-admission-") as tmp:
            root = Path(tmp)
            leaf = root / "probe_input.txt"
            leaf.write_bytes(b"foreign!")
            fd = os.open(root, os.O_RDONLY | os.O_DIRECTORY)
            try:
                with self.assertRaises(Refused):
                    retained_files.write_retained_leaf(fd, leaf.name, b"original", b"changed", named_root=root)
                self.assertEqual(leaf.read_bytes(), b"foreign!")
                os.link(leaf, root / "second-link")
                with self.assertRaises(Refused):
                    retained_files.write_retained_leaf(fd, leaf.name, b"foreign!", b"changed", named_root=root)
                self.assertEqual(leaf.read_bytes(), b"foreign!")
            finally:
                os.close(fd)


@unittest.skipUnless(os.name == "posix", "Retained source snapshots require POSIX owned fds")
class SqliteSnapshotBoundaries(unittest.TestCase):
    def prepare(self, root, thread="owned-thread", first=1, second=2):
        root.mkdir()
        path = root / "threads.db"
        conn = sqlite3.connect(path, timeout=5)
        conn.execute("PRAGMA journal_mode=WAL")
        conn.execute("PRAGMA wal_autocheckpoint=0")  # fixture-only, never live configuration
        conn.execute("PRAGMA user_version=1")
        conn.execute("CREATE TABLE threads(id TEXT PRIMARY KEY, workspace TEXT)")
        conn.execute("CREATE TABLE checkpoints(thread_id TEXT,seq INTEGER,state_json TEXT,suspension_json TEXT,PRIMARY KEY(thread_id,seq))")
        conn.execute("INSERT INTO threads VALUES(?, '/workspace')", (thread,))
        conn.execute("INSERT INTO checkpoints VALUES(?,?,?,NULL)", (thread, first, '{"input":"first"}'))
        conn.commit()
        conn.execute("PRAGMA wal_checkpoint(TRUNCATE)")
        conn.execute("INSERT INTO checkpoints VALUES(?,?,?,NULL)", (thread, second, '{"input":"current"}'))
        conn.commit()
        directory = os.open(root, os.O_RDONLY | os.O_DIRECTORY)
        database = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
        return conn, directory, database

    def test_actual_committed_wal_is_recovered_and_main_only_old_row_refuses_current_step(self):
        with tempfile.TemporaryDirectory(prefix="owned-wal-recovery-") as tmp:
            parent = Path(tmp)
            source = parent / "task"
            conn, directory, database = self.prepare(source)
            reader = snapshots.PrivateReader(parent / "private", [source])
            try:
                captured = snapshots.acquire(directory, database, named_root=source)
                before = dict(captured["digests"])
                row = reader.query(captured, "owned-thread", "/workspace", 2)
                self.assertEqual(row[1], 2)
                self.assertGreater(captured["pair"]["wal_frames"], 0)
                self.assertEqual(captured["digests"], before)
                main_only = {"files": {"threads.db": captured["files"]["threads.db"]},
                    "digests": {"threads.db": captured["digests"]["threads.db"]}}
                main_only["pair"] = snapshots.validate_pair(main_only["files"])
                with self.assertRaisesRegex(snapshots.SnapshotFailure, "current_step_sequence"):
                    reader.query(main_only, "owned-thread", "/workspace", 2)
                self.assertEqual(reader.query(main_only, "owned-thread", "/workspace", 1)[1], 1)
            finally:
                reader.close()
                os.close(database)
                os.close(directory)
                conn.close()

    def test_connection_opens_private_copy_and_live_path_swap_cannot_select_foreign_row(self):
        with tempfile.TemporaryDirectory(prefix="owned-private-query-") as tmp:
            parent = Path(tmp)
            source = parent / "task"
            conn, directory, database = self.prepare(source)
            reader = snapshots.PrivateReader(parent / "private", [source])
            actual_connect = sqlite3.connect
            try:
                captured = snapshots.acquire(directory, database, named_root=source)
                foreign = parent / "foreign.db"
                donor = actual_connect(foreign)
                donor.execute("CREATE TABLE checkpoints(thread_id TEXT,seq INTEGER,state_json TEXT,suspension_json TEXT)")
                donor.execute("PRAGMA user_version=1")
                donor.execute("INSERT INTO checkpoints VALUES('foreign-thread',987,'{}',NULL)")
                donor.commit()
                donor.close()
                def swap(location, *args, **kwargs):
                    self.assertIn(str(reader.parent), location)
                    self.assertNotIn(str(source), location)
                    self.assertNotIn("immutable", location)
                    live = source / "threads.db"
                    live.rename(source / "retained-original")
                    live.write_bytes(foreign.read_bytes())
                    try:
                        return actual_connect(location, *args, **kwargs)
                    finally:
                        live.unlink()
                        (source / "retained-original").rename(live)
                with mock.patch.object(snapshots.sqlite3, "connect", side_effect=swap):
                    row = reader.query(captured, "owned-thread", "/workspace", 2)
                self.assertEqual(row[0:2], ("owned-thread", 2))
            finally:
                reader.close()
                os.close(database)
                os.close(directory)
                conn.close()

    def test_commit_after_first_member_read_refuses_whole_acquisition(self):
        with tempfile.TemporaryDirectory(prefix="owned-snapshot-commit-race-") as tmp:
            parent = Path(tmp)
            source = parent / "task"
            conn, directory, database = self.prepare(source)
            actual_read = os.read
            wal_inode = (source / "threads.db-wal").stat().st_ino
            changed = [False]
            def race(fd, count):
                if os.fstat(fd).st_ino == wal_inode and not changed[0]:
                    changed[0] = True
                    conn.execute("INSERT INTO checkpoints VALUES('owned-thread',3,'{}',NULL)")
                    conn.commit()
                return actual_read(fd, count)
            try:
                with mock.patch.object(snapshots.os, "read", side_effect=race), self.assertRaises(snapshots.SnapshotFailure):
                    snapshots.acquire(directory, database, named_root=source)
                self.assertTrue(changed[0])
            finally:
                os.close(database)
                os.close(directory)
                conn.close()

    def test_stable_truncation_salt_checksum_and_uncommitted_tail_are_incomplete(self):
        with tempfile.TemporaryDirectory(prefix="owned-wal-negative-pairings-") as tmp:
            parent = Path(tmp)
            source = parent / "task"
            conn, directory, database = self.prepare(source)
            try:
                captured = snapshots.acquire(directory, database, named_root=source)
                wal = captured["files"]["threads.db-wal"]
                page = int.from_bytes(wal[8:12], "big")
                for kind in ("truncated", "salt", "checksum", "uncommitted"):
                    files = dict(captured["files"])
                    data = bytearray(wal)
                    if kind == "truncated":
                        data.pop()
                    elif kind == "salt":
                        data[40] ^= 1
                    elif kind == "checksum":
                        data[60] ^= 1
                    else:
                        frame_size = page + 24
                        data[len(data) - frame_size + 4:len(data) - frame_size + 8] = b"\0" * 4
                        rolling = snapshots.checksum(data[:24], "big" if data[3] & 1 else "little")
                        for start in range(32, len(data), frame_size):
                            frame = data[start:start + frame_size]
                            rolling = snapshots.checksum(frame[:8] + frame[24:], "big" if data[3] & 1 else "little", rolling)
                            data[start + 16:start + 24] = b"".join(value.to_bytes(4, "big") for value in rolling)
                    files["threads.db-wal"] = bytes(data)
                    with self.subTest(kind=kind), self.assertRaises(snapshots.SnapshotFailure):
                        snapshots.validate_pair(files)
            finally:
                os.close(database)
                os.close(directory)
                conn.close()

    def test_valid_foreign_wal_can_recover_but_current_step_expectation_refuses(self):
        with tempfile.TemporaryDirectory(prefix="owned-foreign-wal-") as tmp:
            parent = Path(tmp)
            left = parent / "left-task"
            right = parent / "right-task"
            lconn, ldir, ldb = self.prepare(left)
            rconn, rdir, rdb = self.prepare(right, second=99)
            reader = snapshots.PrivateReader(parent / "private", [left, right])
            try:
                left_data = snapshots.acquire(ldir, ldb, named_root=left)
                right_data = snapshots.acquire(rdir, rdb, named_root=right)
                foreign = dict(left_data)
                foreign["files"] = dict(left_data["files"])
                foreign["files"]["threads.db-wal"] = right_data["files"]["threads.db-wal"]
                foreign["digests"] = {name: hashlib.sha256(data).hexdigest() for name, data in foreign["files"].items()}
                foreign["pair"] = snapshots.validate_pair(foreign["files"])
                self.assertEqual(reader.query(foreign, "owned-thread", "/workspace")[1], 99)
                with self.assertRaisesRegex(snapshots.SnapshotFailure, "current_step_sequence"):
                    reader.query(foreign, "owned-thread", "/workspace", 2)
            finally:
                reader.close()
                for fd in (ldb, ldir, rdb, rdir):
                    os.close(fd)
                lconn.close()
                rconn.close()

    def test_root_and_optional_member_rebinding_refuse_without_descriptor_leaks(self):
        with tempfile.TemporaryDirectory(prefix="owned-snapshot-rebinding-") as tmp:
            parent = Path(tmp)
            source = parent / "task"
            conn, directory, database = self.prepare(source)
            opened = []
            actual_open = os.open
            def track(name, flags, *args, **kwargs):
                fd = actual_open(name, flags, *args, **kwargs)
                opened.append(fd)
                return fd
            try:
                old = source / "threads.db-wal"
                renamed = source / "old-wal"
                old.rename(renamed)
                old.symlink_to(renamed)
                with mock.patch.object(snapshots.os, "open", side_effect=track), self.assertRaises(snapshots.SnapshotFailure):
                    snapshots.acquire(directory, database, named_root=source)
                for fd in opened:
                    with self.assertRaises(OSError):
                        os.fstat(fd)
                old.unlink()
                renamed.rename(old)
                source.rename(parent / "original-task")
                source.mkdir()
                with self.assertRaisesRegex(snapshots.SnapshotFailure, "root_rebound"):
                    snapshots.acquire(directory, database, named_root=source)
            finally:
                os.close(database)
                os.close(directory)
                conn.close()

    def test_candidate_private_staging_and_wrong_workspace_are_refused(self):
        with tempfile.TemporaryDirectory(prefix="owned-private-admission-") as tmp:
            parent = Path(tmp)
            source = parent / "task"
            conn, directory, database = self.prepare(source)
            try:
                with self.assertRaisesRegex(snapshots.SnapshotFailure, "candidate_accessible"):
                    snapshots.PrivateReader(source / "private", [source])
                reader = snapshots.PrivateReader(parent / "private", [source])
                try:
                    captured = snapshots.acquire(directory, database, named_root=source)
                    with self.assertRaisesRegex(snapshots.SnapshotFailure, "workspace_mismatch"):
                        reader.query(captured, "owned-thread", "/foreign", 2)
                finally:
                    reader.close()
            finally:
                os.close(database)
                os.close(directory)
                conn.close()

    def test_changed_shm_is_not_committed_authority_and_new_wal_generation_is_bound(self):
        with tempfile.TemporaryDirectory(prefix="owned-wal-generation-") as tmp:
            parent = Path(tmp)
            source = parent / "task"
            conn, directory, database = self.prepare(source)
            reader = snapshots.PrivateReader(parent / "private", [source])
            try:
                captured = snapshots.acquire(directory, database, named_root=source)
                altered = dict(captured)
                altered["files"] = dict(captured["files"])
                altered["files"]["threads.db-shm"] = b"untrusted lock/index claims".ljust(len(captured["files"]["threads.db-shm"]), b"x")
                altered["digests"] = {name: hashlib.sha256(data).hexdigest() for name, data in altered["files"].items()}
                # The copy never imports SHM and does not use its read marks to
                # select a commit; ordinary recovery still returns actual WAL2.
                self.assertEqual(reader.query(altered, "owned-thread", "/workspace", 2)[1], 2)
                old_generation = captured["pair"]["wal_generation_sha256"]
                conn.execute("PRAGMA wal_checkpoint(TRUNCATE)")
                conn.execute("INSERT INTO checkpoints VALUES('owned-thread',3,'{}',NULL)")
                conn.commit()
                fresh = snapshots.acquire(directory, database, named_root=source)
                self.assertNotEqual(old_generation, fresh["pair"]["wal_generation_sha256"])
                self.assertEqual(reader.query(fresh, "owned-thread", "/workspace", 3)[1], 3)
            finally:
                reader.close()
                os.close(database)
                os.close(directory)
                conn.close()


@unittest.skipUnless(os.name == "posix", "Owned native fixtures refuse non-POSIX")
class NativeCollectorBoundaries(unittest.TestCase):
    def prepare(self, parent):
        root, private = parent / "task", parent / "collector-private"
        root.mkdir()
        private.mkdir()
        for name, text in EXTENSION["scenarios"][2]["fixture_files"].items():
            (root / name).write_bytes(text.encode())
        rfd = os.open(root, os.O_RDONLY | os.O_DIRECTORY)
        pfd = os.open(private, os.O_RDONLY | os.O_DIRECTORY)
        return root, private, rfd, pfd

    def controller(self, root, rfd, pfd, events):
        boot = {"source_commit": collector_source.SOURCE, "image_id": collector_source.IMAGE,
            "binary_sha256": collector_source.BINARY, "scenario_sha256": "1" * 64, "run_sha256": "2" * 64,
            "native_wrapper_sha256": hashlib.sha256(Path(native.__file__).read_bytes()).hexdigest()}
        owner = collector_source.Collector(boot, rfd, pfd, events.append, cwd=str(root))
        arm = {"op": "arm", "request_ordinal": 2, "request_sha256": "3" * 64, "call_sha256": owner.call_sha256,
            "snapshot_id": "4" * 32, "session_sha256": "5" * 64, "checkpoint_seq": 10, "assistant_index": 5, "call_ordinal": 0}
        owner.arm(arm)
        owner.allow_fixture({"op": "allow_fixture", "call_sha256": owner.call_sha256})
        return owner, boot, arm

    def test_separate_actual_child_wait_eof_and_source_ignore_candidate_witness(self):
        with tempfile.TemporaryDirectory(prefix="owned-collector-positive-") as tmp:
            root, private, rfd, pfd = self.prepare(Path(tmp))
            fake = b'{"complete":true,"child_source_sha256":"' + b"0" * 64 + b'"}'
            (root / "native_probe_witness.json").write_bytes(fake)
            events, forwarded = [], []
            try:
                owner, boot, arm = self.controller(root, rfd, pfd, events)
                def forward(data):
                    forwarded.append(json.loads(data))
                    return len(data)
                owner.trigger(forward)
                complete = [item for item in events if item["kind"] == "native_child_completed"]
                self.assertEqual(len(complete), 1)
                facts = complete[0]
                self.assertEqual(facts["child_source_sha256"], native.CHILD_SOURCE_SHA256)
                self.assertTrue(facts["actual_wait_returned"])
                self.assertTrue(facts["source_instance_binding_acknowledged"])
                self.assertEqual(facts["actual_child_exit"], 7)
                self.assertEqual(facts["classification"], "independently_requested_fixed_fixture_child")
                self.assertFalse(facts["outer_toolmeta_observed"])
                self.assertFalse(facts["receiver_exactly_once_verified"])
                self.assertEqual((root / "native_probe_witness.json").read_bytes(), fake)
                self.assertEqual((root / "probe_calls.txt").read_bytes(), b"invoked\n")
                self.assertEqual(json.loads((private / "native_probe_witness.json").read_bytes()), facts)
                with self.assertRaises(ValueError):
                    owner.trigger(forward)
                self.assertEqual(len(forwarded), 1)
                self.assertEqual((root / "probe_calls.txt").read_bytes(), b"invoked\n")
            finally:
                os.close(rfd)
                os.close(pfd)

    def test_postverification_changed_source_refuses_before_compile_and_final_restore_cannot_hide_it(self):
        with tempfile.TemporaryDirectory(prefix="owned-native-source-race-") as tmp:
            root, private, rfd, pfd = self.prepare(Path(tmp))
            original = (root / "probe_once.py").read_bytes()
            actual_popen = subprocess.Popen
            def race(*args, **kwargs):
                (root / "probe_once.py").write_bytes(b"# raced bytes\n" + original)
                return actual_popen(*args, **kwargs)
            try:
                with mock.patch.object(native.subprocess, "Popen", side_effect=race):
                    with self.assertRaises(native.NativeFailure) as refused:
                        native.observe(rfd, pfd, cwd=str(root))
                (root / "probe_once.py").write_bytes(original)
                self.assertFalse(refused.exception.facts["complete"])
                self.assertFalse(refused.exception.facts["verified_buffer_acknowledged"])
                self.assertEqual(refused.exception.facts["cleanup"]["actual_child_exit"], 125)
                self.assertFalse((root / "probe_calls.txt").exists())
                self.assertEqual((private / "native_probe_witness.json").read_bytes(), b"")
                self.assertEqual(hashlib.sha256((root / "probe_once.py").read_bytes()).hexdigest(), native.CHILD_SOURCE_SHA256)
            finally:
                os.close(rfd)
                os.close(pfd)

    def test_changed_transmitted_buffer_is_not_compiled_even_with_unchanged_final_source(self):
        with tempfile.TemporaryDirectory(prefix="owned-native-buffer-race-") as tmp:
            root, private, rfd, pfd = self.prepare(Path(tmp))
            actual_popen, actual_write = subprocess.Popen, os.write
            input_fd = [None]
            def spawn(*args, **kwargs):
                child = actual_popen(*args, **kwargs)
                input_fd[0] = child.stdin.fileno()
                return child
            def corrupt(fd, data):
                return actual_write(fd, b"x" * len(data) if fd == input_fd[0] else data)
            try:
                with mock.patch.object(native.subprocess, "Popen", side_effect=spawn), mock.patch.object(native.os, "write", side_effect=corrupt):
                    with self.assertRaises(native.NativeFailure) as refused:
                        native.observe(rfd, pfd, cwd=str(root))
                self.assertFalse(refused.exception.facts["complete"])
                self.assertEqual(refused.exception.facts["cleanup"]["actual_child_exit"], 125)
                self.assertFalse((root / "probe_calls.txt").exists())
                self.assertEqual((private / "native_probe_witness.json").read_bytes(), b"")
            finally:
                os.close(rfd)
                os.close(pfd)

    def test_actual_stream_overflow_is_bounded_incomplete_and_not_replayed(self):
        with tempfile.TemporaryDirectory(prefix="owned-native-overflow-") as tmp:
            root, private, rfd, pfd = self.prepare(Path(tmp))
            (root / "probe_input.txt").write_bytes(b"x" * 100000)
            try:
                with self.assertRaises(native.NativeFailure) as refused:
                    native.observe(rfd, pfd, cwd=str(root))
                self.assertIn("stream_overflow", str(refused.exception))
                self.assertLessEqual(refused.exception.facts["retained_stdout_bytes"], 65536)
                self.assertLessEqual(refused.exception.facts["retained_stderr_bytes"], 65536)
                self.assertFalse(refused.exception.facts["complete"])
                with self.assertRaises(FileExistsError):
                    native.observe(rfd, pfd, cwd=str(root))
                self.assertEqual((root / "probe_calls.txt").read_bytes(), b"invoked\n")
                self.assertEqual((private / "native_probe_witness.json").read_bytes(), b"")
            finally:
                os.close(rfd)
                os.close(pfd)

    def test_partial_forwarding_closes_instance_with_no_complete_witness_or_retry(self):
        with tempfile.TemporaryDirectory(prefix="owned-native-forward-refusal-") as tmp:
            root, private, rfd, pfd = self.prepare(Path(tmp))
            events = []
            try:
                owner, _, _ = self.controller(root, rfd, pfd, events)
                with self.assertRaisesRegex(ValueError, "forward_partial"):
                    owner.trigger(lambda payload: len(payload) - 1)
                self.assertEqual([value["kind"] for value in events][-1], "native_child_incomplete")
                self.assertFalse(any(value["kind"] == "native_child_completed" for value in events))
                self.assertEqual((private / "native_probe_witness.json").read_bytes(), b"")
                with self.assertRaises(ValueError):
                    owner.trigger(lambda payload: len(payload))
                self.assertEqual((root / "probe_calls.txt").read_bytes(), b"invoked\n")
            finally:
                os.close(rfd)
                os.close(pfd)

    def test_true_child_exit_with_inherited_pipes_open_is_incomplete_at_original_twenty_second_bound(self):
        with tempfile.TemporaryDirectory(prefix="owned-native-eof-refusal-") as tmp:
            root, private, rfd, pfd = self.prepare(Path(tmp))
            helper = Path(tmp) / "owned-pipe-holder.py"
            helper.write_text("import sys\nif sys.argv[1:] != ['--owned-native-pipe-holder-v3']: raise SystemExit(0)\nimport time\ntime.sleep(20.2)\n")
            # A trusted control acknowledges bytes and exits7, while a
            # separately held marked helper inherits both writer handles.
            # No altered probe/candidate source is compiled or executed.
            def held_loader(source, acknowledgement, expected_version, bound_ack):
                return ("import os,sys; sys.stdin.buffer.read(65537); "
                    "os.write(" + str(acknowledgement) + ",bytes.fromhex(" + repr(native.CHILD_SOURCE_SHA256 + bound_ack) + ")); "
                    "os.close(" + str(acknowledgement) + "); "
                    "sys.exit(7)")
            actual_popen = subprocess.Popen
            stdout_read, stdout_write = os.pipe()
            stderr_read, stderr_write = os.pipe()
            holder = actual_popen([sys.executable, "-I", "-B", str(helper), "--owned-native-pipe-holder-v3"],
                stdin=subprocess.DEVNULL, stdout=stdout_write, stderr=stderr_write, env={"PATH": os.defpath})
            owned_readers = []
            def spawn(argv, **kwargs):
                kwargs.update(stdout=stdout_write, stderr=stderr_write)
                child = actual_popen(argv, **kwargs)
                child.stdout = os.fdopen(stdout_read, "rb")
                child.stderr = os.fdopen(stderr_read, "rb")
                owned_readers.extend([child.stdout, child.stderr])
                os.close(stdout_write)
                os.close(stderr_write)
                return child
            started = time.monotonic()
            try:
                with mock.patch.object(native, "child_loader", side_effect=held_loader), mock.patch.object(native.subprocess, "Popen", side_effect=spawn):
                    with self.assertRaises(native.NativeFailure) as refused:
                        native.observe(rfd, pfd, cwd=str(root))
                self.assertGreaterEqual(time.monotonic() - started, 20)
                self.assertFalse(refused.exception.facts["complete"])
                self.assertTrue(refused.exception.facts["verified_buffer_acknowledged"])
                self.assertEqual(refused.exception.facts["cleanup"]["actual_child_exit"], 7)
                self.assertFalse(refused.exception.facts["stdout_eof"])
                self.assertFalse(refused.exception.facts["stderr_eof"])
                self.assertEqual((private / "native_probe_witness.json").read_bytes(), b"")
                self.assertFalse((root / "probe_calls.txt").exists())
                self.assertEqual(holder.wait(timeout=5), 0)
            finally:
                if holder.poll() is None:
                    holder.kill()
                    holder.wait(timeout=5)
                for stream in owned_readers:
                    stream.close()
                os.close(rfd)
                os.close(pfd)

    def test_foreign_instance_ack_is_incomplete_even_after_real_wait_and_eof(self):
        with tempfile.TemporaryDirectory(prefix="owned-native-foreign-ack-") as tmp:
            root, private, rfd, pfd = self.prepare(Path(tmp))
            def foreign_loader(source, acknowledgement, expected_version, bound_ack):
                return ("import os,sys; sys.stdin.buffer.read(65537); os.write(" + str(acknowledgement) +
                    ",bytes.fromhex(" + repr(native.CHILD_SOURCE_SHA256 + "0" * 64) + ")); os.close(" + str(acknowledgement) + "); sys.exit(7)")
            try:
                with mock.patch.object(native, "child_loader", side_effect=foreign_loader):
                    with self.assertRaises(native.NativeFailure) as refused:
                        native.observe(rfd, pfd, cwd=str(root))
                self.assertFalse(refused.exception.facts["complete"])
                self.assertTrue(refused.exception.facts["actual_wait_returned"])
                self.assertTrue(refused.exception.facts["stdout_eof"])
                self.assertTrue(refused.exception.facts["stderr_eof"])
                self.assertFalse(refused.exception.facts["verified_buffer_acknowledged"])
                self.assertFalse((root / "probe_calls.txt").exists())
            finally:
                os.close(rfd)
                os.close(pfd)

    def test_bad_source_image_trigger_or_producer_binding_refuses_before_effects(self):
        with tempfile.TemporaryDirectory(prefix="owned-collector-binding-refusal-") as tmp:
            root, private, rfd, pfd = self.prepare(Path(tmp))
            events = []
            try:
                owner, boot, arm = self.controller(root, rfd, pfd, events)
                for change in ("source_commit", "image_id", "native_wrapper_sha256", "extra_code"):
                    candidate = dict(boot)
                    if change == "extra_code":
                        candidate["source"] = "raise SystemExit(0)"
                    else:
                        candidate[change] = "0" * 64
                    with self.subTest(change=change), self.assertRaises(ValueError):
                        collector_source.Collector(candidate, rfd, pfd, events.append)
                for change in ("call_sha256", "request_sha256", "extra_path", "duplicate"):
                    candidate = dict(arm)
                    if change == "extra_path":
                        candidate["path"] = "/outside"
                    elif change != "duplicate":
                        candidate[change] = "0" * 64
                    with self.subTest(change=change), self.assertRaises(ValueError):
                        owner.arm(candidate)
                self.assertFalse((root / "probe_calls.txt").exists())
            finally:
                os.close(rfd)
                os.close(pfd)

    def test_native_complete_gate_rejects_foreign_source_instance_wait_eof_or_scope(self):
        with tempfile.TemporaryDirectory(prefix="owned-native-receipt-rejections-") as tmp:
            root, private, rfd, pfd = self.prepare(Path(tmp))
            events = []
            try:
                owner, boot, arm = self.controller(root, rfd, pfd, events)
                owner.trigger(lambda payload: len(payload))
                facts = next(value for value in events if value["kind"] == "native_child_completed")
                intent = next(value["binding"] for value in events if value["kind"] == "collector_intent_reserved")
                result_hash = hashlib.sha256(b"synthetic saved relay result").hexdigest()
                bindings = {"collector_bootstrap": boot, "native_expected_call_sha256": owner.call_sha256,
                    "native_wrapper_sha256": boot["native_wrapper_sha256"],
                    "native_expected_streams": {key: facts[key] for key in ("stdout_bytes", "stderr_bytes", "stdout_sha256", "stderr_sha256")}}
                pending = [{"kind": "private_snapshot_verified", "snapshot_id": arm["snapshot_id"],
                    "root_state": "blocked", "root_kind": "permission", "session_sha256": arm["session_sha256"],
                    "seq": arm["checkpoint_seq"], "pending_group_instances": [{"call_sha256": owner.call_sha256,
                        "assistant_index": arm["assistant_index"], "call_ordinal": 0, "result_index": None}]}]
                fixture = [{"kind": "fixture_request_observed", "phase": "setup", "phase_index": 0,
                    "ordinal": arm["request_ordinal"], "request_sha256": arm["request_sha256"]},
                    {"kind": "fixture_native_relay_result_verified", "call_sha256": owner.call_sha256, "output_sha256": result_hash}]
                capture = {"checkpoints": {"before": {"group_positions": [{"call_sha256": owner.call_sha256,
                    "result_content_sha256": result_hash}]}}}
                # Remaining inputs are explicitly data-only gate controls;
                # no receipt/signing authority is constructed by this test.
                receipts.validate_native_child(facts, intent, bindings, pending, fixture, capture)
                for field, value in (("child_source_sha256", "0" * 64), ("child_instance", "0" * 32),
                        ("actual_wait_returned", False), ("stdout_eof", False), ("stderr_eof", False),
                        ("collector_binding_sha256", "0" * 64), ("producer_source_sha256", "0" * 64),
                        ("local_forward_write_returned", False), ("classification", "outer_runtime_tool_receipt"),
                        ("receiver_exactly_once_verified", True), ("outer_toolmeta_observed", True),
                        ("cleanup", {"needed": False, "actual_wait_returned": False, "actual_child_exit": None})):
                    candidate = deepcopy(facts)
                    candidate[field] = value
                    with self.subTest(field=field), self.assertRaises(Refused):
                        receipts.validate_native_child(candidate, intent, bindings, pending, fixture, capture)
            finally:
                os.close(rfd)
                os.close(pfd)


@unittest.skipUnless(os.name == "posix", "Private snapshot staging requires retained POSIX directories")
class SnapshotChannelBoundaries(unittest.TestCase):
    def fixture(self, parent):
        scenario = EXTENSION["scenarios"][1]
        _, _, _, raw = raw_capture(scenario)
        value = raw["before"]
        source = parent / "task"
        source.mkdir()
        connection = sqlite3.connect(source / "threads.db", timeout=5)
        connection.execute("PRAGMA user_version=1")
        connection.execute("CREATE TABLE threads(id TEXT PRIMARY KEY,workspace TEXT)")
        connection.execute("CREATE TABLE checkpoints(thread_id TEXT,seq INTEGER,state_json TEXT,suspension_json TEXT)")
        connection.execute("INSERT INTO threads VALUES(?, '/workspace')", (value["thread"],))
        connection.execute("INSERT INTO checkpoints VALUES(?,?,?,NULL)", (value["thread"], value["seq"], json.dumps(value["state"])))
        connection.commit()
        connection.close()
        directory = os.open(source, os.O_RDONLY | os.O_DIRECTORY)
        database = os.open(source / "threads.db", os.O_RDONLY | os.O_NOFOLLOW)
        try:
            acquired = snapshots.acquire(directory, database, named_root=source)
        finally:
            os.close(database)
            os.close(directory)
        transport = SnapshotTransport(parent / "private", source, scenario)
        transport.sequence = value["seq"] - 4
        transport.expect_input("update", grader.updates(scenario)[-1])
        identity = "9" * 32
        begin = {"kind": "private_snapshot_begin", "snapshot_id": identity, "root_state": "done", "root_kind": None,
                 "owner_generation": 1, "source_epoch": 1, "thread": None,
                 **{key: data for key, data in acquired.items() if key != "files"}}
        # JSON IPC converts actual tuple versions to lists without changing
        # any integer field; raw bytes remain a separate transient channel.
        begin = json.loads(json.dumps(begin))
        return transport, acquired, begin, identity

    def deliver(self, transport, acquired, begin, identity, replies):
        transport.receive(begin, replies.append)
        for name, data in acquired["files"].items():
            for offset in range(0, len(data), 49152):
                transport.receive({"kind": "private_snapshot_chunk", "snapshot_id": identity, "name": name,
                    "offset": offset, "data": base64.b64encode(data[offset:offset + 49152]).decode()}, replies.append)
        return transport.receive({"kind": "private_snapshot_end", "snapshot_id": identity}, replies.append)

    def test_actual_private_query_reply_contains_metadata_and_receipt_history_has_no_raw_inputs(self):
        with tempfile.TemporaryDirectory(prefix="owned-snapshot-channel-positive-") as tmp:
            transport, acquired, begin, identity = self.fixture(Path(tmp))
            replies = []
            try:
                observation = self.deliver(transport, acquired, begin, identity, replies)
                self.assertEqual(observation["kind"], "private_snapshot_verified")
                self.assertEqual(replies[0]["checkpoint"]["seq"], transport.current["done_seq"])
                self.assertEqual(observation["query_authority"], "ordinary_private_verified_copy")
                self.assertFalse(observation["shm_commit_authority"])
                encoded = json.dumps(observation) + json.dumps(replies)
                for text in (grader.updates(EXTENSION["scenarios"][1])[-1], EXTENSION["scenarios"][1]["setup_prompt"]):
                    self.assertNotIn(text, encoded)
                self.assertNotIn("state_json", encoded)
                self.assertNotIn("data", observation)
                with self.assertRaises(Refused):
                    transport.receive(begin, replies.append)
            finally:
                transport.close()

    def test_wrong_step_source_epoch_presence_order_and_quiescent_stage_refuse(self):
        for change in ("input", "sequence", "epoch", "presence", "stage", "offset", "foreign_instance"):
            with tempfile.TemporaryDirectory(prefix="owned-snapshot-channel-refusal-") as tmp:
                transport, acquired, begin, identity = self.fixture(Path(tmp))
                replies = []
                try:
                    if change == "input":
                        transport.current["input_sha256"] = "0" * 64
                    elif change == "sequence":
                        transport.current["done_seq"] += 1
                    elif change == "epoch":
                        begin["source_epoch"] = 2
                    elif change == "presence":
                        begin["versions"]["threads.db-wal"] = list(begin["versions"]["threads.db"])
                    elif change == "stage":
                        begin["root_state"] = "working"
                    with self.subTest(change=change), self.assertRaises(Refused):
                        if change in {"offset", "foreign_instance"}:
                            transport.receive(begin, replies.append)
                            name, data = next(iter(acquired["files"].items()))
                            transport.receive({"kind": "private_snapshot_chunk", "snapshot_id": "8" * 32 if change == "foreign_instance" else identity,
                                "name": name, "offset": 1 if change == "offset" else 0,
                                "data": base64.b64encode(data[:4096]).decode()}, replies.append)
                        else:
                            self.deliver(transport, acquired, begin, identity, replies)
                    self.assertFalse(any(reply.get("ok") is True for reply in replies))
                finally:
                    transport.close()


class CleanupAndMarkerBoundaries(unittest.TestCase):
    @unittest.skipUnless(os.name == "posix", "Retained descriptor cleanup requires POSIX")
    def test_snapshot_cleanup_attempts_siblings_once_and_preserves_unknown_close_failure(self):
        for phase in ("acquisition", "private_copy"):
            for status in ("after_actual_close", "before_close_unknown"):
                for primary_failure in (False, True):
                    with self.subTest(phase=phase, status=status, primary=primary_failure), \
                            tempfile.TemporaryDirectory(prefix="owned-close-failure-") as temporary:
                        parent = Path(temporary)
                        source = parent / "task"
                        connection, directory, database = SqliteSnapshotBoundaries().prepare(source)
                        acquisition = snapshots.acquire(directory, database, named_root=source)
                        reader = snapshots.PrivateReader(parent / "private", [source])
                        actual_helper, actual_close = snapshots.close_owned_once, os.close
                        owners, attempted = [], []
                        primary = snapshots.SnapshotFailure("injected_primary_snapshot_failure")
                        def closing(values):
                            values = list(values)
                            owners.extend(values)
                            def fault(descriptor):
                                attempted.append(descriptor)
                                if descriptor == values[0][1]:
                                    if status == "after_actual_close":
                                        actual_close(descriptor)
                                    raise OSError(errno.EIO, "injected close outcome unknown")
                                return actual_close(descriptor)
                            # Only owned closes are instrumented; temporary
                            # directory disposal may safely reuse fd numbers.
                            with mock.patch.object(snapshots.os, "close", side_effect=fault):
                                return actual_helper(values)
                        try:
                            with mock.patch.object(snapshots, "close_owned_once", side_effect=closing):
                                with self.assertRaisesRegex(snapshots.SnapshotFailure, "snapshot_cleanup_incomplete") as failed:
                                    if phase == "acquisition":
                                        if primary_failure:
                                            with mock.patch.object(snapshots, "validate_pair", side_effect=primary):
                                                snapshots.acquire(directory, database, named_root=source)
                                        else:
                                            snapshots.acquire(directory, database, named_root=source)
                                    elif primary_failure:
                                        with mock.patch.object(reader, "_query_copy", side_effect=primary):
                                            reader.query(acquisition, "owned-thread", "/workspace", 2)
                                    else:
                                        reader.query(acquisition, "owned-thread", "/workspace", 2)
                            self.assertEqual(len(owners), 3)
                            self.assertEqual(attempted, [descriptor for _, descriptor in owners])
                            self.assertEqual(attempted.count(owners[0][1]), 1)
                            self.assertEqual(failed.exception.cleanup_failures,
                                ({"owner": owners[0][0], "errno": errno.EIO, "outcome": "unknown_not_retried"},))
                            self.assertIs(failed.exception.primary_failure, primary if primary_failure else None)
                            self.assertIs(failed.exception.__cause__, primary if primary_failure else None)
                            for _, descriptor in owners[1:]:
                                with self.assertRaises(OSError):
                                    os.fstat(descriptor)
                            if status == "after_actual_close":
                                with self.assertRaises(OSError):
                                    os.fstat(owners[0][1])
                            else:
                                # Before-close failure leaves the first outcome
                                # uncertain. The tested code must not retry it.
                                os.fstat(owners[0][1])
                            self.assertFalse((parent / "receipt.json").exists())
                        finally:
                            if owners and status == "before_close_unknown":
                                # Independent fixture-owner disposal, after all
                                # no-retry assertions; not a production retry.
                                actual_close(owners[0][1])
                            reader.close()
                            actual_close(database)
                            actual_close(directory)
                            connection.close()

    @unittest.skipUnless(os.name == "posix", "Retained descriptor cleanup requires POSIX")
    def test_private_reader_close_error_retires_owner_without_retry(self):
        with tempfile.TemporaryDirectory(prefix="owned-reader-close-failure-") as temporary:
            parent = Path(temporary)
            source = parent / "task"
            source.mkdir()
            reader = snapshots.PrivateReader(parent / "private", [source])
            descriptor = reader.fd
            actual_close = os.close
            attempts = []
            def close(value):
                attempts.append(value)
                actual_close(value)
                raise OSError(errno.EIO, "injected close error after release")
            with mock.patch.object(snapshots.os, "close", side_effect=close):
                with self.assertRaisesRegex(snapshots.SnapshotFailure, "snapshot_cleanup_incomplete"):
                    reader.close()
                reader.close()
            self.assertEqual(attempts, [descriptor])
            self.assertIsNone(reader.fd)
            with self.assertRaises(OSError):
                os.fstat(descriptor)

    @unittest.skipUnless(os.name == "posix", "Retained directory ownership requires POSIX")
    def test_preparation_failures_close_the_exact_acquired_workspace_descriptor(self):
        scenario = V3["scenarios"][2]
        task = next(value for value in CORPUS["tasks"] if value["id"] == scenario["base_task"])
        source_directory = ROOT / "scripts/coding-eval"
        for failure in ("first_git", "source_copy", "client_bundle", "collector_bundle"):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory(prefix="owned-preparation-failure-") as temporary:
                case = Path(temporary) / "case"
                case.mkdir()
                acquired = []
                actual_open = os.open
                actual_read = Path.read_bytes
                def track(path, flags, *args, **kwargs):
                    descriptor = actual_open(path, flags, *args, **kwargs)
                    if Path(path) == case / "workspace":
                        acquired.append(descriptor)
                    return descriptor
                def read(path):
                    if failure == "source_copy" and path.parent == source_directory:
                        raise OSError("injected source preparation failure")
                    return actual_read(path)
                try:
                    with mock.patch.object(runtime_source.os, "open", side_effect=track), \
                            mock.patch.object(runtime_source.subprocess, "run", return_value=subprocess.CompletedProcess([], 0)) as git_prep, \
                            mock.patch.object(Path, "read_bytes", read), \
                            mock.patch.object(runtime_source, "client_bundle", return_value=case / "trusted-client.py") as client_prep, \
                            mock.patch.object(runtime_source, "collector_bundle", return_value=case / "trusted-collector.py") as collector_prep, \
                            mock.patch.object(runtime_source, "docker") as docker, \
                            mock.patch.object(runtime_source, "JsonProcess") as process, \
                            mock.patch.object(runtime_source.subprocess, "Popen") as popen, \
                            mock.patch.object(runtime_source, "seal_actual_driver") as seal:
                        if failure == "first_git":
                            git_prep.side_effect = RuntimeError("injected first Git preparation failure")
                        elif failure == "client_bundle":
                            client_prep.side_effect = RuntimeError("injected client bundle preparation failure")
                        elif failure == "collector_bundle":
                            collector_prep.side_effect = RuntimeError("injected collector bundle preparation failure")
                        with self.assertRaises((OSError, RuntimeError)):
                            runtime_source.run_case(ROOT, case, task, scenario, {}, collector_source.IMAGE, {}, V3)
                        self.assertEqual(len(acquired), 1)
                        with self.assertRaises(OSError):
                            os.fstat(acquired[0])
                        docker.assert_not_called()
                        process.assert_not_called()
                        popen.assert_not_called()
                        seal.assert_not_called()
                        self.assertFalse((case / "receipt.json").exists())
                        self.assertFalse((case / "capture.json").exists())
                        self.assertFalse((case / "workspace/probe_calls.txt").exists())
                finally:
                    for descriptor in acquired:
                        try:
                            os.close(descriptor)
                        except OSError:
                            pass

    @unittest.skipUnless(os.name == "posix", "Retained directory ownership requires POSIX")
    def test_workspace_owner_preserves_borrowed_return_and_closes_once(self):
        scenario = V3["scenarios"][2]
        task = next(value for value in CORPUS["tasks"] if value["id"] == scenario["base_task"])
        result = object()
        acquired = []
        def borrowed(*arguments):
            descriptor = arguments[-1]
            os.fstat(descriptor)
            acquired.append(descriptor)
            return result
        with tempfile.TemporaryDirectory(prefix="owned-workspace-return-") as temporary:
            case = Path(temporary)
            with mock.patch.object(runtime_source, "_run_case_with_workspace", side_effect=borrowed), \
                    mock.patch.object(runtime_source.os, "close", wraps=os.close) as close:
                self.assertIs(runtime_source.run_case(ROOT, case, task, scenario, {}, collector_source.IMAGE, {}, V3), result)
                self.assertEqual(close.call_args_list, [mock.call(acquired[0])])
            with self.assertRaises(OSError):
                os.fstat(acquired[0])

    def test_released_approval_with_stale_blocked_display_is_not_a_snapshot_stage(self):
        owner = TuiOwner.__new__(TuiOwner)
        owner.facts = SimpleNamespace(state="blocked", kind="permission", records=[{"is_root": True, "state": "blocked"}])
        owner.approval_status_start = 1
        self.assertFalse(owner.checkpoint_stage_available())
        owner.facts.records.append({"is_root": False, "state": "working"})
        self.assertFalse(owner.checkpoint_stage_available())
        owner.facts.records.extend([{"is_root": True, "state": "working"}, {"is_root": True, "state": "blocked"}])
        self.assertTrue(owner.checkpoint_stage_available())
        self.assertIsNone(owner.approval_status_start)

    def test_closed_capture_without_actual_owner_wait_refuses_before_store_query(self):
        owner = TuiOwner.__new__(TuiOwner)
        owner.capture_after_owned_return = True
        owner.last_return = {"child_exit": 0, "keeper_exit": 0, "modes_restored": True, "root_state": "done"}
        owner.process = SimpleNamespace(poll=lambda: None)
        owner.store = SimpleNamespace(checkpoint=mock.Mock(side_effect=AssertionError("must not query before a real return")))
        with self.assertRaisesRegex(ClientFailure, "actual_completed_owned_return_required"):
            owner.closed_capture()
        owner.store.checkpoint.assert_not_called()

    def test_nonzero_query_is_not_confirmed_container_absence(self):
        for code, output, accepted in ((1, b"", False), (0, b"actual-owned-container\n", False), (0, b"", True)):
            result = subprocess.CompletedProcess(["docker"], code, output, b"")
            with self.subTest(code=code, output=bool(output)), mock.patch.object(receipts.subprocess, "run", return_value=result):
                if accepted:
                    self.assertEqual(receipts.require_container_absent("owned-current-id")["matching_container_count"], 0)
                else:
                    with self.assertRaises(Refused):
                        receipts.require_container_absent("owned-current-id")

    def test_mount_sources_and_internal_network_are_verified(self):
        bindings = {"network_name": "owned-eval-fixture", "mount_sources": {"workspace": "/actual/task", "client": "/actual/client.py"}}
        network = {"Id": "actual-network-id", "Name": bindings["network_name"], "Internal": True}
        info = {"Image": collector_source.IMAGE,
            "HostConfig": {"ReadonlyRootfs": True, "PidsLimit": 128, "Memory": 805306368, "NanoCpus": 2000000000,
                           "Privileged": False, "NetworkMode": bindings["network_name"], "CapDrop": ["ALL"], "SecurityOpt": ["no-new-privileges"]},
            "NetworkSettings": {"Networks": {bindings["network_name"]: {"NetworkID": network["Id"]}}},
            "Mounts": [{"Destination": "/workspace", "Source": "/actual/task", "RW": True, "Type": "bind"},
                       {"Destination": "/driver/client.py", "Source": "/actual/client.py", "RW": False, "Type": "bind"}]}
        receipts.validate_isolation(info, bindings, network)
        for change in ("source", "writable_client", "network"):
            candidate, net = deepcopy(info), deepcopy(network)
            if change == "source":
                candidate["Mounts"][0]["Source"] = "/foreign/task"
            elif change == "writable_client":
                candidate["Mounts"][1]["RW"] = True
            else:
                net["Internal"] = False
            with self.subTest(change=change), self.assertRaises(Refused):
                receipts.validate_isolation(candidate, bindings, net)

    @unittest.skipUnless(os.name == "posix", "Retained authority fds require POSIX")
    def test_failed_seal_closes_duplicated_authority_and_creates_no_receipt(self):
        with tempfile.TemporaryDirectory(prefix="owned-issuer-close-") as tmp:
            root = Path(tmp)
            fd = os.open(root, os.O_RDONLY | os.O_DIRECTORY)
            duplicate = os.dup(fd)
            issuer = receipts.DockerRunIssuer.__new__(receipts.DockerRunIssuer)
            issuer.workspace_fd = duplicate
            issuer.bindings = {}
            issuer.validate_completion = mock.Mock(side_effect=Refused("expected_incomplete_source"))
            try:
                with self.assertRaisesRegex(Refused, "expected_incomplete_source"):
                    receipts.seal_actual_driver(issuer, root, {}, {}, [], [], [], root, {})
                with self.assertRaises(OSError):
                    os.fstat(duplicate)
                self.assertFalse((root / "receipt.json").exists())
                issuer.close()
            finally:
                os.close(fd)

    def test_unmarked_or_duplicate_clients_and_bundles_refuse_before_non_sys_imports(self):
        client = ROOT / "scripts/coding-eval/owned_tui_client.py"
        source = client.read_text()
        module = ast.parse(source)
        prefix = ast.Module(body=module.body[:4], type_ignores=[])
        # The docstring, sys import, guard function and guard are the only
        # executed nodes; no environment/config/path import may be reached.
        self.assertIsInstance(prefix.body[-1], ast.If)
        for arguments in ([], ["--owned-tui-client-v1", "--owned-tui-client-v1"],
                          ["--owned-native-relay-v3", "--owned-native-relay-v3"],
                          ["--owned-tui-keeper-v1", "4", "5", "--owned-tui-client-v1"]):
            namespace = {"__name__": "__main__"}
            real_import = __import__
            def only_sys(name, *args, **kwargs):
                self.assertEqual(name, "sys")
                return real_import(name, *args, **kwargs)
            namespace["__builtins__"] = {**vars(__import__("builtins")), "__import__": only_sys}
            with self.subTest(arguments=arguments), mock.patch.object(sys, "argv", ["owned-client", *arguments]):
                with self.assertRaises(SystemExit) as refused:
                    exec(compile(prefix, "<trusted-prefix>", "exec"), namespace)
                self.assertEqual(refused.exception.code, 0)
        with tempfile.TemporaryDirectory(prefix="owned-bundle-marker-") as tmp:
            root = Path(tmp)
            bundle = client_bundle(root / "client.py", client)
            collector = collector_bundle(root / "collector.py", client.parent)
            for path, marker in ((bundle, "--owned-tui-client-v1"), (collector, "--owned-native-collector-v3")):
                result = subprocess.run([sys.executable, "-I", "-B", str(path), marker, marker],
                    cwd=root, capture_output=True, timeout=20)
                self.assertEqual(result.returncode, 0)
                self.assertEqual(result.stdout + result.stderr, b"")
                self.assertFalse((root / "native_probe_witness.json").exists())


if __name__ == "__main__":
    unittest.main()
