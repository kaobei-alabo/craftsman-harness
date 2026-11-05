#!/usr/bin/env python3
"""Informational, release-mode gadget microbenchmark. Python 3.11+; no LLM calls."""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import sys
import tempfile
import time
import tomllib

SCHEMA = "craftsman-harness/gadget-benchmark@1"
TEST = "gadgets::tests::benchmark::measure_dispatch_and_catalog"
MAX_REPORT_BYTES = 64 * 1024


def run_tool(command: list[str], root: Path, env: dict[str, str]) -> subprocess.CompletedProcess:
    return subprocess.run(command, cwd=root, env=env, capture_output=True, check=False)


def text(output: bytes) -> str:
    return output.decode("utf-8", errors="replace")


def require_success(result: subprocess.CompletedProcess, operation: str) -> None:
    if result.returncode:
        # JSON-quote diagnostics: terminal controls and CI-command prefixes never
        # escape onto a human channel. Do not print captured model/tool data.
        detail = json.dumps(text(result.stderr[-16_384:]), ensure_ascii=True)
        raise RuntimeError(f"{operation} failed (exit {result.returncode}); diagnostic: {detail}")


def pinned_toolchain(root: Path) -> str:
    with (root / "rust-toolchain.toml").open("rb") as handle:
        pin = tomllib.load(handle)["toolchain"]["channel"]
    if not isinstance(pin, str) or not re.fullmatch(r"1\.\d+\.\d+", pin):
        raise ValueError("benchmark requires the repository's exact stable Rust pin")
    return pin


def check_toolchain(root: Path, pin: str, env: dict[str, str], run=run_tool) -> str:
    readonly = {**env, "RUSTUP_AUTO_INSTALL": "0"}
    installed = run(["rustup", "toolchain", "list"], root, readonly)
    require_success(installed, "installed-toolchain query")
    if not any(line.strip() == pin or line.strip().startswith(f"{pin}-") for line in text(installed.stdout).splitlines()):
        raise RuntimeError(f"Rust {pin} is missing; explicitly install it with wasm32-wasip2 before benchmarking")
    targets = run(["rustup", "target", "list", "--installed", "--toolchain", pin], root, readonly)
    require_success(targets, "installed-target query")
    if "wasm32-wasip2" not in text(targets.stdout).splitlines():
        raise RuntimeError(f"wasm32-wasip2 is missing for Rust {pin}; explicitly add the target before benchmarking")
    version = run(["rustup", "run", pin, "rustc", "--version"], root, readonly)
    require_success(version, "compiler-version query")
    result = text(version.stdout).strip()
    if not result.startswith(f"rustc {pin} "):
        raise RuntimeError("installed compiler does not match the exact repository pin")
    return result


def validate_report(value: object, pin: str, samples: int, warmups: int) -> dict:
    if not isinstance(value, dict) or value.get("schema") != SCHEMA:
        raise ValueError("invalid benchmark schema")
    expected_keys = {"schema", "scope", "crate_version", "rust_toolchain", "os", "arch",
                     "debug_assertions", "native_code_disk_cache", "samples", "warmups",
                     "component_sha256", "benchmark_binary_sha256", "model_requests", "gadget_list_calls",
                     "quality_gate_cleared", "fixture_setup_ns", "runtime_init_ns", "cold_dispatch_ns",
                     "initial_catalog_ns", "warm_dispatch", "warm_catalog", "unchanged_catalog_notices_added",
                     "tool_definitions_changed", "compiled_components"}
    if value.keys() != expected_keys:
        raise ValueError("unexpected or missing benchmark fields")
    if value.get("scope") != "runtime_microbenchmark" or value.get("rust_toolchain") != pin:
        raise ValueError("benchmark scope/toolchain mismatch")
    for name, expected in {"debug_assertions": False, "native_code_disk_cache": False,
                           "quality_gate_cleared": False, "tool_definitions_changed": False}.items():
        if value.get(name) is not expected:
            raise ValueError(f"invalid benchmark invariant: {name}")
    for name, expected in {"samples": samples, "warmups": warmups, "model_requests": 0,
                           "gadget_list_calls": 0, "unchanged_catalog_notices_added": 0,
                           "compiled_components": 1}.items():
        if type(value.get(name)) is not int or value[name] != expected:
            raise ValueError(f"invalid benchmark counter: {name}")
    for name in ["component_sha256", "benchmark_binary_sha256"]:
        if not isinstance(value.get(name), str) or not re.fullmatch(r"[0-9a-f]{64}", value[name]):
            raise ValueError("missing component/binary identity")
    if value.get("os") not in {"linux", "macos", "windows"} or value.get("arch") not in {"x86_64", "aarch64"}:
        raise ValueError("unsupported benchmark platform")
    if not isinstance(value.get("crate_version"), str) or not re.fullmatch(r"\d+\.\d+\.\d+", value["crate_version"]):
        raise ValueError("invalid crate version")
    for name in ["fixture_setup_ns", "runtime_init_ns", "cold_dispatch_ns", "initial_catalog_ns"]:
        if type(value.get(name)) is not int or value[name] < 0:
            raise ValueError(f"invalid duration: {name}")
    for name in ["warm_dispatch", "warm_catalog"]:
        summary = value.get(name)
        if not isinstance(summary, dict) or summary.keys() != {"samples", "min_ns", "p50_ns", "p95_ns", "p99_ns", "max_ns", "mean_ns"} or summary.get("samples") != samples or type(summary.get("samples")) is not int:
            raise ValueError("invalid distribution sample count")
        keys = ["min_ns", "p50_ns", "p95_ns", "p99_ns", "max_ns", "mean_ns"]
        if any(type(summary.get(key)) is not int or summary[key] < 0 for key in keys):
            raise ValueError("invalid distribution duration")
        ordered = [summary[key] for key in keys[:-1]]
        if ordered != sorted(ordered) or not summary["min_ns"] <= summary["mean_ns"] <= summary["max_ns"]:
            raise ValueError("inconsistent distribution")
    return value


