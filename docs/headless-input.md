# Private headless instruction and file-input admission

**Implementation status: October 23, 2026 (UTC).** This is a private app-layer
boundary, not a shipped `craftsman exec` command. It is independent of pending provider
response PR #112 and does not claim that PR has merged or that its checks passed.
The accepted contract remains [headless-mode.md](headless-mode.md); current overall
status is [runtime-status.md](runtime-status.md).

## What is implemented

`exec::input::Sources` selects exactly one explicit instruction source: argument
bytes or a workspace instruction file. Attachments are data, never an alternative
instruction source or an authorization grant. Missing/conflicting sources,
malformed UTF-8 and empty/Unicode-whitespace-only instructions fail with fixed
`InputError` text. An unnamed stdin stream is never inspected. An argument exactly
`-` is recognized as the accepted explicit stdin form but the **old Invocation /
PreparedInput / FileInvocation routes still refuse it**, rather than reading
implicitly or sending a literal instruction. A separate private `StdinInvocation`
now selects native byte-pipe stdin with retained mode/handle ownership and bounded
read clocks before the same complete admission; see [named-stdin.md](named-stdin.md).
This is not public stdin/console/regular-file/flag support.

`construction::PreparedInput::prepare` admits input before durable session selection,
thread identity creation or writer opening. `input::Invocation::prepare` then
consumes that opaque pair through the existing `Runtime::build` and SAME shared
session-builder core. Its owned input can be run once. The returned private `Run`
retains the original runtime, source facts and success/error outcome, including
partial state on a failed turn; it is not a public result or safe transcript.

Existing private `Prepared::prepare` / `Runtime::build` remain configuration/runtime
foundations used by earlier adapters and fixtures. They do not acquire input
validation merely because this module exists. A future frontend must choose the
new input-first entry point explicitly, and must validate its other invocation
options/output destinations before any run or writer assignment.

## Source and resource bounds

| Item | Admission |
|---|---|
| Original instruction bytes | At most 1 MiB; a file's leading BOM counts before normalization |
| Each original attached file | At most 1 MiB |
| All original attachments | At most 8 MiB; repeated paths count as separate actual reads |
| Private attachment count | At most 128, including empty files |
| Each private path label | At most 4096 Unicode scalars / 16 KiB UTF-8; no empty or NUL-containing selection |
| Framed private message | Derived ceiling of 22,053,376 bytes; complete label/framing work is admitted before assembly |

Count/path/framing ceilings are additional private resource gates, not new shipped
CLI guarantees or substitutes for the accepted public output/schema bounds. Input
is refused whole, never silently clipped or repaired into a valid instruction.

Argument content preserves its original whitespace, Unicode and BOM data. An
instruction file may have its first leading UTF-8 BOM removed once; remaining
characters are content. Attached UTF-8 data is preserved exactly, including a BOM,
CRLFs, whitespace and empty files. Source facts count original bytes before any
preamble removal or framing, not Unicode scalars, rendered length or model tokens.
They have no public `Serialize` / raw `Debug` implementation and are not usage,
billing, grants, provider receipts or a complete audit ledger.

The existing private provider request boundary has its own stricter source/view/
encoded-body budgets. Admitting input within R2's raw byte bounds is **not** a promise
that a request of that size will pass those later budgets. Those budgets are not
expanded here to manufacture provider admission or token usage.

## Preparation ordering and retained authority

```text
borrowed explicit Sources
  -> bounded source grammar / literal UTF-8 / path and count admission
  -> existing read-only Bootstrap config, frozen credentials and permissions
       retained ConfinedFs workspace/config authority; bounded discovery
  -> selected instruction/attachment file reads through THAT SAME filesystem
       read_file deny policy; no-follow parents; regular/single-link files
       metadata/identity checks; per-file AND remaining aggregate byte limit
  -> complete UTF-8 normalization and bounded original-data message assembly
  -> existing session selection / optional fresh ID and empty thread-store row
  -> SAME shared-builder private Runtime; one admitted model-data turn
```

`ConfinedFs::read_input_raw` only tightens the existing authority: the limit cannot
exceed 1 MiB and may be lower to enforce the remaining attachment budget. Metadata
size is checked before retention, and an over-limit read discards its prefix.
The admitted input read must match the original file length as well as the shared
identity/size/mtime checks. Ordinary and guest raw reads retain their 10 MiB ceiling
and existing behavior; the new typed input-limit error is classified as a resource
limit, never string-inferred approval authority.

