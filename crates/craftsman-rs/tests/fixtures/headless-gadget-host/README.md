# Private headless guest-host fixture

App-layer, source-built adversarial component for `gadgets::headless` tests.
It reuses the existing WIT file unchanged and does not link/modify the SDK exports,
old fixture corpus or runtime benchmark fixtures. Its independent locked Cargo
package builds with the repository's exact toolchain into a separate
`target/gadget-fixtures/headless-host` directory. No component binary is committed.

`describe` intentionally attempts read/write/HTTP imports; all must be denied even
after source approval. Calls exercise caught write denials followed by a read,
CPU loop or WASI sleep, raw bytes, ambient WASI probes, misleading guest error
strings, bounded diagnostics, memory and result limits. Additional exports exercise
original path/payload execution versus redacted guest JSON, synchronous shared
stdout/stderr/import logs, cut/UTF-8/collision failures and caught-import post-seal
WASIp2 writes. Post-readiness stop/deadline assertions remain unchanged; entry log
records include non-secret padding so delayed matchers do not hide their markers.
Tests use temporary roots and synthetic configured credential/source-bound grants,
never global environment mutation. These verify the PRIVATE guest output adapter,
not public ToolOutcome/provenance/result/trace, live-model/default-on/release gates,
native OS containment or a runnable exec route.
