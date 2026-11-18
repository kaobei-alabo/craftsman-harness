"""Retained main/WAL/SHM acquisition and ordinary recovery in a private copy.

Raw bytes cross only a transient driver channel and never enter report history.
SHM presence/bytes are observed, then discarded without committed authority; copied
databases use ordinary SQLite recovery, never immutable=1 or a live-path open.
"""
import hashlib
import os
from pathlib import Path
import sqlite3
import stat
import sys
import tempfile


class SnapshotFailure(ValueError):
    pass


def check(condition, code):
    if not condition:
        raise SnapshotFailure(code)


def close_owned_once(owners):
    """Attempt every owned close once; errors never become completion.

    A close error leaves its outcome unknown, even if the OS already released
    the handle. Do not retry a descriptor number that may have been reused.
    """
    primary = sys.exc_info()[1]
    failures = []
    for name, descriptor in owners:
        try:
            os.close(descriptor)
        except OSError as error:
            failures.append({"owner": name, "errno": error.errno,
                             "outcome": "unknown_not_retried"})
    if failures:
        error = SnapshotFailure("snapshot_cleanup_incomplete")
        error.cleanup_failures = tuple(failures)
        error.primary_failure = primary
        raise error from primary


def version(info):
    return (info.st_dev, info.st_ino, info.st_mode, info.st_size, info.st_mtime_ns, info.st_ctime_ns, info.st_nlink)


NAMES = ("threads.db", "threads.db-wal", "threads.db-shm")
MAXIMUM = 8388608
MAX_READ_ATTEMPTS = 4096
MAX_FRAMES = 16384


def signature(directory_fd, database_fd):
    result = {}
    for name in NAMES:
        try:
            named = os.stat(name, dir_fd=directory_fd, follow_symlinks=False)
        except FileNotFoundError:
            check(name != "threads.db", "snapshot_database_missing")
            result[name] = None
            continue
        check(stat.S_ISREG(named.st_mode) and named.st_nlink == 1 and named.st_size <= MAXIMUM,
              "snapshot_nonregular_or_large")
        result[name] = version(named)
    check(result["threads.db"] == version(os.fstat(database_fd)), "snapshot_database_rebound")
    return result


def checksum(data, byteorder, seed=(0, 0)):
    check(len(data) % 8 == 0, "snapshot_checksum_alignment")
    first, second = seed
    for position in range(0, len(data), 8):
        first = (first + int.from_bytes(data[position:position + 4], byteorder) + second) & 0xffffffff
        second = (second + int.from_bytes(data[position + 4:position + 8], byteorder) + first) & 0xffffffff
    return first, second


def validate_pair(files):
    main = files["threads.db"]
    check(len(main) >= 100 and main[:16] == b"SQLite format 3\0", "snapshot_database_header_invalid")
    page_size = int.from_bytes(main[16:18], "big")
    page_size = 65536 if page_size == 1 else page_size
    check(512 <= page_size <= 65536 and page_size & (page_size - 1) == 0 and len(main) % page_size == 0,
          "snapshot_database_page_size_invalid")
    wal = files.get("threads.db-wal", b"")
    if not wal:
        return {"wal_frames": 0, "committed_pages": int.from_bytes(main[28:32], "big"),
                "stale_tail_frames": 0, "wal_generation_sha256": None}
    check(len(wal) >= 32, "snapshot_wal_truncated")
    magic = int.from_bytes(wal[:4], "big")
    check(magic in (0x377f0682, 0x377f0683) and int.from_bytes(wal[4:8], "big") == 3007000,
          "snapshot_wal_header_invalid")
    check(int.from_bytes(wal[8:12], "big") == page_size, "snapshot_wal_page_size_mismatch")
    frame_size = 24 + page_size
    check((len(wal) - 32) % frame_size == 0, "snapshot_wal_frame_truncated")
    order = "big" if magic & 1 else "little"
    rolling = checksum(wal[:24], order)
    check(rolling == (int.from_bytes(wal[24:28], "big"), int.from_bytes(wal[28:32], "big")),
          "snapshot_wal_header_checksum_invalid")
    physical = (len(wal) - 32) // frame_size
    check(physical <= MAX_FRAMES, "snapshot_wal_frame_bound")
    committed_pages = 0
    for index in range(1, physical + 1):
        frame = wal[32 + (index - 1) * frame_size:32 + index * frame_size]
        # This deliberately supports only a complete current-generation WAL.
        # Reset/spill/old-salt tails may be lawful SQLite states; they remain
        # unsupported, not silently recovered as a successful whole snapshot.
        check(frame[8:16] == wal[16:24], "snapshot_wal_tail_or_salt_unsupported")
        page = int.from_bytes(frame[:4], "big")
        check(page > 0, "snapshot_wal_page_invalid")
        rolling = checksum(frame[:8] + frame[24:], order, rolling)
        check(rolling == (int.from_bytes(frame[16:20], "big"), int.from_bytes(frame[20:24], "big")),
              "snapshot_wal_frame_checksum_invalid")
        committed_pages = int.from_bytes(frame[4:8], "big")
    check(physical == 0 or committed_pages > 0, "snapshot_wal_uncommitted_tail_unsupported")
    return {"wal_frames": physical, "committed_pages": committed_pages, "stale_tail_frames": 0,
            "wal_generation_sha256": hashlib.sha256(wal[:24]).hexdigest()}