Explicit file selection does not override tool/path denies, config/credential
hard-denies, workspace confinement or file-type restrictions. Paths used for IO
remain original, not redacted aliases. A retained workspace handle is reused; the
adapter never checks a path and then reopens it through ambient `RealFs`.

Each file is an admitted snapshot, not a transaction over all attachments or a
portable external-writer CAS. Started blocking file/config work is not rolled back
or OS-contained. Source facts do not claim all-signal shutdown or native sandboxing.

## Model data, not command dispatch or public output

With no attachments, the validated instruction is passed unchanged as model user
data. With attachments, original data is appended in explicit data delimiters;
JSON-encoded headers preserve filename/source labels, observed byte lengths and
order. Label control characters cannot create a new header line. Framing is **not
protection against prompt injection**, and neither text nor labels can widen the
immutable tool gate. No slash-command, plugin-command or native-shell submission
route is selected by this entry point.

Raw admitted input and retained state stay private. After a turn, `Run::message_view`
now selects the actual runtime's separately bounded retained-message snapshot,
not the raw state clone. Other state/public/terminal/omission producers remain
unselected; see [retained-message-views.md](retained-message-views.md). A separate
`Run::message_selection` now consumes that admitted snapshot for exact whole-message
suffix bounds; original input/state/final answer/outcome remain unchanged. This is
not terminal/public output or a way to omit an unadmitted source; see
[message-selection.md](message-selection.md). `Run::terminal()` now consumes the actual returned
Run, closes execution and retains original outcome/runtime under a separate private
publication lease. It observes real typed stops without reopening the live latch;
public final output/IO/status remain separate. See
[terminal-publication.md](terminal-publication.md). Its separate private selector keeps the
actual checked final answer/facts while bounding only whole retained messages after
complete channel admission; original input/outcome/state and execution permissions
remain unchanged. See [terminal-message-selection.md](terminal-message-selection.md). A separate private
ASCII adapter (`terminal-ascii.md`) retains the actual output/facts, verifies full
projected channels and selects by escaped byte lengths; original admitted input,
state/permissions and public channel/IO gates are unchanged. Its separate private
human renderer (`terminal-human.md`) consumes only immutable actual final output
and fixed private-fact diagnostics: complete original redaction, visible controls/
runner defanging and exact LF-framed bounds stay under SAME lease/typed epoch.
Original input/state/execution and public outcome/text eligibility/IO stay unchanged;
no retained transcript or named stdin is decoded/selected through this path.
A separate `FileInvocation` route now preserves the old input APIs/Options and uses
complete same-bootstrap input plus read-only explicit external-file capture BEFORE
session creation. Its actual FileRun owns one checked terminal-file attempt, not a
caller Run/target pair or public stdout/schema writer; see [guarded-output.md](guarded-output.md).
Its directory-rename fixtures retain physical authority on both real movement and
exact native Windows refusal; they never close/loosen the production input roots.
Its content passes also count seek/payload/EOF inside the unchanged native-call cap;
input/source budgets and public writers remain unchanged.
Native cleanup and all early publication results recheck the same actual terminal
lease/epoch before classification, without dropping the original input Run.
Existing copied provider
requests select frozen-seed views before HTTP encoding, while original source data,
dispatch and captured authentication remain separate. No raw checkpoint/journal,
result/event/trace or other public producer is enabled. SDK materialization,
callable headless gadgets/catalog, MCP/LSP/memory/background tasks and frontend
feature defaults remain unchanged.

## Verification and remaining gates

The focused fixtures cover source exclusivity, exact byte limits, UTF-8/BOM/CRLF,
ordered and empty attachments, aggregate overflow, confined denials/types/links,
retained-root replacement, private label encoding and no-store/no-traffic failures.
Actual shared-builder/HTTP fixtures check literal model-only input, immutable input
snapshots/frozen auth, existing outgoing request redaction, and typed approval failure
with retained partial state.

The implemented-source input suite passed **30 fixtures with gadgets off and on**
(26 are platform-independent; four additional cases exercise Unix path/handle
behavior). The unchanged filesystem38, runtime42, provider47 and LLM10 suites passed.
Both production feature checks are warning-free. The reference126, benchmark-driver9,
formatting/whitespace/lineage, seven-document fence/fourteen-local-link and protected
source audits passed. No full local workspace/native retry was queued and no installed
CLI, real user configuration, live model or SDK materialization was used.

