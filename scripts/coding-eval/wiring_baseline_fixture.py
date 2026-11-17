"""Test-only source33 validation baselines; no Git history or caller anchor."""
import hashlib

from preflight_continuation import require


SOURCE_COMMIT = "33a80d99431fd707b148485cb714f365b1946803"
# Independently verified against SOURCE_COMMIT, including the Git blob header.
# Do not refresh these identities from an edited checkout.
BASELINES = {
    "scripts/verify.sh": (
        2512,
        "24809d54ead9039942ac01e52455bfc9e9b605e8b70ab33db0dcfd850cbf2240",
        "1bece9f72c9b3a90eccb9bd902f0bf36f51d6071",
    ),
    ".github/workflows/ci.yml": (
        7987,
        "4d8becd846dd3c45af9d8f0ce7587ea486a52bf6e14c4f58b9694ea951416d26",
        "47de15fe7b65fc54fe49912b9ea97d1553baf6e0",
    ),
}
ADDITIONS = {
    "scripts/verify.sh": (
        b'run_check coding-eval-latest "$verification_python" -I -B scripts/coding-eval/test_preflight_latest.py\n',
        b'run_check coding-eval-long-session "$verification_python" -I -B scripts/coding-eval/test_long_session.py\n',
        b'run_check coding-eval-owned-tui "$verification_python" -I -B scripts/coding-eval/test_owned_tui.py\n',
    ),
    ".github/workflows/ci.yml": (
        b"          python -I -B scripts/coding-eval/test_preflight_latest.py\n",
        b"          python -I -B scripts/coding-eval/test_long_session.py\n",
        b"          python -I -B scripts/coding-eval/test_owned_tui.py\n",
    ),
}


def original_validation_bytes(actual, path):
    """Recover only the exact original bytes, or refuse the current wiring."""
    require(path in BASELINES and type(actual) is bytes, "invalid_validation_baseline")
    lines = actual.splitlines(keepends=True)
    commands = ADDITIONS[path]
    occurrences = 1 if path == "scripts/verify.sh" else 2
    require(all(lines.count(command) == occurrences for command in commands),
            "incomplete_additive_validation_wiring")
    original = b"".join(line for line in lines if line not in commands)
    size, sha256, git_blob = BASELINES[path]
    header = b"blob " + str(len(original)).encode("ascii") + b"\0"
    require(len(original) == size and hashlib.sha256(original).hexdigest() == sha256
            and hashlib.sha1(header + original).hexdigest() == git_blob,
            "original_validation_baseline_changed")
    return original
