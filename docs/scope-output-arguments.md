# Private relative-scope and output-file argv admission

**Status: October 29, 2026 (Asia/Taipei).** `ScopedInvocation` and
`ScopedFileInvocation` are private app-layer adapters, not a public `craftsman exec`
command, complete path resolver, public output-format producer or release gate.
They resolve a deliberately bounded path subset and consume the SAME existing
`ControlInvocation` / `ControlFileInvocation`, returning the existing Run/FileRun.

## Explicit scope resolution, not ambient authority

Both original caller Defaults must be absolute, bounded scopes. The original
`Defaults.workspace` is the launch/default workspace input. Normal relative
`--cwd` and `--config-dir` values are resolved against THAT original explicit
base, independently of option order; an absolute override retains its original
spelling. Without an override, the original default stays unchanged.

This does not read or mutate cwd, HOME, configuration, environment or a store.
There is no `chdir`, canonicalization, symlink-following or alias repair in this
adapter. Raw native append preserves admitted components, including a Windows
verbatim disk base; it is not `PathBuf::join/push` removing a component before
validation. Windows verbatim concatenation must remain an admitted native
component spelling, not silently normalize a non-native separator.

Only normal relative components are supported here. Empty, dot/parent,
root-only/drive-relative/UNC/device/foreign-drive, control, invalid UTF-8,
over-depth or over-bound spellings refuse before effects. Colon/backslash aliases
inside a normal component are refused; native Windows separators are parsed as
separators. Ambiguous/parent-containing full frontend resolution remains separate.
The original absolute-only control adapter still refuses relative paths.

Relative **task** paths still use the SELECTED workspace through the unchanged
instruction/attachment and ConfinedFs owners. A launch-directory distractor
cannot replace a selected-workspace prompt or attachment. Config remains governed
by the selected config scope for config, skills, credentials and session storage.
Defaults/resolved strings are raw pre-run input, NEVER retained IO authority.

## Output-file selection is not public output-format implementation

The file adapter requires one split/joined `--output-file PATH`. The memory
adapter refuses that flag rather than accepting and ignoring it. Duplicate,
missing/empty/directory-only/control/dot/parent/over-bound paths refuse. A final
single name (including `-`) is a filename, not stdout or an implicit stdin marker.

Relative output paths use the SELECTED workspace, independently of option order.
The current guarded publisher still hard-denies physical workspace/config
roots and descendants: resolving a relative path does NOT grant publication
there. For a usable private file route, choose an absolute destination in an
existing external directory. Inside-workspace output and parent-containing
relative output remain separate gates. Existing, symlinked, missing or rebound
unsafe targets retain the original read-only capture/refusal semantics.

`FileOptions { format, max_bytes }` is explicit typed input BEFORE preparation,
not a format flag, raw record/writer/status/target/gate/seed constructor or post-run
switch. It can select ONLY existing private `AsciiFragment` / `HumanPreview`.
The old complete pure Destination grammar is reused and its provisional selection
is dropped; the SAME old concrete file factory later captures original physical
roots, identities, absence, budget and one-attempt create-only publication.

**`--output-format text|json|jsonl` remains unsupported and refuses.** Private
fragments/previews are not relabeled as public text eligibility, exec-result JSON
or JSONL events. Trace, request/token/wall/gadget/legacy/other unimplemented flags
also refuse before effects; none are accepted and ignored. Source/attachment
values and everything after `--` remain original data, never rescanned as controls.

## Admission and ownership order

```text
whole native count/token/total + explicit-default byte bounds
  -> strict original UTF-8 + complete scope/output flag structure
  -> bounded normal-relative resolution, untouched source/control-value projection
  -> SAME old complete source/permission/session/absolute-scope syntax (DROPPED)
  -> SAME old complete private Destination syntax (DROPPED, when file-selected)
  -> SAME ControlInvocation / ControlFileInvocation and original typed Destination
  -> SAME Bootstrap/captured credentials/ConfinedFs/source roots/session owners
  -> SAME input/native/file preparation -> existing Run/FileRun/terminal/publisher
```

The provisional policy/selection is not cached as authority. Bootstrap revalidates
ORIGINAL configured credential-source env names before input and captures actual
auth; it alone retains physical workspace/config authority and old canonical
session/schema/primary-scope selection. Native file output capture still precedes
stdin inspection, with retained-target revalidation after full input admission
and before IDs/store/providers. No source rewind, new reader/writer, independent
runtime, caller Run/Prepared/target/data/seeds/gates or after-run format/path switch.

Resources derive from EXISTING bounds: at most **1430 argv entries**, **1048608
native bytes/token**, **11598320 total native argv bytes**. Only one output-file
path/two entries are additional. A full output spelling has at most 16640 UTF-8
bytes / 4352 scalars; the old directory 16 KiB / 4096 scalar / 128-component,
255-byte name and private 1 MiB publication bounds still independently apply.
Original 1 MiB instructions, 128 attachments, 8 MiB attachment total, selector/ID
limits and native first-byte/total/read-call budgets are NOT increased.

Errors contain only fixed typed facts; Display is `[REDACTED]`. No raw argv,
scopes, credentials or invocation Debug/Serialize/Clone surface is added.

## Evidence and remaining boundaries

The first locked production check passed warning-free. The first NEW test compile
recorded an accessor-name typo and one unused NEW fixture import; these are fixed
only in new fixture/accessor coverage, not by editing old tests, suppressing
warnings or changing production limits. That initial log/source snapshot remains
historical evidence, not a passing run.

New isolated grammar/runtime/native fixtures cover relative workspace/config
selection and order independence; original source/label/BOM/CRLF/data; configured
credential denial; trust without grants; plan/deny/fail; fresh/resume/continue;
original pre-run path/format capture; guarded ASCII/human and typed partial approval;
public-format rejection and native no-consumption on invalid scopes/policy/session/
output/credential sources. Marked real subprocesses preserve BOTH actual original
provider requests without reset, masking or invented zero baselines. Unmarked or
unknown-scenario helpers return BEFORE fixture/config/stdin work. No user config,
SDK, guest, real credentials or global environment mutation.

All older bodies/tests/readers/native rights/modes/EOF/restoration/clocks, guarded
record/publication/cleanup/lease/epoch/limits and guest fixtures/hosts remain exact
after two narrow append-only input-module/preflight declarations. Framework,
SDK/WIT, dependencies/lock/Rust pin, frontend defaults/SafeBackend, schemas/frames/
goldens/trace/corpus remain unchanged. Full fresh exact-head CI and every mandatory
step in BOTH feature modes on Linux/macOS/Windows independently gate merge; only
a missing review may receive a user-authorized admin override.

Public exec/full parent/ambiguous scope/stdin/stdio/R2/results/events/redacted
persistence/trace/compatible provenance/headless gadget callables/catalog/shared
usage/#97/P2g/live/default-on/release remain separate and open. Grammar/resolution
is not namespace/content CAS, external-writer rollback, native OS containment,
absolute native IO deadlines or all-role/process shutdown.


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