Initial compilation caught the new input-limit variant missing from the app's
exhaustive typed filesystem/gadget error maps; both now classify it as an existing
resource-limit kind, without changing older kinds or SDK/WIT. An initial focused run
had26 passed /2 failed because new fixtures assumed a non-contract selector prefix
and permitted control-bearing IO paths. Fixtures now use the accepted `path:` deny,
retain the existing control-path refusal, and separately test label JSON encoding.
These failed attempts are not reported as passes. No deny, assertion or source byte
limit was weakened. Hosted exact-final-head verification remains required; local
focused passes are not a completed workspace, Windows or public-command guarantee.

The separate private byte-pipe route now implements named stdin's accepted10s
first-byte/30s total-read clocks, EOF/UTF-8/cap checks and owned native lifecycle.
Complete console/regular-file/exclusive-public-front-end stdin support remains open. The full frontend/flag/output-path pipeline, public providers,
successful-response/alternative-state boundaries, compatible gadget provenance,
seeded storage/output/trace, #97 before the shared token meter, P2g shutdown and
live/default-on/release gates remain open. Framework/SDK/WIT/root dependencies/
lockfile/toolchain, ordinary frontends, schemas/frames/goldens/trace-v1 and the
historical October 16 baseline/Appendix A/target diagrams are unchanged.

Every partial implementation PR includes documentation. ALL FOUR exact-final-head CI
jobs and every mandatory step, including Windows, must pass before any
missing-review-only admin merge. #115 satisfied that gate on October 23: all four
jobs/mandatory steps passed (main Linux/macOS828/969, Windows805/935; zero failed),
with a fresh review/base/rules audit and identical CI-tested/squash-merged trees.
Only missing review was overridden. This input implementation does not authorize a
bypass of a failed check on this or another PR.

The first hosted native-stdin revision failed both Windows builds (12 failures
each); its earlier local passes are not cross-platform completion. The separate
native correction preserves real-stdin/deadline/source assertions, distinguishes
Pending from genuine EOF and verifies read/type/synchronous ownership without a
blocking-reader fallback. This document's existing boundary is unchanged; see
[named-stdin.md](named-stdin.md) for the preserved failures and fresh exact-head
Windows/other-platform merge gates. No public frontend/callable/meter is enabled.

A second Windows attempt still failed the five real-stdin cases in both builds.
Current native polling keeps original grants: owned NOWAIT where supported, or
unchanged-mode availability before read-only data/actual EOF probes. No blind
idle/generic/background reader, access upgrade or public OS/exclusive-input
claim is added; all earlier42 stdin assertions and this boundary stay intact.
The additive read-only cases and final-head gates are in `named-stdin.md`.

A separate private [stdin-file-output.md](stdin-file-output.md) route now admits
exact positional-`-` input with an ORIGINAL guarded destination captured before
stdin inspection and rechecked after whole input before IDs/store/providers.
Bootstrap/ConfinedFs/captured credentials, native clocks/limits and old attachments
are reused; existing argument/file/native bodies and source facts stay unchanged.
It returns the old paired file controller, never caller Run/target/bytes/status
pairing or public CLI/stdio/full flags. Late target refusal cannot rewind consumed
source bytes or claim external-writer/OS/cancellation/global-stdin guarantees.


## Private instruction argv selection and routing

The new [input-arguments.md](input-arguments.md) parser is only the explicit source/
attachment subset, not `craftsman exec` or full scope/permission/output/session/usage/
deadline flags. Whole bounded native argv/strict UTF8/arity/source/path grammar
precedes config/stdin/session/provider effects. Only exact positional `-` chooses
the old native factory; `--prompt-file -` stays a workspace filename and `--`
protects option-looking literal data. Repeated one-path split/joined `--file`
preserves original order/labels/duplicates; no shell/command expansion or aliases.

Only the existing four concrete factories own admitted input/runtime/targets;
new opaque wrappers return the EXISTING Run/FileRun, with unchanged partial state,
SAME gate, source/read clocks, file publication/cleanup and OS limitations. No
arbitrary runtime/Run/target/data/grants/seeds or post-run path/format pairing.
Private argv overhead ceilings do not raise source1MiB/attachments8MiB/path/file
bounds. Unknown full frontend flags refuse before effects, not accept-and-ignore.
Current source-local/every-step four-platform-job evidence must be collected
independently of #129's final composition passes; all old bodies/assertions remain
exact and public/R2/#97/P2g/gadget/trace/storage/live gates remain separate/off.


## Private control/source argv follow-up — October26 (Asia/Taipei)

The new [control-arguments.md](control-arguments.md) factories now combine ONLY
supported absolute workspace/config, trust, manual/plan/deny/fail, explicit
allow/deny/env-pass and mutually exclusive fresh/ephemeral/resume/continue controls
with the old original source/attachment argv grammar. Whole native bounds/strict
UTF8/syntax/old provisional selector and session-ID/scope admission precede IO.
Unsupported relative/ambiguous scope, output/trace/request/token/wall/gadget/full
flags refuse, never accept-and-ignore or guess ambient directories.

