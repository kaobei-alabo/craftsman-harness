# Private instruction argv parsing and routing

**Status: October 25, 2026 (Asia/Taipei).** This incomplete app-layer slice adds
`ArgumentInvocation` / `ArgumentFileInvocation`, not a `craftsman exec` subcommand,
public flag parser, stdout/stderr/result/events/usage/trace/storage/gadget producer.
Only the instruction/attachment subset is parsed; all unsupported full flags
refuse before effects rather than being accepted and ignored.

## Exact private subset

```text
[PROMPT | - | --prompt-file PATH | --prompt-file=PATH]
[--file PATH | --file=PATH]... [-- PROMPT_OR_MINUS]
```

There must be exactly one instruction. Each `--file` occurrence selects ONE path;
repeat it for more attachments. Options may surround the positional instruction
until `--`; tokens after `--` are positional data. Two positional instructions,
repeated `--prompt-file`, or file-plus-positional instructions refuse. Missing
split values and empty/NUL file paths refuse. Split option-looking file values
require the joined spelling (`--prompt-file=-name`, `--file=--name`); a single
`--prompt-file -` or `--file -` is an explicit workspace filename, NOT stdin.

Only exact positional `-` selects native stdin, including after `--`. A spaced
`" - "` stays literal. Original Unicode/CRLF/BOM/text/space/quote/equals/backslash/
source path spellings and duplicate attachment order are preserved as data.
There is no shell tokenization, slash-command dispatch, Unicode/path repair,
redacted alias substitution or permission grant. Physical path/read policy still
belongs to the existing retained ConfinedFs, not this grammar validator.

Unknown/abbreviated/legacy options and full `--cwd`/`--config-dir`/permission/
session/output/trace/usage/gadget/wall flags return fixed typed errors BEFORE any
config/stdin inspection, session ID/store or provider effect. No help/error printer,
TTY/CI/environment default or implicit stdin read is introduced. Explicit Options
and guarded Destination are still original typed pre-run parameters, not their
public flag forms. Private HumanPreview/AsciiFragment are NOT public text/JSON
formats and no JSONL/16MiB/public schema output is promised here.

## Same construction, source and publication owners

```text
bounded original native argv -> complete subset/source/count/path grammar
  -> explicit ORIGINAL Options (+ ORIGINAL pre-run Destination for file route)
     literal / prompt-file -> SAME Invocation or FileInvocation
     exact positional '-'  -> SAME StdinInvocation or StdinFileInvocation
  -> SAME Bootstrap/captured credentials/ConfinedFs/source roots
  -> SAME old whole source admission / runtime shared builder
  -> existing Run or FileRun -> existing actual Terminal/FileTerminal/Record
```

New wrappers have opaque fields and only explicit preparation constructors;
consumption returns EXISTING Run/FileRun rather than another raw state/status/IO
wrapper. No arbitrary Prepared/Runtime/Run/target/data/gates/seeds/bytes pairing,
post-run destination/format change, new generic reader/writer or authority is added.
The old constructors recheck their unchanged source grammar and capture owned
input; borrowed argv is never reloaded after preparation. Failed turns retain
original partial outcome/state and typed approval epoch, not fabricated completion.

All older Rust bodies/assertions remain exact after one narrow input-module
append. Native pipe rights/mode strategy/read clocks/EOF/restore/close and source
caps are unchanged ([named-stdin.md](named-stdin.md)); guarded output uses the same
pre-run handles, SAME epoch, one create-only attempt and owned cleanup
([guarded-output.md](guarded-output.md)). Native/file target capture still precedes
stdin and its second retained check precedes sessions
([stdin-file-output.md](stdin-file-output.md)); regular input/file construction
ordering also remains its original ordering. No namespace/content CAS, global
stdin exclusion, absolute native-call interruptibility, Windows ACL/durability,
all-role/process quiescence or P2g guarantee follows from argv validation.

## Derived private resource bounds

Before strict UTF8 decoding, borrowed path retention or source copies:

- at most **260 native argv entries** (`2 * 128 + 4`);
- at most **1,048,592 native encoded bytes/token** (`1 MiB + 16`);
- at most **3,146,816 total native encoded bytes**
  (`1 MiB + 128 * (16 KiB + 8) + 64`).

These overhead ceilings do NOT raise source/label bounds: instructions remain
original1MiB, selected paths128, each4096 scalars/16KiB; actual files remain1MiB
and attachments8MiB total under the unchanged complete old admission. Oversize/
invalid UTF8/selection refuses, never truncates, lossily decodes or repairs. Argv
lengths are bounded syntax observations, NOT model token usage, run facts, billing,
source receipts or public metadata. No raw argv/path/invocation Debug/Serialize/
Clone or new sink is exposed; error text stays fixed `[REDACTED]`.