def read_report(path: Path, pin: str, samples: int, warmups: int) -> dict:
    metadata = path.lstat()
    if not stat.S_ISREG(metadata.st_mode) or metadata.st_size > MAX_REPORT_BYTES:
        raise ValueError("benchmark report must be a bounded regular file, not a symlink")
    with path.open("rb") as handle:
        raw = handle.read(MAX_REPORT_BYTES + 1)
    if len(raw) > MAX_REPORT_BYTES:
        raise ValueError("benchmark report exceeds its bound")
    return validate_report(json.loads(raw), pin, samples, warmups)


def publish(value: dict, output: Path | None) -> None:
    data = json.dumps(value, ensure_ascii=True, indent=2) + "\n"
    if output is None:
        sys.stdout.write(data)
    else:
        # Never replace a previous measurement (also safe when writers race).
        with output.open("x", encoding="ascii") as handle:
            handle.write(data)
            handle.flush()
            os.fsync(handle.fileno())


def benchmark_environment(source: dict[str, str], pin: str) -> dict[str, str]:
    # The trusted source build needs no provider credentials. This removes
    # conventional credential variables; it is not an OS sandbox or redactor.
    env = {name: value for name, value in source.items()
           if not name.upper().endswith(("_API_KEY", "_TOKEN", "_SECRET", "_PASSWORD"))}
    return {**env, "RUSTUP_AUTO_INSTALL": "0", "RUSTUP_TOOLCHAIN": pin}


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--samples", type=int, default=500)
    parser.add_argument("--warmups", type=int, default=20)
    parser.add_argument("--offline", action="store_true", help="do not fetch Cargo dependencies")
    parser.add_argument("--output", type=Path, help="create a new JSON report; default stdout")
    args = parser.parse_args(argv)
    if not 20 <= args.samples <= 10_000 or not 1 <= args.warmups <= 1_000:
        parser.error("samples must be 20..10000; warmups 1..1000")
    try:
        if args.output is not None and os.path.lexists(args.output):
            raise FileExistsError("report already exists; choose a new output path")
        root = Path(__file__).resolve().parents[1]
        pin = pinned_toolchain(root)
        env = benchmark_environment(dict(os.environ), pin)
        env.update(CRAFTSMAN_GADGET_BENCH_SAMPLES=str(args.samples), CRAFTSMAN_GADGET_BENCH_WARMUPS=str(args.warmups))
        if args.offline:
            env["CARGO_NET_OFFLINE"] = "true"
        compiler = check_toolchain(root, pin, env)
        commit = run_tool(["git", "rev-parse", "HEAD"], root, env)
        require_success(commit, "source-commit query")
        source_commit = text(commit.stdout).strip()
        if not re.fullmatch(r"[0-9a-f]{40}|[0-9a-f]{64}", source_commit):
            raise ValueError("invalid source commit")
        dirty = run_tool(["git", "status", "--porcelain"], root, env)
        require_success(dirty, "source-status query")
        with tempfile.TemporaryDirectory(prefix="craftsman-gadget-benchmark-") as temporary:
            report_file = Path(temporary) / "measurement.json"
            env["CRAFTSMAN_GADGET_BENCH_OUTPUT"] = str(report_file)
            command = ["rustup", "run", pin, "cargo", "test", "--locked", "--release",
                       "--no-default-features", "--features", "gadgets", "-p", "craftsman-rs",
                       "--bin", "craftsman", TEST, "--", "--exact", "--ignored", "--nocapture", "--test-threads=1"]
            started = time.monotonic_ns()
            result = run_tool(command, root, env)
            build_and_measure_ns = time.monotonic_ns() - started
            require_success(result, "release benchmark")
            report = read_report(report_file, pin, args.samples, args.warmups)
        report.update(source_commit=source_commit, source_dirty=bool(dirty.stdout), compiler=compiler,
                      cargo_profile="release", build_and_measure_ns=build_and_measure_ns,
                      rustflags_present=bool(env.get("RUSTFLAGS") or env.get("CARGO_ENCODED_RUSTFLAGS")))
        publish(report, args.output)
        return 0
    except (OSError, ValueError, RuntimeError, KeyError) as error:
        sys.stderr.write("gadget benchmark failed: " + json.dumps(str(error), ensure_ascii=True) + "\n")
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
