#!/usr/bin/env python3
"""Check that scripts/gen-fixtures-v040.py only ever replaces its own files.

The generator writes into --out, and a mistyped path (the real ~/.craftsman, say)
must not lose anything. This script needs no baseline binary and no cargo: it
imports the generator, runs check_out_dir and install_output on directories it
builds in a temp dir, and starts the command line with a binary that does not
exist to show that the --out refusal comes before any work and removes nothing.

    python3 -I -B scripts/check-gen-fixtures-v040-out.py

Prints one line per case and exits 0 when every case behaves, 1 otherwise.
Standard library only.
"""

import importlib.util
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
GENERATOR = HERE / "gen-fixtures-v040.py"

sys.dont_write_bytecode = True  # importing the generator must not leave a __pycache__


def load_generator():
    spec = importlib.util.spec_from_file_location("gen_fixtures_v040", str(GENERATOR))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


gen = load_generator()
OLD_DB = b"old database"
NEW_DB = b"new database"
STAGED_JOURNAL = sorted(gen.journal_names())[0]


def refusal(out, force=False):
    """The refusal text from check_out_dir, or None when it lets `out` through."""
    try:
        gen.check_out_dir(Path(out), force)
    except SystemExit as stop:
        return str(stop.code)
    return None


def own_files(out):
    """Lay out `out` as an earlier run of the generator left it."""
    out.mkdir(parents=True, exist_ok=True)
    for name in gen.DATABASE_FILES:
        (out / name).write_bytes(OLD_DB)
    (out / gen.JOURNAL_DIR).mkdir(exist_ok=True)
    for name in sorted(gen.journal_names()):
        (out / gen.JOURNAL_DIR / name).write_bytes(b"{}\n")


def staged(root):
    """What a finished run has staged: a new database and one journal."""
    stage = root / "staged"
    (stage / gen.JOURNAL_DIR).mkdir(parents=True, exist_ok=True)
    (stage / "threads.db").write_bytes(NEW_DB)
    (stage / gen.JOURNAL_DIR / STAGED_JOURNAL).write_bytes(b"new\n")
    return stage


def snapshot(out):
    return {
        str(path.relative_to(out)): path.read_bytes()
        for path in sorted(out.rglob("*"))
        if path.is_file()
    }


def run_cli(*args):
    done = subprocess.run(
        [sys.executable, "-I", "-B", str(GENERATOR)] + list(args),
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=60,
    )
    return done.returncode, done.stderr.decode("utf-8", "replace")


CASES = []


def case(function):
    CASES.append(function)
    return function


@case
def a_missing_directory_is_allowed(tmp):
    assert refusal(tmp / "missing") is None


@case
def an_empty_directory_is_allowed(tmp):
    (tmp / "empty").mkdir()
    assert refusal(tmp / "empty") is None


@case
def the_generators_own_files_are_allowed(tmp):
    own_files(tmp / "mine")
    assert refusal(tmp / "mine") is None
    (tmp / "journals-only" / gen.JOURNAL_DIR).mkdir(parents=True)
    assert refusal(tmp / "journals-only") is None


@case
def a_foreign_file_is_refused_and_named(tmp):
    own_files(tmp / "d")
    (tmp / "d" / "notes.txt").write_text("mine")
    text = refusal(tmp / "d")
    assert text and "notes.txt" in text and "--force" in text, text


@case
def a_foreign_file_inside_the_journal_directory_is_refused(tmp):
    own_files(tmp / "d")
    (tmp / "d" / gen.JOURNAL_DIR / "keep.txt").write_text("mine")
    text = refusal(tmp / "d")
    assert text and "sessions-rs/keep.txt" in text, text


@case
def a_foreign_directory_is_refused(tmp):
    (tmp / "d" / "photos").mkdir(parents=True)
    text = refusal(tmp / "d")
    assert text and "photos" in text, text


@case
def a_file_in_place_of_the_directory_is_refused(tmp):
    (tmp / "afile").write_text("x")
    assert refusal(tmp / "afile")


