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


def write_retained_leaf(root, name, expected, replacement, maximum=1048576, named_root=None):
    """Write only the verified open leaf. Rebound names fail, never redirect IO.

    This detects external changes; it is not an external-writer content CAS.
    A raced name replacement can leave the old owned leaf changed, but cannot
    redirect this write into a foreign sentinel.
    """
    require(isinstance(expected, bytes) and isinstance(replacement, bytes)
            and len(expected) <= maximum and len(replacement) <= maximum, "invalid_observation_write")
    require(isinstance(name, str) and name not in {"", ".", ".."} and "/" not in name and "\\" not in name,
            "invalid_observation_leaf")
    root_identity = (os.fstat(root).st_dev, os.fstat(root).st_ino)
    if named_root is not None:
        named = os.stat(named_root, follow_symlinks=False)
        require(stat.S_ISDIR(named.st_mode) and (named.st_dev, named.st_ino) == root_identity,
                "observation_write_root_rebound")
    before = os.stat(name, dir_fd=root, follow_symlinks=False)
    require(stat.S_ISREG(before.st_mode) and before.st_nlink == 1, "unsafe_observation_write_leaf")
    identity = lambda info: (info.st_dev, info.st_ino, info.st_mode, info.st_size, info.st_mtime_ns, info.st_ctime_ns, info.st_nlink)
    fd = os.open(name, os.O_RDWR | os.O_NOFOLLOW | os.O_NONBLOCK | getattr(os, "O_BINARY", 0), dir_fd=root)
    try:
        actual = os.fstat(fd)
        require(identity(actual) == identity(before), "observation_write_leaf_changed")
        current = bytearray()
        while True:
            chunk = os.read(fd, min(65536, maximum + 1 - len(current)))
            if not chunk:break
            current.extend(chunk)
            require(len(current) <= maximum, "observation_write_read_bound")
        require(bytes(current) == expected and identity(os.fstat(fd)) == identity(actual)
                and identity(os.stat(name, dir_fd=root, follow_symlinks=False)) == identity(before),
                "observation_write_source_changed")
        if named_root is not None:
            named = os.stat(named_root, follow_symlinks=False)
            require(stat.S_ISDIR(named.st_mode) and (named.st_dev, named.st_ino) == root_identity,
                    "observation_write_root_rebound")
        os.lseek(fd, 0, os.SEEK_SET)
        position = 0
        while position < len(replacement):
            count = os.write(fd, replacement[position:])
            require(count > 0, "observation_write_failed")
            position += count
        os.ftruncate(fd, len(replacement))
        os.fsync(fd)
        after = os.fstat(fd)
        named = os.stat(name, dir_fd=root, follow_symlinks=False)
        require(stat.S_ISREG(named.st_mode) and identity(named) == identity(after), "observation_write_name_rebound")
        if named_root is not None:
            named = os.stat(named_root, follow_symlinks=False)
            require(stat.S_ISDIR(named.st_mode) and (named.st_dev, named.st_ino) == root_identity,
                    "observation_write_root_rebound")
    finally:
        os.close(fd)


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
