"""Marked v2 fixture: one immutable child, actual bounded wait/stream witness."""
import sys

if __name__ == "__main__" and sys.argv[1:] != ["--owned-native-probe-v2"]:
    raise SystemExit(0)

import hashlib
import json
import os
from pathlib import Path
import secrets
import stat
import subprocess


CHILD_SOURCE_SHA256 = "4ca33bc733f3498c6817bf3a628bb0875b7b7cb17a3a35bc4e69d1c7d9ea6c87"


def main():
    if os.name != "posix":
        raise ValueError("native_probe_requires_posix")
    root = os.open(Path(__file__).resolve().parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    source = witness = None
    child = None
    try:
        source = os.open("probe_once.py", os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=root)
        before = os.fstat(source)
        if not stat.S_ISREG(before.st_mode) or before.st_nlink != 1 or before.st_size > 65536:
            raise ValueError("native_probe_source_refused")
        data = bytearray()
        while True:
            chunk = os.read(source, 65536)
            if not chunk:
                break
            data.extend(chunk)
            if len(data) > 65536:
                raise ValueError("native_probe_source_bound")
        after = os.fstat(source)
        fields = lambda info: (info.st_dev, info.st_ino, info.st_size, info.st_mtime_ns, info.st_ctime_ns, info.st_nlink)
        if fields(before) != fields(after) or hashlib.sha256(data).hexdigest() != CHILD_SOURCE_SHA256:
            raise ValueError("native_probe_source_changed")
        os.lseek(source, 0, os.SEEK_SET)
        # Reserve the owned witness name before spawning. A failed/incomplete
        # attempt stays incomplete and cannot trigger an automatic replay.
        witness = os.open("native_probe_witness.json", os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600, dir_fd=root)
        instance = secrets.token_hex(16)
        loader = ("import os,sys; f=os.fdopen(" + str(source) + ", 'rb', closefd=False); "
                  "b=f.read(65537); assert len(b)<=65536; "
                  "sys.argv=['probe_once.py','--craftsman-soak-v1']; "
                  "exec(compile(b,'probe_once.py','exec'),{'__name__':'__main__','__file__':'probe_once.py'})")
        child = subprocess.Popen([sys.executable, "-I", "-B", "-c", loader],
            cwd=Path(__file__).resolve().parent, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            pass_fds=(source,), env={"PATH": os.defpath, "PYTHONDONTWRITEBYTECODE": "1"})
        stdout, stderr = child.communicate(timeout=20)
        if len(stdout) > 65536 or len(stderr) > 65536 or type(child.returncode) is not int:
            raise ValueError("native_probe_observation_bound")
        value = {"version": 2, "kind": "observed_native_probe_child", "child_instance": instance,
                 "child_source_sha256": CHILD_SOURCE_SHA256, "actual_wait_returned": True,
                 "actual_child_exit": child.returncode, "stdout_bytes": len(stdout), "stderr_bytes": len(stderr),
                 "stdout_sha256": hashlib.sha256(stdout).hexdigest(), "stderr_sha256": hashlib.sha256(stderr).hexdigest(),
                 "outer_toolmeta_observed": False}
        encoded = json.dumps(value, sort_keys=True, separators=(",", ":")).encode()
        if len(encoded) > 2048:
            raise ValueError("native_probe_witness_bound")
        position = 0
        while position < len(encoded):
            written = os.write(witness, encoded[position:])
            if written <= 0:
                raise ValueError("native_probe_witness_write_failed")
            position += written
        os.fsync(witness)
        # Forward exactly the actual captured bytes once. The child witness is
        # not an assertion about the outer ShellRunner's metadata or cleanup.
        sys.stdout.buffer.write(stdout)
        sys.stdout.buffer.flush()
        sys.stderr.buffer.write(stderr)
        sys.stderr.buffer.flush()
        return child.returncode if child.returncode >= 0 else 128 - child.returncode
    finally:
        if child is not None:
            if child.poll() is None:
                child.kill()
                child.wait(timeout=5)
            for stream in (child.stdout, child.stderr):
                if stream is not None:
                    stream.close()
        for fd in (source, witness, root):
            if fd is not None:
                os.close(fd)


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError, subprocess.SubprocessError):
        raise SystemExit(2)