def acquire(directory_fd, database_fd, maximum=MAXIMUM, named_root=None):
    descriptors = {}
    try:
        before = signature(directory_fd, database_fd)
        directory_before = version(os.fstat(directory_fd))
        if named_root is not None:
            check(version(os.stat(named_root, follow_symlinks=False)) == directory_before, "snapshot_root_rebound")
        for name, expected in before.items():
            if expected is None:
                continue
            fd = os.dup(database_fd) if name == "threads.db" else os.open(
                name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=directory_fd)
            descriptors[name] = fd
            check(version(os.fstat(fd)) == expected, "snapshot_identity_changed")
        values = {}
        total = 0
        attempts = 0
        for name, fd in descriptors.items():
            os.lseek(fd, 0, os.SEEK_SET)
            data = bytearray()
            while True:
                attempts += 1
                check(attempts <= MAX_READ_ATTEMPTS, "snapshot_read_attempt_bound")
                value = os.read(fd, min(65536, maximum + 1 - total))
                if not value:
                    break
                total += len(value)
                check(total <= maximum, "snapshot_acquisition_bound")
                data.extend(value)
            check(len(data) == before[name][3], "snapshot_truncated")
            values[name] = bytes(data)
        # A second bounded content pass corroborates each retained snapshot;
        # the final fence still spans ALL files, not isolated per-file pairs.
        for name, fd in descriptors.items():
            os.lseek(fd, 0, os.SEEK_SET)
            corroboration = hashlib.sha256()
            count = 0
            while True:
                attempts += 1
                check(attempts <= MAX_READ_ATTEMPTS, "snapshot_read_attempt_bound")
                chunk = os.read(fd, min(65536, maximum + 1 - count))
                if not chunk:
                    break
                count += len(chunk)
                check(count <= maximum, "snapshot_corroboration_bound")
                corroboration.update(chunk)
            check(count == len(values[name]) and corroboration.hexdigest() == hashlib.sha256(values[name]).hexdigest(),
                  "snapshot_content_changed")
        check(signature(directory_fd, database_fd) == before and version(os.fstat(directory_fd)) == directory_before,
              "snapshot_pairing_or_version_changed")
        for name, fd in descriptors.items():
            check(version(os.fstat(fd)) == before[name], "snapshot_version_changed")
        if named_root is not None:
            check(version(os.stat(named_root, follow_symlinks=False)) == directory_before, "snapshot_root_rebound")
        pair = validate_pair(values)
        return {"files": values, "versions": before,
                "digests": {name: hashlib.sha256(data).hexdigest() for name, data in values.items()},
                "raw_bytes": total, "read_attempts": attempts, "root_version": directory_before,
                "pair": pair, "shm_is_observation_only": True}
    except OSError:
        raise SnapshotFailure("snapshot_input_unavailable") from None
    finally:
        close_owned_once(descriptors.items())


