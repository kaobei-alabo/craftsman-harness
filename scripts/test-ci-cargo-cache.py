#!/usr/bin/env python3
"""Cache contracts use disposable files and never invoke a compiler/model."""

import importlib.util
from pathlib import Path
import re
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location("ci_cargo_cache", ROOT / "scripts/ci-cargo-cache.py")
cache = importlib.util.module_from_spec(spec)
spec.loader.exec_module(cache)


class CacheContracts(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.target = Path(self.temp.name) / "target"
        self.debug = self.target / "debug"
        self.metadata = {
            "target_directory": str(self.target),
            "workspace_members": ["workspace-id"],
            "packages": [{"id": "workspace-id", "name": "craftsman-rs",
                          "targets": [{"name": "craftsman"}, {"name": "golden_frames"}]}],
        }

    def write(self, name, text="artifact"):
        path = self.debug / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)
        return path

    def test_stages_dependencies_without_workspace_binaries_fixtures_or_credentials(self):
        expected = ["deps/libserde-ab.rlib", "deps/libserde-ab.rmeta", "deps/serde-ab.d",
                    "deps/serde_derive-ab.dll", "deps/serde_derive-ab.pdb",
                    "build/whisper-rs-sys-ab/out/library.a", ".fingerprint/serde-ab/lib-serde.json"]
        excluded = ["deps/craftsman-ab.exe", "deps/craftsman-ab.pdb", "deps/libcraftsman_rs-ab.rlib",
                    "deps/golden_frames-ab.d", "deps/test_without_extension-ab",
                    "build/craftsman-rs-ab/out/private", ".fingerprint/craftsman-rs-ab/bin.json",
                    "incremental/source/object.o", "gadget-fixtures/demo/gadget.wasm", "credentials.toml"]
        for path in expected + excluded:
            self.write(path)
        result = cache.run("stage", self.metadata)
        staged = self.target / "ci-dependency-cache/debug"
        self.assertEqual(result["files"], len(expected))
        self.assertEqual({str(p.relative_to(staged)) for p in staged.rglob("*") if p.is_file()}, set(expected))
        self.assertTrue(all((self.debug / p).is_file() for p in expected + excluded))

    def test_restore_filters_workspace_files_again_and_preserves_current_outputs(self):
        current = self.write("deps/libserde-ab.rlib", "current")
        saved = self.target / "ci-dependency-cache/debug/deps"
        saved.mkdir(parents=True)
        (saved / "libserde-ab.rlib").write_text("older")
        (saved / "libtokio-ab.rlib").write_text("dependency")
        (saved / "libcraftsman_rs-ab.rlib").write_text("workspace")
        (saved / "golden_frames-ab.exe").write_text("test")
        result = cache.run("restore", self.metadata)
        self.assertEqual(result["files"], 1)
        self.assertEqual(current.read_text(), "current")
        self.assertEqual((self.debug / "deps/libtokio-ab.rlib").read_text(), "dependency")
        self.assertFalse((self.debug / "deps/libcraftsman_rs-ab.rlib").exists())
        self.assertFalse((self.debug / "deps/golden_frames-ab.exe").exists())

    def test_budget_skips_whole_files_and_keeps_total_bounded(self):
        self.write("deps/libfirst-ab.rlib", "12345")
        self.write("deps/libsecond-ab.rlib", "12345678")
        result = cache.copy_artifacts(self.debug, self.target / "saved", set(), max_bytes=7)
        self.assertEqual(result, {"files": 1, "bytes": 5, "skipped_over_budget": 1})

    def test_stage_is_create_only(self):
        self.write("deps/libserde-ab.rlib")
        cache.run("stage", self.metadata)
        with self.assertRaises(FileExistsError):
            cache.run("stage", self.metadata)

    def test_missing_cache_is_a_cold_build(self):
        self.assertEqual(cache.run("restore", self.metadata)["files"], 0)

    def test_symlinked_files_and_directories_are_not_staged(self):
        secret = Path(self.temp.name) / "secret"
        secret.write_text("never cache this")
        deps = self.debug / "deps"
        deps.mkdir(parents=True)
        try:
            (deps / "libsecret-ab.rlib").symlink_to(secret)
            build = self.debug / "build"
            build.mkdir()
            (build / "dependency-ab").symlink_to(secret.parent, target_is_directory=True)
        except OSError:
            self.skipTest("creating symlinks requires host privileges")
        self.assertEqual(cache.run("stage", self.metadata)["files"], 0)

    def test_cargo_freshness_mtime_is_retained(self):
        self.assertEqual(cache.MAX_BYTES, 2 * 1024**3)
        # Restored artifacts must retain the mtime Cargo uses for freshness.
        source = self.write("deps/libserde-ab.rlib", "dependency")
        cache.run("stage", self.metadata)
        saved = self.target / "ci-dependency-cache/debug/deps/libserde-ab.rlib"
        self.assertEqual(saved.stat().st_mtime_ns, source.stat().st_mtime_ns)

    def test_restore_does_not_write_through_destination_links(self):
        saved = self.target / "ci-dependency-cache/debug/build/dependency-ab/out"
        saved.mkdir(parents=True)
        (saved / "library.a").write_text("dependency")
        other = Path(self.temp.name) / "outside"
        other.mkdir()
        build = self.debug / "build"
        build.mkdir(parents=True)
        try:
            (build / "dependency-ab").symlink_to(other, target_is_directory=True)
        except OSError:
            self.skipTest("creating symlinks requires host privileges")
        self.assertEqual(cache.run("restore", self.metadata)["files"], 0)
        self.assertEqual(list(other.iterdir()), [])


