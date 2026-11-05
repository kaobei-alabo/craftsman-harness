#!/usr/bin/env python3
"""No-network contract tests for the benchmark driver (Python standard library)."""
import copy
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("benchmark_gadgets", Path(__file__).with_name("benchmark-gadgets.py"))
bench = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bench)


def report():
    summary = {"samples":20,"min_ns":1,"p50_ns":2,"p95_ns":3,"p99_ns":4,"max_ns":5,"mean_ns":2}
    return {"schema":bench.SCHEMA,"scope":"runtime_microbenchmark","crate_version":"0.4.0",
            "rust_toolchain":"1.99.1","os":"linux","arch":"x86_64","debug_assertions":False,
            "native_code_disk_cache":False,"samples":20,"warmups":1,
            "component_sha256":"a"*64,"benchmark_binary_sha256":"b"*64,
            "model_requests":0,"gadget_list_calls":0,"quality_gate_cleared":False,
            "fixture_setup_ns":1,"runtime_init_ns":2,"cold_dispatch_ns":3,"initial_catalog_ns":4,
            "warm_dispatch":copy.deepcopy(summary),"warm_catalog":copy.deepcopy(summary),
            "unchanged_catalog_notices_added":0,"tool_definitions_changed":False,"compiled_components":1}


class BenchmarkContract(unittest.TestCase):
    def test_valid_measurement_is_advisory_and_matches_the_requested_sample_counts(self):
        value = report()
        self.assertIs(bench.validate_report(value, "1.99.1", 20, 1), value)
        with self.assertRaises(ValueError):
            bench.validate_report(value, "1.99.1", 500, 1)

    def test_invariants_cannot_masquerade_as_a_live_gate_or_accept_bool_counters(self):
        for key, bad in [("quality_gate_cleared",True),("model_requests",True),
                         ("native_code_disk_cache",True),("gadget_list_calls",1),
                         ("unchanged_catalog_notices_added",1),("tool_definitions_changed",True),
                         ("debug_assertions",True),("benchmark_binary_sha256","invalid")]:
            value = report(); value[key] = bad
            with self.subTest(key=key), self.assertRaises(ValueError):
                bench.validate_report(value, "1.99.1", 20, 1)

    def test_unexpected_fields_and_inconsistent_distributions_are_refused(self):
        for name in ["warm_dispatch", "warm_catalog"]:
            for key, bad in [("p95_ns",-1),("mean_ns",True),("p99_ns",2),("mean_ns",10)]:
                value = report(); value[name][key] = bad
                with self.subTest(name=name,key=key,bad=bad), self.assertRaises(ValueError):
                    bench.validate_report(value, "1.99.1", 20, 1)
        value = report(); value["arbitrary_text"] = "must not be published"
        with self.assertRaises(ValueError):
            bench.validate_report(value, "1.99.1", 20, 1)

    def test_publishing_never_overwrites_an_existing_result(self):
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory) / "result.json"
            bench.publish(report(), target)
            before = target.read_bytes()
            with self.assertRaises(FileExistsError): bench.publish(report(), target)
            self.assertEqual(target.read_bytes(), before)
            self.assertEqual(json.loads(before)["quality_gate_cleared"], False)

    def test_result_files_are_bounded_and_do_not_follow_symlinks(self):
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory) / "result.json"
            target.write_text(json.dumps(report()))
            self.assertEqual(bench.read_report(target, "1.99.1", 20, 1)["samples"], 20)
            oversized = Path(directory) / "oversized.json"
            oversized.write_bytes(b" " * (bench.MAX_REPORT_BYTES + 1))
            with self.assertRaises(ValueError): bench.read_report(oversized, "1.99.1", 20, 1)
            linked = Path(directory) / "link.json"
            try: linked.symlink_to(target)
            except OSError: return  # Windows may not grant symlink privileges.
            with self.assertRaises(ValueError): bench.read_report(linked, "1.99.1", 20, 1)

    def test_missing_toolchain_is_read_only_and_never_queries_or_installs_it(self):
        calls = []
        def fake(command, root, env):
            calls.append(command)
            self.assertEqual(env["RUSTUP_AUTO_INSTALL"], "0")
            return subprocess.CompletedProcess(command, 0, b"stable-test-host\n", b"")
        with self.assertRaises(RuntimeError): bench.check_toolchain(Path("."), "1.99.1", {}, fake)
        self.assertEqual(calls, [["rustup", "toolchain", "list"]])

    def test_missing_target_is_not_installed_and_cannot_reach_cargo(self):
        calls = []
        def fake(command, root, env):
            calls.append(command)
            self.assertEqual(env["RUSTUP_AUTO_INSTALL"], "0")
            output = b"1.99.1-test-host\n" if command[1] == "toolchain" else b"test-host\n"
            return subprocess.CompletedProcess(command, 0, output, b"")
        with self.assertRaises(RuntimeError): bench.check_toolchain(Path("."), "1.99.1", {}, fake)
        self.assertEqual(len(calls), 2)
        self.assertNotIn("install", sum(calls, []))
        self.assertNotIn("cargo", sum(calls, []))

    def test_provider_credentials_are_not_passed_to_native_benchmark_subprocesses(self):
        source = {"PATH":"test-path","CARGO_HOME":"public-cache", "OPENAI_API_KEY":"synthetic",
                  "CRAFTSMAN_PLATFORM_TOKEN":"synthetic", "ANTHROPIC_API_KEY":"synthetic", "CUSTOM_SECRET":"synthetic"}
        env = bench.benchmark_environment(source, "1.99.1")
        self.assertEqual(env, {"PATH":"test-path","CARGO_HOME":"public-cache", "RUSTUP_AUTO_INSTALL":"0", "RUSTUP_TOOLCHAIN":"1.99.1"})
        self.assertIn("OPENAI_API_KEY", source)  # parent environment is never mutated

    def test_existing_output_stops_before_any_compiler_or_git_query(self):
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory) / "existing.json"
            target.write_text("preserve")
            with patch.object(bench, "check_toolchain", side_effect=AssertionError("must not query")):
                self.assertEqual(bench.main(["--output",str(target)]), 1)
            self.assertEqual(target.read_text(), "preserve")


if __name__ == "__main__":
    unittest.main()
