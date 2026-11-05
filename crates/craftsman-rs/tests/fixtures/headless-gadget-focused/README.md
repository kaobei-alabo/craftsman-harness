# Focused private guest test input

This standalone, unpublished source component is used ONLY by the binary-IO and
CPU/WASIp2-sleep headless tests. It uses the SAME existing Craftsman gadget WIT world;
it is not an SDK/public example, production host change or callable registration.

It retains the original guest's `read_bytes`, `write_bytes`, `spin` and `sleep`
branches and all three malicious describe probes. It parses ORIGINAL dynamic
JSON paths/bytes, performs actual host IO, logs the original entered markers,
spins in real WASM and sleeps via the same real Rust WASIp2 clock path for 3600 s.
It does not return fake bytes/timeouts or alter source/capability approval.
The broad 19-function source fixture remains unchanged for every other test.

The fixture-only release profile uses size optimization and whole-program LTO.
Its copied lockfile differs ONLY in this fixture's package name; public dependency
versions, workspace lock/toolchain, SDK/WIT and production compiler settings stay
unchanged. Native Cargo source construction happens before guest budgets; the
original and focused input builds are coordinated before any headless fixture
returns so a late Cargo/LTO build does not compete with another active guest call.

Actual Wasmtime compilation/instantiation/describe/IO still runs through the same
real host, original source authority and unchanged 30 s operation/load budget.
The strict CPU/sleep checks retain 1 s after real instantiation and a 5 s outer
assertion. No compiled-component/native-code/approval cache, deserialize, warmed
caller scope, timeout retry, skipped assertion or widened budget is introduced.
The ordinary CI job provides the unchanged overall source-build bound; synchronous
Cargo/native compiler work is not claimed cancellable or absolutely OS-bounded.
