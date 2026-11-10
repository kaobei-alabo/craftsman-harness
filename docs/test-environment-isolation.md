# Immutable environment test fixtures

The test fixtures avoid process-global environment mutation while other native
or asynchronous work is running. Original config, MCP, route and authentication
assertions are preserved.

## The concrete safety defect

Five existing test bodies called process-global `std::env::set_var/remove_var`
while other libtest/Tokio/native/server work could read the environment. Unique
synthetic variable names and a claim of sole writer ownership are insufficient:
Rust documents that Unix callers cannot rely on thread-safe foreign environment
access (including DNS), and recommends `Command::env` for child processes. A
new lock around only these setters would not serialize all foreign readers.

## Same assertions, immutable child environments

The four config/MCP/route cases keep their original names and complete existing
assertion bodies. Only unsafe setup/cleanup is removed. A fixed registered argv
marker chooses the same body in one actual source-built child test process;
exact synthetic values and missing-variable conditions are supplied with
Command.env/env_remove BEFORE process startup. Neither parent nor child mutates
its running environment. No ambient user configuration/stdin/SDK is inspected.
Unmarked/unknown/duplicate helpers return before environment/body inspection.
Successful native exit PLUS an exact completion marker written only AFTER all
original assertions prevents no-op/filtered helpers from being passing evidence.
Output is strict UTF-8/whole64KiB per pipe, with one20s test execution and5s owned
cleanup bounds; no unchanged retries, fake shell readiness, or production limit
change. A new ignored helper is fixture infrastructure, not a callable/CLI flag.

## Actual per-router auth source, not a manufactured auth result

The old concurrent serve protocol/auth test no longer changes the global proxy
secret mid-server. A cfg(test)-only source is bound to that actual router state;
its short mutex is released before request work. It starts absent and switches
to the same synthetic secret, retaining every real HTTP/WS/status/frame assertion.
The same existing absent/empty/lossy/header/constant-time comparison is reused.
Normal routers and the unchanged public authorized(headers) function still read
the original environment source; no auth default, protocol frame, route, policy,
capability or model behavior changes. Immutable-child present/absent/empty cases
independently exercise the REAL production environment adapter. Two independent
real routers protect source isolation without walking a real home directory.

## Limits and validation

All old shell/filesystem/guest/input/provider/assertion/SDK/WIT/native/process/
reader/clock limits remain unchanged, including20s readiness/15s invalid byte/
10s stop/6s partial/Windows2s+3s/30s outer/2s drain. No model call, installed gadget,
user/global env/SDK mutation, sampler/extra shell poll/new shell launcher, agent/
goal, warning suppression or independently blocked provider work. Framework,
deps/lock/Rust1.99.1/Wasmtime48.0.8/frontends/SafeBackend/desktop-v4/standard ACP/
schemas/goldens/trace/corpus remain unchanged. Documentation accompanies the
partial correction; private/public exec/usage/settlement/callable/headless catalog/
P2g/live/default-on/release gates stay separate and off.

Fresh genuinely changed-source validation must preserve every old assertion,
case name/multiplicity/ignored/doctest and disclose any old OR new failure. ONE
full per mode, not an unchanged retry; all FOUR exact-head jobs/EVERY setup,
mandatory and post step/both modes/actual macOS15+Windows/warnings/source/tree/
review proof precede missing-formal-review-only merge. Earlier failed evidence remains preserved.

## Maintained validation

The original config/MCP/route and real HTTP/WS authentication assertions remain,
with isolated child environments and per-router state. These tests run in both
full modes. Temporary shell phase/snapshot instrumentation and its investigation
docs have been retired; that removal does not change the environment correction,
original shell deadlines or production process ownership.