@case
def a_symlinked_journal_directory_is_refused(tmp):
    elsewhere = tmp / "elsewhere"
    elsewhere.mkdir()
    (elsewhere / "precious.txt").write_text("x")
    (tmp / "d").mkdir()
    try:
        (tmp / "d" / gen.JOURNAL_DIR).symlink_to(elsewhere, target_is_directory=True)
    except (OSError, NotImplementedError):
        return "skipped: no symlinks here"
    assert refusal(tmp / "d")
    try:
        gen.install_output(staged(tmp), tmp / "d", False)
    except SystemExit:
        pass
    else:
        raise AssertionError("install_output accepted a symlinked sessions-rs")
    assert (elsewhere / "precious.txt").read_text() == "x"


@case
def the_default_fixture_directory_is_exempt(tmp):
    default = tmp / "default"
    own_files(default)
    (default / "README.md").write_text("provenance")
    (default / "stray.txt").write_text("x")
    saved = gen.DEFAULT_OUT
    gen.DEFAULT_OUT = default
    try:
        assert refusal(default) is None
        assert refusal(tmp / "default" / ".." / "default") is None
    finally:
        gen.DEFAULT_OUT = saved
    assert refusal(default)  # any other directory with the same content is not


@case
def force_passes_the_check_but_replaces_only_the_generators_files(tmp):
    out = tmp / "d"
    own_files(out)
    (out / "notes.txt").write_text("mine")
    (out / gen.JOURNAL_DIR / "keep.txt").write_text("replaced with the directory")
    assert refusal(out, force=True) is None
    gen.install_output(staged(tmp), out, True)
    assert (out / "threads.db").read_bytes() == NEW_DB
    assert sorted(p.name for p in (out / gen.JOURNAL_DIR).iterdir()) == [STAGED_JOURNAL]
    assert (out / "notes.txt").read_text() == "mine"
    assert not (out / "threads.db-wal").exists()


@case
def install_output_without_force_deletes_nothing_in_a_foreign_directory(tmp):
    out = tmp / "d"
    own_files(out)
    (out / gen.JOURNAL_DIR / "keep.txt").write_text("mine")
    (out / "notes.txt").write_text("mine")
    before = snapshot(out)
    try:
        gen.install_output(staged(tmp), out, False)
    except SystemExit:
        pass
    else:
        raise AssertionError("install_output wrote into a foreign directory")
    assert snapshot(out) == before


@case
def install_output_creates_a_missing_directory_and_replaces_an_earlier_run(tmp):
    out = tmp / "new" / "deeper"
    gen.install_output(staged(tmp), out, False)
    assert (out / "threads.db").read_bytes() == NEW_DB
    assert refusal(out) is None  # its own output is still allowed afterwards
    gen.install_output(staged(tmp), out, False)
    assert sorted(p.name for p in (out / gen.JOURNAL_DIR).iterdir()) == [STAGED_JOURNAL]


@case
def the_command_line_refuses_before_doing_any_work(tmp):
    out = tmp / "d"
    own_files(out)
    (out / gen.JOURNAL_DIR / "keep.txt").write_text("mine")
    before = snapshot(out)
    code, stderr = run_cli("--binary", str(tmp / "no-such-craftsman"), "--out", str(out))
    assert code != 0 and "refusing to write" in stderr, (code, stderr)
    assert "not an executable" not in stderr, "the binary was looked at before --out"
    assert snapshot(out) == before


@case
def the_command_line_lets_a_clean_directory_through_and_force_a_foreign_one(tmp):
    for name, extra in (("empty", None), ("forced", "notes.txt")):
        out = tmp / name
        out.mkdir()
        flags = []
        if extra:
            (out / extra).write_text("mine")
            flags = ["--force"]
        code, stderr = run_cli("--binary", str(tmp / "no-such-craftsman"), "--out", str(out), *flags)
        # Past the --out check, the missing binary is what stops it.
        assert code != 0 and "not an executable" in stderr, (name, code, stderr)
        assert "refusing" not in stderr, (name, stderr)
        assert sorted(p.name for p in out.iterdir()) == ([extra] if extra else [])


def main():
    failed = 0
    with tempfile.TemporaryDirectory(prefix="gen-out-check-") as scratch:
        for number, function in enumerate(CASES):
            tmp = Path(scratch) / ("case%02d" % number)
            tmp.mkdir()
            try:
                note = function(tmp)
                print("ok    %s%s" % (function.__name__, " (%s)" % note if note else ""))
            except Exception as error:  # noqa: BLE001 - report every failing case
                failed += 1
                print("FAIL  %s: %s: %s" % (function.__name__, type(error).__name__, error))
    print("%d of %d cases failed" % (failed, len(CASES)))
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
