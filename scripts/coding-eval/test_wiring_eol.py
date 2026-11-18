"""Owned Git checkout controls for the two exact validation-file attributes."""
import hashlib
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
import preflight_latest as preflight
import wiring_baseline_fixture as baseline
from coding_eval_anchors import ORIGINAL_CORPUS_SHA256
from preflight_continuation import Refused


ROOT = Path(__file__).resolve().parents[2]
LF_RULES = {
    "scripts/verify.sh": b"scripts/verify.sh text eol=lf",
    ".github/workflows/ci.yml": b".github/workflows/ci.yml text eol=lf",
}
TEXT_CONTROLS = {
    "scripts/another.sh": b"synthetic nearby shell file\n",
    ".github/workflows/another.yml": b"synthetic nearby workflow file\n",
    "nested/scripts/verify.sh": b"synthetic nested shell file\n",
    "nested/.github/workflows/ci.yml": b"synthetic nested workflow file\n",
}
UNCHANGED_CONTROLS = {
    "crates/craftsman-rs/tests/fixtures/eol-synthetic.txt": b"synthetic compatibility LF bytes\n",
    "eol-synthetic.db": b"synthetic database attribute LF bytes\n",
}


class WiringCheckoutBoundaries(unittest.TestCase):
    def command(self, root, *arguments):
        return subprocess.run(["git", "-C", str(root), *arguments],
            check=True, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=20)

    def checkout(self, attributes):
        temporary = tempfile.TemporaryDirectory(prefix="craftsman-wiring-eol-")
        self.addCleanup(temporary.cleanup)
        root = Path(temporary.name)
        source = root / "source"
        source.mkdir()
        files = {".gitattributes": attributes, **TEXT_CONTROLS, **UNCHANGED_CONTROLS}
        for path in LF_RULES:
            actual = (ROOT / path).read_bytes()
            original = baseline.original_validation_bytes(actual, path)
            preflight.validate_additive_wiring(original, actual, path)
            files[path] = actual
        corpus = preflight.git(ROOT, "cat-file", "blob", "HEAD:evals/coding/tasks.json")
        self.assertEqual(hashlib.sha256(corpus).hexdigest(), ORIGINAL_CORPUS_SHA256)
        files["evals/coding/tasks.json"] = corpus
        for path, data in files.items():
            target = source / path
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(data)
        self.command(source, "init", "--quiet")
        self.command(source, "config", "core.autocrlf", "false")
        self.command(source, "add", "--all")
        self.command(source, "-c", "core.hooksPath=/dev/null", "-c", "commit.gpgsign=false",
            "-c", "user.name=Evaluation fixture", "-c", "user.email=fixture@localhost",
            "commit", "--quiet", "--no-gpg-sign", "-m", "Owned wiring attribute fixture")
        checkout = root / "checkout"
        self.command(root, "-c", "core.hooksPath=/dev/null", "-c", "protocol.file.allow=always",
            "clone", "--quiet", "--no-local", "--no-hardlinks", "--depth=1", "--single-branch",
            "--config", "core.autocrlf=true", source.as_uri(), str(checkout))
        self.assertEqual(self.command(checkout, "rev-parse", "--is-shallow-repository").stdout.strip(), b"true")
        self.assertEqual(self.command(checkout, "rev-list", "--count", "HEAD").stdout.strip(), b"1")
        self.assertEqual(self.command(checkout, "config", "--get", "core.autocrlf").stdout.strip(), b"true")
        return checkout, files

    def test_exact_lf_paths_keep_guard_bytes_and_preserve_other_attributes(self):
        attributes = (ROOT / ".gitattributes").read_bytes()
        for rule in LF_RULES.values():self.assertEqual(attributes.splitlines().count(rule), 1)
        checkout, source = self.checkout(attributes)
        for path in LF_RULES:
            actual = (checkout / path).read_bytes()
            self.assertEqual(actual, source[path])
            self.assertNotIn(b"\r", actual)
            original = baseline.original_validation_bytes(actual, path)
            preflight.validate_additive_wiring(original, actual, path)
        for path, data in TEXT_CONTROLS.items():
            self.assertEqual((checkout / path).read_bytes(), data.replace(b"\n", b"\r\n"))
        corpus = source["evals/coding/tasks.json"]
        self.assertEqual((checkout / "evals/coding/tasks.json").read_bytes(), corpus.replace(b"\n", b"\r\n"))
        for path, data in UNCHANGED_CONTROLS.items():self.assertEqual((checkout / path).read_bytes(), data)
        observed = self.command(checkout, "check-attr", "text", "eol", "--", *LF_RULES).stdout.decode().splitlines()
        for path in LF_RULES:
            self.assertIn(path + ": text: set", observed)
            self.assertIn(path + ": eol: lf", observed)

    def test_missing_misdirected_or_crlf_rule_keeps_actual_guard_refusal(self):
        attributes = (ROOT / ".gitattributes").read_bytes()
        for path, rule in LF_RULES.items():
            for mutation, replacement in (
                    ("missing", b""),
                    ("misdirected", rule.replace(path.encode(), (path + ".other").encode())),
                    ("crlf", rule.replace(b"eol=lf", b"eol=crlf"))):
                with self.subTest(path=path, mutation=mutation):
                    changed = attributes.replace(rule, replacement, 1)
                    checkout, source = self.checkout(changed)
                    actual = (checkout / path).read_bytes()
                    self.assertEqual(actual, source[path].replace(b"\n", b"\r\n"))
                    with self.assertRaisesRegex(Refused, "incomplete_additive_validation_wiring"):
                        baseline.original_validation_bytes(actual, path)
                    for other in LF_RULES:
                        if other == path:continue
                        retained = (checkout / other).read_bytes()
                        self.assertEqual(retained, source[other])
                        original = baseline.original_validation_bytes(retained, other)
                        preflight.validate_additive_wiring(original, retained, other)


if __name__ == "__main__":
    unittest.main()
