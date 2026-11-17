"""Finite no-follow observations through an independently retained task root."""
import os
import stat
from preflight_continuation import require


def read_leaf(root, name, maximum=1048576):
    require(isinstance(name, str) and name not in {"", ".", ".."} and "/" not in name and "\\" not in name,
            "invalid_observation_leaf")
    before = os.stat(name, dir_fd=root, follow_symlinks=False)
    require(stat.S_ISREG(before.st_mode) and before.st_nlink == 1 and before.st_size <= maximum, "unsafe_observation_leaf")
    fd = os.open(name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | getattr(os, "O_BINARY", 0), dir_fd=root)
    identity = lambda info: (info.st_dev, info.st_ino, info.st_size, info.st_mtime_ns, info.st_ctime_ns, info.st_nlink)
    try:
        actual = os.fstat(fd)
        require(identity(actual) == identity(before) and stat.S_ISREG(actual.st_mode), "observation_leaf_changed")
        value = bytearray()
        while True:
            chunk = os.read(fd, min(65536, maximum + 1 - len(value)))
            if not chunk:
                break
            value.extend(chunk)
            require(len(value) <= maximum, "observation_leaf_bound")
        require(identity(os.fstat(fd)) == identity(actual), "observation_leaf_changed")
    finally:
        os.close(fd)
    require(identity(os.stat(name, dir_fd=root, follow_symlinks=False)) == identity(before), "observation_leaf_changed")
    return bytes(value)


def snapshot_retained(root):
    result = {}
    entries = 0
    total = 0
    def walk(fd, prefix):
        nonlocal entries, total
        for name in os.listdir(fd):
            entries += 1
            require(entries <= 1024, "task_snapshot_entry_bound")
            if name in {".git", "target", "__pycache__"}:
                continue
            info = os.stat(name, dir_fd=fd, follow_symlinks=False)
            if stat.S_ISDIR(info.st_mode):
                child = os.open(name, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=fd)
                try:walk(child, prefix + name + "/")
                finally:os.close(child)
            else:
                value = read_leaf(fd, name)
                total += len(value)
                require(total <= 8 * 1024 * 1024, "task_snapshot_byte_bound")
                result[prefix + name] = value
    walk(root, "")
    return result
