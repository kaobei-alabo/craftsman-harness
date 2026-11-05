# Private guest fixture load correction

**Status: October 25, 2026 (Asia/Taipei).** This is a test-input correction for
PR #130's failed first Linux gadget-host run, NOT a production timeout/cache/SDK/
WIT/host/callable/exec/result/usage/OS-containment change.

## Preserved first hosted failure

Head `ed0c2c99e89317c8a9166aee2261d9c27472efee`, CI `40165049917`, one attempt:
formatting/macOS/Windows passed; Linux gadget main **1441 passed / 2 failed /
6 ignored**. New58 argument, old48 native and old42 composition cases passed
BOTH builds on EACH OS. Linux feature-off1302 passed; macOS1302/1443 and Windows
1277/1407 passed. All four raw logs used exact tree5cd68e1 and base87b357d.
That head stayed unmerged; no failed check was bypassed or retried unchanged.

Two existing guest tests failed: binary non-overwrite expected DeniedCapability
but got Timeout; the CPU/WASIp2-sleep case got Timeout during its 30 s load,
BEFORE the independent 1 s post-instantiation measurement. Test/host blobs were
identical to #129's base. The real host recompiles owned approved bytes inside
EACH existing operation budget, so its broad19-function input's unrelated network/
logging/large-output code also consumes those IO/deadline tests' loading budget.
This diagnosis is not a claim that every Timeout is transient or due to a fixture.

## Smaller input, identical measured behavior

The NEW unpublished [focused source fixture](../crates/craftsman-rs/tests/fixtures/headless-gadget-focused/README.md)
retains the four relevant branches (`read_bytes`, `write_bytes`, `spin`, `sleep`)
AND all malicious read/write/HTTP describe probes. Paths/bytes are still parsed
from ORIGINAL JSON, real host IO still creates/reads binary bytes and refuses
overwrite, and the original entered logs precede a real WASM spin or 3600 s Rust
WASIp2 sleep. The shared WIT/version, source approval, caps, policy and runtime
owners are unchanged. A new dynamic256-byte roundtrip/denied-overwrite case and
exact describe-denial/export check prevent a hardcoded-result fixture substitute.

Only TWO old tests choose the focused component; all their assertions, 30 s
loading/default operation limits, 1 s after-instantiation budget and 5 s outer
assertion remain byte-for-byte unchanged. The original broad source and build
helper remain exact for every other case. New fixture-only size/LTO settings and
its package-renamed copied lockfile do not change root dependencies/profiles/
toolchain, SDK/WIT or production Engine configuration.

Original/focused Cargo source inputs are built once together BEFORE any headless
fixture returns, preventing a late native Cargo/LTO build from competing with an
already-active guest budget. No Wasmtime compile/instantiate/describe/call is
moved out of the actual production load path or its 30 s operation budget.
There is NO native-code/component/approval cache, deserialize, bypass, warm guest,
new caller gate/seeds/source constructor, retry, fake marker/status/bytes/receipt,
post-readiness budget increase or accepted Timeout-instead-of-denial fallback.
The synchronous source build retains the overall CI bound and has no new absolute
native-call/process interruption guarantee.

## Collected local attempt evidence and limits

The first source build passed without warnings. The original binaries were
340104 bytes (broad) /141598 bytes (focused). A marked ignored probe used actual
runtime/source approval and host load, under the unchanged30 s budget:360 ms /
137 ms on this one macOS host. This is advisory local compile evidence, NOT a
portable latency/OS deadline, production benchmark, live-quality or release gate.
Unmarked probe returns BEFORE source/config work; normal suites ignore it.

Both original failed tests passed with unchanged assertions. The first NEW
export-check fixture wrongly expected declaration order; the existing parser
sorts names. Only the NEW expectation was corrected to that existing contract;
its initial failure remains logged. Corrected original2/new2 cases passed
warning-free. No old decoder/policy/fixture branch/test assertion/budget was changed.

Current-source bounded guest/argv/native/file/runtime/contract/driver/format/link/
protected checks and ALL FOUR fresh exact-final-head jobs AND EVERY mandatory
step must pass independently before missing-review-only admin merge. Keep the
failed initial #130 logs and earlier #128/#129 evidence separate. No unchanged
full-CI retry, weakened assertion/deadline or independent blocked-PR workaround.
Public exec/full flags/stdin/stdio/R2/results/events/storage/trace/provenance/
headless gadget callables/catalog/shared usage/#97/P2g/live/release remain separate.

Current corrected-source evidence: all30 bounded rows passed warning-free,
including all37 guest cases at four test threads, new58 argv/old48 native/old42
composition in BOTH modes, every selected older suite, both production checks,
actual marked/unmarked compile probes, fixture/root fmt, reference126/driver9/
whitespace/lineage. The exact22-path source/old-assertion/30s-1s-5s audit and74
local Markdown links/fences pass. Source branches/probe payload strings are
byte-exact, not only token-normalized. This is not final-head all-platform CI;
ALL FOUR fresh jobs/EVERY mandatory step still independently gate merge.