## Preserved attempts and native evidence

The initial production check completed with one NEW private-interface warning
(SelectionError visibility); only that safe enum's visibility was matched, not
suppressed. The initial57 normal cases compiled/passed with one unused NEW
fixture PreparationError import warning. Added bootstrap coverage now uses that
import and raises the inventory to58 normal plus one ignored helper. The NEW
native helper was strengthened to preserve BOTH actual literal and named-stdin
provider requests without any mock/history reset, masked request or invented zero
baseline. Current-source evidence is independent of those first attempts.

The argv-marked ignored helper returns BEFORE config/stdin inspection when
unmarked. Marked native subprocesses exercise real inherited stdin through the
production old memory/file chains, invalid flag/source/target no-consumption and
an unnamed pipe left available after actual literal execution. Only isolated
pipes/config, synthetic local providers and original observed bytes/requests are
used; no real user config/SDK/guest or global environment mutation. Source syntax,
UTF8/byte/path/count limits, original labels/BOM/CRLF, actual bootstrap failure,
partial typed approval/provider error, file create-only/single-attempt and owned
source-after-preparation behavior are covered without new production hooks.

#129's composition foundation merged as87b357d after ALL FOUR corrected-head
jobs/EVERY mandatory step in CI40155545193 and exact tested/merged tree/sole-parent
proof. It is historical to this new source, not current-source/full-frontend CI.
All earlier failed heads/warnings remain recorded. This slice requires fresh
bounded local/source-link/protected audits and ALL FOUR exact-final-head jobs AND
EVERY mandatory step, both feature builds/Windows included, before any missing-
review-only admin merge. Old shell/reader/writer/test/deadline behavior may not be
weakened or retried unchanged to manufacture green.

Public exec/full frontend flags/console/file stdin/stdout/stderr/R2/results/events/
redacted storage/trace/compatible provenance/headless gadget callables/catalog/
shared usage/#97/P2g/live/default-on/release remain independent/open. Framework/
SDK/WIT/dependencies/lock/Rust pin/frontends/hosts/frames/schemas/goldens/corpus/
trace stay unchanged; documentation accompanies every partial implementation.

Collected current-source evidence: all26 bounded local rows passed warning-free,
including new58, unchanged composition42 and native48 normal cases in EACH feature
mode, every selected older suite, both locked production checks, reference126/
driver9/fmt/whitespace/lineage. Exact14-path protected audit keeps older bodies
unchanged after one input-module append;68 local Markdown links/fences pass.
This is local/synthetic source evidence, not a substitute for ALL FOUR fresh
exact-final-head jobs/EVERY mandatory step, both Windows feature builds included.


## Guest fixture load follow-up — October25 (Asia/Taipei)

PR #130's first CI40165049917 at ed0c2c9 stayed unmerged: fmt/macOS/Windows
passed but Linux gadget main1441/2failed/6ignored. All58 new argv/old48 native/
old42 composition cases passed BOTH modes/EACH OS; the two unchanged guest tests
observed loading Timeout instead of reaching their denial/post-instantiation
assertions. The exact failure logs/tested tree are preserved, not waived/retried.

The new focused SOURCE input retains actual dynamic binary read/write, original
spin/WASIp2 sleep/entered logs and all malicious describe probes. Only those TWO
old tests choose it; their complete assertions and original30s load/default,
1s post-instantiation and5s outer budgets stay exact. Broad19-function source,
SDK/WIT, production host/compiler/cache/limits and native reader/publisher bodies
remain unchanged. Coordinated source builds happen before guest budgets; actual
Wasmtime load/approval/instantiate/IO still use the SAME existing operation path.
New fixture-only size/LTO settings are not a runtime optimization/approval/cache.
See [guest-fixture-loads.md](guest-fixture-loads.md) for first attempts, advisory
local141598-vs340104 byte/137-vs360ms load observations and truthful limitations.
New exact export/describe-denial/dynamic256-byte roundtrip checks do not invent
status/bytes/zero counts or skip failure; only the NEW sorted-export expectation
was corrected after its first failure. Fresh bounded verification/ALL FOUR exact-
final-head jobs/EVERY mandatory step still gate merge, Windows included. Public
exec/wholeflags/stdin/stdio/R2/results/events/storage/trace/provenance/gadgets/
usage/#97/P2g/live/release remain separate/off.


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
