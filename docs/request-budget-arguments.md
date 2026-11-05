# Private request-budget arguments

The new private `input::request_budget` adapter accepts **`--max-rounds`** and
connects it to the existing shared logical-request meter. It returns the same
`Run` or paired `FileRun` as the older adapters. Public `craftsman exec` remains
unavailable; this is an implementation building block for the accepted R5 contract.

## Selection and meaning

The default is **50**. A single split `--max-rounds 2` or joined
`--max-rounds=2` selects an integer in **1..10000**. Values must consist of ASCII
decimal digits. Empty, missing, signed, fractional, overflowing, out-of-range or
duplicate controls fail with fixed errors before configuration/input/session IO.

The cap counts logical model-request admissions across **main, delegated and
compaction providers**. It does not count only conversational rounds. Internal
transport retries stay within their original logical admission. Existing failures
and drops retain their admitted history. The last allowed response may complete;
only a subsequent attempted admission causes refusal. The future public limit is
`limits.max_requests`; no public result or token/usage ledger is emitted here.

## Original arguments and authority

The adapter admits the whole original native argument count and encoded token/
total lengths before strict UTF-8 conversion, retention or copies. Its ceilings
are 1432 entries, 1048608 bytes per token and 11598352 aggregate encoded bytes:
only two entries and 32 bytes of overhead beyond the existing scoped adapter.
Original input, attachment, selector, path and destination bounds still apply.

A split flag's value remains data, including a spelling that the old grammar
will reject. Extraction cannot promote `--prompt-file --max-rounds=2` into a
valid budget selection. Joined `--prompt-file=--max-rounds=2` preserves the
original filename; `-- --max-rounds=2` preserves the literal prompt and default.
All other source, permission, session, relative-scope and output-file syntax
passes through the existing complete grammar before any effects.

The validated opaque `RequestLimit` flows through new, explicitly selected
factory methods to `Runtime::build_with_request_limit`. The same Bootstrap,
retained filesystem, captured credentials, input-first preparation, invocation
gate and shared session builder remain authoritative. No caller-created meter,
runtime, gate or output target is accepted. Output capture stays before native
stdin, and the same file terminal owns one create-only publication attempt.

The older argument/control/scoped routes retain the default and continue
rejecting this flag. Token/wall budgets, public formats, trace, gadget controls
and unknown flags remain rejected. Optional token reservations and their original
factories are unchanged. Real approval-required state dominates budget refusal;
neither a caught child/compaction error nor the cap can reopen the execution gate.

## Verification and limits

Thirty new tests cover malformed/native inputs, data preservation, unchanged old
grammar, actual builder main/child/automatic compaction sharing, final responses
at the cap, refusal without another provider request, approval dominance and
retained private file publication. Four marked native-pipe scenarios exercise
actual stdin and both output routes; their parent tests require native success
and post-assertion completion. Unmarked/unknown/duplicate helpers return before
fixture or stdin inspection. Pipes have 64 KiB output limits, 20-second execution
and 5-second owned cleanup bounds. Only isolated configs and synthetic providers
are used. The one ignored test is the explicitly marked helper, not skipped
behavior coverage.

The initial feature-off focused run passed all 30 cases without compiler warnings.
Whole-workspace and hosted validation are recorded in
[production readiness](production-readiness.md). A closed invocation gate is not
proof of native/HTTP cancellation or OS containment. Public output/usage,
successful-response boundaries, headless gadget callables and shutdown remain
separate implementation work.

See [request admission](request-admission.md),
[scope/output arguments](scope-output-arguments.md) and the accepted
[headless specification](headless-mode.md).