class WorkflowContracts(unittest.TestCase):
    def setUp(self):
        self.workflow = (ROOT / ".github/workflows/ci.yml").read_text()
        self.test_job = self.workflow.split("\n  test:\n", 1)[1]

    def test_draft_promotion_and_conversion_trigger_validation(self):
        types = re.search(r"types: \[(.*?)\]", self.workflow).group(1)
        self.assertEqual(set(types.split(", ")), {
            "opened", "synchronize", "reopened", "ready_for_review", "converted_to_draft",
        })
        gate = "if: ${{ github.event_name != 'pull_request' || github.event.pull_request.draft != true }}"
        self.assertIn(gate, self.test_job.split("    strategy:", 1)[0])

    def test_ready_validation_keeps_platforms_build_modes_and_limits(self):
        for platform in ("ubuntu-24.04", "macos-15", "windows-2025"):
            self.assertIn(f"runner: {platform}", self.test_job)
        for command in (
            "GGML_NATIVE=OFF cargo check --locked --workspace",
            "cargo test --locked --no-default-features --workspace --no-fail-fast -- --test-threads=4",
            "cargo test --locked --no-default-features --features craftsman-rs/gadgets --workspace --no-fail-fast -- --test-threads=4",
            "git diff --exit-code -- crates/craftsman-rs/tests/golden",
        ):
            self.assertIn(f"run: {command}", self.test_job)
        self.assertIn("timeout-minutes: 60", self.test_job)
        self.assertIn("fail-fast: false", self.test_job)

    def test_cache_partition_and_save_authority(self):
        self.assertIn("cargo-deps-v1-${{ runner.os }}-${{ runner.arch }}-", self.workflow)
        for source in ("rust-toolchain.toml", "**/Cargo.lock", "**/Cargo.toml", "scripts/ci-cargo-cache.py"):
            self.assertIn(source, self.workflow)
        self.assertNotIn("restore-keys:", self.workflow)
        self.assertNotIn("enableCrossOsArchive:", self.workflow)
        for block in self.test_job.split("      - name: ")[1:]:
            if block.startswith(("Stage Cargo dependency cache", "Save Cargo dependency cache")):
                self.assertIn("github.event_name == 'push'", block)
                self.assertIn("github.ref == 'refs/heads/main'", block)
        self.assertIn("steps.stage-cache.outcome == 'success'", self.workflow)
        self.assertIn("steps.cargo-cache.outputs.cache-primary-key", self.workflow)
        for path in ("~/.cargo/credentials", "target/debug", "target/gadget-fixtures", "target/release"):
            self.assertNotIn(path, self.workflow)


if __name__ == "__main__":
    unittest.main()
