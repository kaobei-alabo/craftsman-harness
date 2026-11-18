"""Marked fixed fixture child; verified snapshot and bounded actual collection.

The standalone task-file witness is only a local fixture diagnostic. Runtime
issuance uses this code in the separately owned read-only collector, never a
candidate-written witness. The original probe and its marker stay unchanged.
"""
import sys

if __name__ == "__main__" and sys.argv[1:] != ["--owned-native-probe-v2"]:
    raise SystemExit(0)

import hashlib
import json
import os
from pathlib import Path
import secrets
import selectors
import stat
import subprocess
import time


CHILD_SOURCE_SHA256 = "4ca33bc733f3498c6817bf3a628bb0875b7b7cb17a3a35bc4e69d1c7d9ea6c87"
SOURCE_LIMIT = STREAM_LIMIT = 65536
EXECUTION_SECONDS = 20
CLEANUP_SECONDS = 5


class NativeFailure(ValueError):
    def __init__(self, code, facts=None):
        super().__init__(code)
        self.facts = facts or {"complete": False, "probe_executed": False}


def version(info):
    return (info.st_dev, info.st_ino, info.st_size, info.st_mtime_ns, info.st_ctime_ns, info.st_nlink)


def verified_source(root):
    """Return a retained descriptor and independently bounded immutable bytes."""
    source = os.open("probe_once.py", os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=root)
    try:
        before = os.fstat(source)
        named = os.stat("probe_once.py", dir_fd=root, follow_symlinks=False)
        if not stat.S_ISREG(before.st_mode) or before.st_nlink != 1 or before.st_size > SOURCE_LIMIT or version(before) != version(named):
            raise NativeFailure("native_probe_source_refused")
        data = bytearray()
        while True:
            chunk = os.read(source, min(65536, SOURCE_LIMIT + 1 - len(data)))
            if not chunk:
                break
            data.extend(chunk)
            if len(data) > SOURCE_LIMIT:
                raise NativeFailure("native_probe_source_bound")
        snapshot = bytes(data)
        if version(os.fstat(source)) != version(before) or len(snapshot) != before.st_size or hashlib.sha256(snapshot).hexdigest() != CHILD_SOURCE_SHA256:
            raise NativeFailure("native_probe_source_changed")
        return source, snapshot, version(before)
    except BaseException:
        os.close(source)
        raise


def child_loader(source, acknowledgement, expected_version, bound_ack):
    # Compile only b, the verified snapshot delivered on the private pipe.
    # A second retained-source read detects a raced mutation but is never the
    # executed buffer. Restoring the name cannot erase its version change.
    return "\n".join([
        "import hashlib,os,sys",
        "b=sys.stdin.buffer.read(65537)",
        "h=hashlib.sha256(b).hexdigest()",
        "if len(b)>65536 or h!=" + repr(CHILD_SOURCE_SHA256) + ": sys.exit(125)",
        "f=" + str(source),
        "fields=lambda s:(s.st_dev,s.st_ino,s.st_size,s.st_mtime_ns,s.st_ctime_ns,s.st_nlink)",
        "expected=" + repr(expected_version),
        "if fields(os.fstat(f))!=expected: sys.exit(125)",
        "os.lseek(f,0,os.SEEK_SET)",
        "with os.fdopen(os.dup(f),'rb') as retained: current=retained.read(65537)",
        "if fields(os.fstat(f))!=expected or hashlib.sha256(current).hexdigest()!=h: sys.exit(125)",
        "os.write(" + str(acknowledgement) + ",bytes.fromhex(h)+bytes.fromhex(" + repr(bound_ack) + "))",
        "os.close(" + str(acknowledgement) + ")",
        "sys.argv=['probe_once.py','--craftsman-soak-v1']",
        "exec(compile(b,'probe_once.py','exec'),{'__name__':'__main__','__file__':'probe_once.py'})",
    ])