class PrivateReader:
    """Driver-held parent outside every candidate-accessible bind mount."""
    def __init__(self, parent, candidate_roots):
        self.parent = Path(parent).resolve()
        forbidden = [Path(path).resolve() for path in candidate_roots]
        check(all(self.parent != root and root not in self.parent.parents for root in forbidden),
              "snapshot_private_parent_is_candidate_accessible")
        self.parent.mkdir(mode=0o700, exist_ok=False)
        self.fd = os.open(self.parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        try:
            info = os.fstat(self.fd)
            check(info.st_uid == os.getuid() and stat.S_IMODE(info.st_mode) == 0o700, "snapshot_private_parent_unowned")
            self.identity = (info.st_dev, info.st_ino)
        except BaseException:
            self.close()
            raise

    def query(self, acquisition, thread=None, expected_workspace=None, expected_sequence=None):
        files = acquisition["files"]
        check(set(files) <= set(NAMES) and "threads.db" in files
              and all(isinstance(data, bytes) and hashlib.sha256(data).hexdigest() == acquisition["digests"][name]
                      for name, data in files.items()) and sum(map(len, files.values())) <= MAXIMUM,
              "snapshot_bytes_changed_after_acquisition")
        check(validate_pair(files) == acquisition["pair"], "snapshot_pairing_changed_after_acquisition")
        info = os.fstat(self.fd)
        named = os.stat(self.parent, follow_symlinks=False)
        check((info.st_dev, info.st_ino) == self.identity == (named.st_dev, named.st_ino),
              "snapshot_private_parent_rebound")
        with tempfile.TemporaryDirectory(prefix="verified-sqlite-", dir=self.parent) as temporary:
            root = Path(temporary)
            root.chmod(0o700)
            root_fd = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
            copies = {}
            conn = None
            try:
                root_identity = version(os.fstat(root_fd))[:3]
                for name in ("threads.db", "threads.db-wal"):
                    if name not in files:
                        continue
                    fd = os.open(name, os.O_RDWR | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600, dir_fd=root_fd)
                    copies[name] = fd
                    position = attempts = 0
                    while position < len(files[name]):
                        attempts += 1
                        check(attempts <= MAX_READ_ATTEMPTS, "snapshot_copy_write_attempt_bound")
                        count = os.write(fd, files[name][position:position + 65536])
                        check(count > 0, "snapshot_copy_partial_write")
                        position += count
                    os.fsync(fd)
                def verify_copies():
                    check(version(os.fstat(root_fd))[:3] == root_identity
                          == version(os.stat(root, follow_symlinks=False))[:3], "snapshot_copy_root_rebound")
                    for name, fd in copies.items():
                        before = version(os.fstat(fd))
                        check(stat.S_ISREG(before[2]) and before[6] == 1
                              and version(os.stat(name, dir_fd=root_fd, follow_symlinks=False)) == before,
                              "snapshot_copy_leaf_rebound")
                        os.lseek(fd, 0, os.SEEK_SET)
                        copied = hashlib.sha256()
                        total = attempts = 0
                        while True:
                            attempts += 1
                            check(attempts <= MAX_READ_ATTEMPTS, "snapshot_copy_read_attempt_bound")
                            data = os.read(fd, min(65536, MAXIMUM + 1 - total))
                            if not data:
                                break
                            total += len(data)
                            check(total <= MAXIMUM, "snapshot_copy_read_bound")
                            copied.update(data)
                        check(total == len(files[name]) and copied.hexdigest() == acquisition["digests"][name]
                              and version(os.fstat(fd)) == before
                              and version(os.stat(name, dir_fd=root_fd, follow_symlinks=False)) == before,
                              "snapshot_copy_bytes_changed")
                verify_copies()
                return self._query_copy(root, acquisition, thread, expected_workspace, expected_sequence, verify_copies)
            finally:
                close_owned_once([*copies.items(), ("private-copy-root", root_fd)])

    def _query_copy(self, root, acquisition, thread, expected_workspace, expected_sequence, verify_copies):
            # Ordinary SQLite creates a new SHM and recovers the copied WAL.
            # The live database path and stale live lock bytes are never used.
            conn = None
            try:
                # This connection is confined to the private copied namespace.
                # Normal WAL recovery may create/update that copy's journal
                # state; no connection or write touches the live source path.
                conn = sqlite3.connect((root / "threads.db").as_uri() + "?mode=rw", uri=True, timeout=5)
                progress = [0]
                def bounded_query():
                    progress[0] += 1
                    return progress[0] > 4096
                conn.set_progress_handler(bounded_query, 1000)
                version_row = conn.execute("PRAGMA user_version").fetchone()
                if version_row == (0,):
                    raise SnapshotFailure("checkpoint_initializing")
                check(version_row == (1,), "snapshot_schema_unverified")
                check(conn.execute("PRAGMA quick_check").fetchall() == [("ok",)], "snapshot_sqlite_integrity_failed")
                if thread is None:
                    row = conn.execute("SELECT thread_id,seq,state_json,suspension_json FROM checkpoints ORDER BY seq DESC LIMIT 1").fetchone()
                else:
                    row = conn.execute("SELECT thread_id,seq,state_json,suspension_json FROM checkpoints WHERE thread_id=? ORDER BY seq DESC LIMIT 1", (thread,)).fetchone()
                check(row is not None, "checkpoint_missing")
                check(isinstance(row[0], str) and len(row[0].encode()) <= 512 and type(row[1]) is int
                      and 0 <= row[1] <= 2**63 - 1 and isinstance(row[2], str) and len(row[2].encode()) <= MAXIMUM,
                      "snapshot_row_bound")
                if expected_workspace is not None:
                    workspace = conn.execute("SELECT workspace FROM threads WHERE id=?", (row[0],)).fetchone()
                    check(workspace == (expected_workspace,), "snapshot_workspace_mismatch")
                if expected_sequence is not None:
                    check(row[1] == expected_sequence, "snapshot_current_step_sequence_mismatch")
                verify_copies()
                return row
            except sqlite3.Error:
                raise SnapshotFailure("snapshot_sqlite_recovery_failed") from None
            finally:
                if conn is not None:
                    conn.close()

    def close(self):
        if self.fd is not None:
            descriptor = self.fd
            self.fd = None
            close_owned_once([("private-reader-root", descriptor)])