Explicit caller Defaults are raw absolute inputs, not physical authority. Old
Bootstrap remains authoritative for config/credential-source env denies, retained
ConfinedFs, selected sessions and actual all-role policy/gate; the preliminary
syntax policy is dropped. Original Options/source arguments are captured into old
ArgumentInvocation/ArgumentFileInvocation, returning existing Run/FileRun without
new caller Runtime/Run/target/data/seeds/gates/record/IO constructors or post-run
format/path switching. EVERY old body/assertion/reader/writer/codec/lease/limit/
fixture/SDK/WIT/framework/dependency/lock/pin/frontend/frame/schema/golden/trace
stays exact after narrow declarations/grammar-bound aliases. The initial58 normal
cases plus one argv-only ignored helper and locked production check passed on
source-local builds; no user config/SDK/global env mutation or raw history reset.
No public exec/full flags/stdin/stdio/R2/results/events/storage/trace/provenance/
headless gadgets/usage/#97/P2g/live/release enablement. Fresh all-four exact-head/
every-mandatory-step/both-mode/Windows evidence is a separate merge requirement.


## Private relative-scope/output-file argv follow-up — October29 (Asia/Taipei)

The separate [scope-output-arguments.md](scope-output-arguments.md) adapter resolves
ONLY bounded normal-relative cwd/config scopes against ORIGINAL explicit launch
Defaults, independent of option order. Task/output paths use the SELECTED workspace;
no ambient cwd/HOME/env/chdir, canonicalization or dot/parent/drive/UNC/device repair.
The old absolute control adapter remains unchanged and still refuses relative input.

One original `--output-file` becomes a typed pre-run Destination through SAME old
ControlFileInvocation. Physical workspace/config output still refuses, including
relative output; use an existing absolute external directory for this private
publisher. FileOptions selects only old private ASCII/human previews, NOT public
text/JSON/JSONL. Public format/trace/usage/gadget/wall/legacy flags refuse before
effects, never accept-and-ignore or rename preview data into public schema fields.
Source/attachment values and after-`--` tokens remain original task data.

Whole native/default bounds/UTF8 and complete new+old syntax precede effects.
Provisional old policy/destination selections are DROPPED; actual Bootstrap alone
revalidates configured credential sources and captures original auth/ConfinedFs/
source roots/session selection. Existing Run/FileRun, native rights/read clocks,
record/lease/epoch/create-only single-attempt publisher and OS caveats stay exact.
Only two narrow old input-module/preflight declarations are appended; ALL older
Rust bodies/tests/fixtures/hosts/limits and framework/SDK/WIT/deps/lock/pin/frontend/
SafeBackend/frames/schemas/goldens/trace/corpus remain unchanged.

The first locked production check passed warning-free. The first NEW fixture
compile recorded an accessor typo/unused NEW import, corrected only in new tests
with actual source-before-store coverage; its log is preserved, not called green.
Isolated grammar/runtime/native fixtures retain original input/request history,
without real user config/SDK/global env mutation. Unmarked/unknown helper scenarios
return before fixture/config/stdin work. Current-source bounded checks/links and
ALL FOUR fresh exact-head jobs/EVERY mandatory step/BOTH modes/Windows independently
gate merge; no old assertions/deadlines/process workaround or unchanged full-CI retry.
Public exec/full parent/ambiguous paths/stdin/stdio/R2/results/events/storage/trace/
provenance/headless gadgets/shared usage/#97/P2g/live/default-on/release stay open/off.


Collected current-source evidence: all **34 bounded local rows** passed warning-free,
including new60 scope/output cases and unchanged58 controls/58 source argv/48 native/
42 composition cases in BOTH modes, old37 guest cases locally (35 compiled on Windows),
every selected older suite, both locked production checks, actual marked/unmarked
existing compile probes, fixture/root fmt/reference126/driver9/whitespace/lineage.
The exact17-path source audit keeps EVERY older Rust body/assertion unchanged after
TWO narrow append-only module/preflight declarations; all91 local Markdown links and
fences pass. The first NEW accessor/import compile failure remains preserved, not
passing evidence; corrected60 and final34 source rows are independent. This is not
fresh all-platform CI, public exec/full path/format/R2/usage/live/OS/release approval.
ALL FOUR fresh exact-final-head jobs/EVERY mandatory step/BOTH modes/Windows remain
independent prerequisites before a missing-review-only admin merge.