def collect_child(root, source, snapshot, expected_version, instance, cwd=None, binding=None):
    """Bound both streams while reading; a refusal never completes a witness."""
    if not isinstance(snapshot, bytes) or len(snapshot) > SOURCE_LIMIT or hashlib.sha256(snapshot).hexdigest() != CHILD_SOURCE_SHA256:
        raise NativeFailure("native_probe_snapshot_refused")
    child = None
    read_ack, write_ack = os.pipe()
    selector = selectors.DefaultSelector()
    retained = {"stdout": bytearray(), "stderr": bytearray(), "ack": bytearray()}
    eof = {"stdout": False, "stderr": False, "ack": False}
    position = 0
    wait_returned = False
    error = None
    started = time.monotonic()
    cleanup = {"needed": False, "actual_wait_returned": False, "actual_child_exit": None}
    bound_ack = hashlib.sha256(json.dumps({"source": CHILD_SOURCE_SHA256, "instance": instance,
        "binding": binding}, sort_keys=True, separators=(",", ":")).encode()).hexdigest()
    expected_ack = bytes.fromhex(CHILD_SOURCE_SHA256 + bound_ack)
    try:
        child = subprocess.Popen([sys.executable, "-I", "-B", "-c", child_loader(source, write_ack, expected_version, bound_ack)],
            cwd=cwd or "/proc/self/fd/" + str(root),
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            pass_fds=(source, write_ack, root), env={"PATH": os.defpath, "PYTHONDONTWRITEBYTECODE": "1"})
        os.close(write_ack)
        write_ack = None
        for stream, name in ((child.stdout, "stdout"), (child.stderr, "stderr"), (read_ack, "ack")):
            os.set_blocking(stream if isinstance(stream, int) else stream.fileno(), False)
            selector.register(stream, selectors.EVENT_READ, name)
        os.set_blocking(child.stdin.fileno(), False)
        selector.register(child.stdin, selectors.EVENT_WRITE, "snapshot")
        while selector.get_map():
            if time.monotonic() - started >= EXECUTION_SECONDS:
                raise NativeFailure("native_probe_execution_timeout")
            for key, _ in selector.select(min(0.05, max(0, EXECUTION_SECONDS - (time.monotonic() - started)))):
                if key.data == "snapshot":
                    try:
                        count = os.write(key.fd, snapshot[position:position + 4096])
                    except BrokenPipeError:
                        count = 0
                        position = len(snapshot)
                    position += count
                    if position == len(snapshot):
                        selector.unregister(key.fileobj)
                        child.stdin.close()
                    continue
                limit = 64 if key.data == "ack" else STREAM_LIMIT
                value = os.read(key.fd, min(65536, limit + 1 - len(retained[key.data])))
                if not value:
                    eof[key.data] = True
                    selector.unregister(key.fileobj)
                else:
                    if len(retained[key.data]) + len(value) > limit:
                        raise NativeFailure("native_probe_stream_overflow")
                    retained[key.data].extend(value)
        child.wait(timeout=max(0.001, EXECUTION_SECONDS - (time.monotonic() - started)))
        wait_returned = True
        if bytes(retained["ack"]) != expected_ack:
            raise NativeFailure("native_probe_executed_buffer_refused")
        if not all(eof.values()):
            raise NativeFailure("native_probe_stream_eof_incomplete")
    except (OSError, ValueError, subprocess.SubprocessError) as caught:
        error = str(caught) if isinstance(caught, NativeFailure) else "native_probe_child_unavailable"
    finally:
        selector.close()
        if child is not None:
            try:
                if child.poll() is None:
                    cleanup["needed"] = True
                    child.kill()
                child.wait(timeout=CLEANUP_SECONDS)
                cleanup["actual_wait_returned"] = True
                cleanup["actual_child_exit"] = child.returncode
            except (OSError, subprocess.SubprocessError):
                error = "native_probe_cleanup_incomplete"
            for stream in (child.stdin, child.stdout, child.stderr):
                if stream is not None and not stream.closed:
                    stream.close()
        os.close(read_ack)
        if write_ack is not None:
            os.close(write_ack)
    if error is not None:
        raise NativeFailure(error, {"complete": False, "verified_buffer_acknowledged": bytes(retained["ack"]) == expected_ack,
            "actual_wait_returned": wait_returned, "cleanup": cleanup,
            "stdout_eof": eof["stdout"], "stderr_eof": eof["stderr"], "ack_pipe_eof": eof["ack"],
            "retained_stdout_bytes": len(retained["stdout"]), "retained_stderr_bytes": len(retained["stderr"])})
    stdout, stderr = bytes(retained["stdout"]), bytes(retained["stderr"])
    facts = {"version": 3, "kind": "observed_native_probe_child", "complete": True,
        "child_instance": instance, "child_source_sha256": bytes(retained["ack"][:32]).hex(),
        "source_instance_binding_acknowledged": bytes(retained["ack"][32:]).hex() == bound_ack,
        "actual_wait_returned": wait_returned, "actual_child_exit": child.returncode,
        "stdout_eof": eof["stdout"], "stderr_eof": eof["stderr"], "ack_pipe_eof": eof["ack"],
        "source_authority": "verified_bounded_snapshot", "exact_executed_buffer_verified": True,
        "stdout_bytes": len(stdout), "stderr_bytes": len(stderr),
        "stdout_sha256": hashlib.sha256(stdout).hexdigest(), "stderr_sha256": hashlib.sha256(stderr).hexdigest(),
        "outer_toolmeta_observed": False, "execution_limit_seconds": EXECUTION_SECONDS,
        "cleanup_limit_seconds": CLEANUP_SECONDS, "stream_limit_bytes": STREAM_LIMIT, "cleanup": cleanup}
    return facts, stdout, stderr


def observe(root, witness_root, binding=None, cwd=None):
    """One create-only attempt in authority held by the actual collector."""
    source = witness = None
    try:
        source, snapshot, source_version = verified_source(root)
        witness = os.open("native_probe_witness.json", os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600, dir_fd=witness_root)
        facts, stdout, stderr = collect_child(root, source, snapshot, source_version, secrets.token_hex(16), cwd)
        if binding is not None:
            facts["collector_binding"] = binding
            facts["classification"] = "independently_requested_fixed_fixture_child"
        data = json.dumps(facts, sort_keys=True, separators=(",", ":")).encode()
        if len(data) > 2048:
            raise NativeFailure("native_probe_witness_bound")
        position = 0
        while position < len(data):
            count = os.write(witness, data[position:])
            if count <= 0:
                raise NativeFailure("native_probe_witness_write_failed")
            position += count
        os.fsync(witness)
        return facts, stdout, stderr
    finally:
        for fd in (source, witness):
            if fd is not None:
                os.close(fd)


def main():
    if os.name != "posix":
        raise NativeFailure("native_probe_requires_posix")
    directory = Path(__file__).resolve().parent
    root = os.open(directory, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        facts, stdout, stderr = observe(root, root, cwd=str(directory))
        sys.stdout.buffer.write(stdout)
        sys.stdout.buffer.flush()
        sys.stderr.buffer.write(stderr)
        sys.stderr.buffer.flush()
        return facts["actual_child_exit"] if facts["actual_child_exit"] >= 0 else 128 - facts["actual_child_exit"]
    finally:
        os.close(root)


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError, subprocess.SubprocessError):
        raise SystemExit(2)
