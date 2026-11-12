#!/usr/bin/env python3
"""Stage/restore bounded external Cargo artifacts, never workspace outputs."""

import argparse
import json
import os
from pathlib import Path
import shutil
import stat
import sys

MAX_BYTES = 2 * 1024**3
CHUNK_BYTES = 1024**2
SUFFIXES = {".rlib", ".rmeta", ".d", ".a", ".lib", ".so", ".dylib", ".dll"}


def workspace_names(metadata):
    members = set(metadata["workspace_members"])
    names = set()
    for package in metadata["packages"]:
        if package["id"] in members:
            names.add(package["name"])
            names.update(target["name"] for target in package["targets"])
    return names | {name.replace("-", "_") for name in names}


def is_workspace_artifact(filename, names):
    name = filename.split("-", 1)[0]
    return name in names or name.removeprefix("lib") in names


def regular_files(directory):
    if directory.is_symlink() or not directory.is_dir():
        return
    for root, dirs, files in os.walk(directory, followlinks=False):
        root = Path(root)
        dirs[:] = sorted(name for name in dirs if not (root / name).is_symlink())
        for name in sorted(files):
            path = root / name
            if not path.is_symlink() and path.is_file():
                yield path


def candidates(debug, names):
    """Only compiler dependency directories and library/dep-info files."""
    for category in (".fingerprint", "build"):
        directory = debug / category
        if directory.is_symlink() or not directory.is_dir():
            continue
        for entry in sorted(directory.iterdir()):
            package = entry.name.rsplit("-", 1)[0]
            if package not in names:
                yield from regular_files(entry)

    deps = debug / "deps"
    if deps.is_symlink() or not deps.is_dir():
        return
    for path in sorted(deps.iterdir()):
        if path.is_symlink() or not path.is_file() or is_workspace_artifact(path.name, names):
            continue
        if path.suffix in SUFFIXES or (
            path.suffix == ".pdb" and path.with_suffix(".dll").is_file()
        ):
            yield path


def copy_artifacts(source, destination, names, max_bytes=MAX_BYTES):
    if source.is_symlink() or destination.is_symlink():
        raise ValueError("cache roots must be directories")
    source = source.resolve()
    destination = destination.resolve()
    copied = files = skipped = 0
    for path in candidates(source, names):
        target = destination / path.relative_to(source)
        # Current build outputs always take precedence over a restored cache.
        if target.exists() or target.is_symlink():
            continue
        size = path.stat().st_size
        if size > max_bytes - copied:
            skipped += 1
            continue
        # Never follow a link planted in the destination directory tree.
        parents = []
        for parent in target.parents:
            if parent == destination:
                break
            parents.append(parent)
        if any(parent.is_symlink() for parent in parents):
            skipped += 1
            continue
        target.parent.mkdir(parents=True, exist_ok=True)
        written = 0
        created = False
        try:
            with path.open("rb") as reader:
                info = os.fstat(reader.fileno())
                if not stat.S_ISREG(info.st_mode) or info.st_size != size:
                    skipped += 1
                    continue
                with target.open("xb") as writer:
                    created = True
                    while written < size:
                        chunk = reader.read(min(CHUNK_BYTES, size - written))
                        if not chunk:
                            raise ValueError("cache artifact changed while copying")
                        writer.write(chunk)
                        written += len(chunk)
                    if reader.read(1):
                        raise ValueError("cache artifact changed while copying")
            shutil.copystat(path, target, follow_symlinks=False)
        except FileExistsError:
            continue
        except (OSError, ValueError):
            if created:
                target.unlink(missing_ok=True)
            raise
        copied += written
        files += 1
    return {"files": files, "bytes": copied, "skipped_over_budget": skipped}


def run(operation, metadata):
    debug = Path(metadata["target_directory"]).resolve() / "debug"
    cache = debug.parent / "ci-dependency-cache" / "debug"
    names = workspace_names(metadata)
    if operation == "stage":
        # Create-only: a partial/previous restore cannot become a new saved cache.
        cache.parent.mkdir(parents=True, exist_ok=False)
        cache.mkdir()
        return copy_artifacts(debug, cache, names)
    if cache.is_symlink() or cache.parent.is_symlink() or not cache.is_dir():
        return {"files": 0, "bytes": 0, "skipped_over_budget": 0}
    return copy_artifacts(cache, debug, names)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("operation", choices=("stage", "restore"))
    parser.add_argument("--metadata", required=True, type=Path)
    args = parser.parse_args()
    try:
        result = run(args.operation, json.loads(args.metadata.read_text()))
    except (OSError, ValueError, KeyError, TypeError):
        print("Cargo dependency cache unavailable; compile from source.", file=sys.stderr)
        return 1
    print(json.dumps(result, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
