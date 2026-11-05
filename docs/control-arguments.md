# Private supported control/source argv composition

**Status: October 26, 2026 (Asia/Taipei).** `ControlInvocation` and
`ControlFileInvocation` are private app-layer factories, NOT a `craftsman exec`
subcommand, public CLI rollout, output/result/event/usage/trace/gadget producer.
They compose the supported source subset with explicit scope/permission/session
controls and return the EXISTING Run/FileRun through the old argv factories.

## Supported private controls

- `--cwd PATH`, `--config-dir PATH` (split/joined): ORIGINAL absolute bounded
  scopes only. Explicit caller defaults are required; no cwd/HOME/env fallback,
  chdir or config mutation. Relative/dot/parent/UNC/device/invalid UTF8/control/
  over-depth spellings refuse BEFORE IO, not normalize into a different root.
  Relative/ambiguous full frontend scope resolution remains a separate gate.
- `--trust-workspace`: default false; one bare occurrence selects ONLY the
  existing trusted-project discovery. It never creates a tool/component grant.
- `--permission-mode manual|plan`, `--on-unapproved deny|fail`: exact old enum
  grammar, defaults manual/deny. Legacy broad modes and auto-approve aliases
  refuse; repeated scalar/boolean controls refuse instead of last-wins merging.
- Repeated split/joined `--allow`, `--deny`, `--env-pass`: ORIGINAL order/values,
  exact old selector/name/platform/conventional-credential grammar. Deny wins,
  plan cannot permit writes, stored/ambient broad modes remain ignored.
- `--ephemeral`, `--session ID`, `--continue`: mutually exclusive. No selector
  retains old fresh durable behavior. Session ID grammar is the old128-byte
  ASCII identifier grammar, including permitted dot characters; this does not
  imply path ownership or existence. Actual read-only catalog/schema/canonical
  primary-scope validation and deterministic updated-desc/id-asc selection remain
  solely in the unchanged session constructor. Missing/mismatched sessions refuse.

Source handling stays positional PROMPT/exact `-`, split/joined `--prompt-file`,
repeated ONE-path `--file` and `--` end-of-options, with all original source/count/
UTF8/path/framing bounds. Controls after `--` are literal source data, not grants;
a `--prompt-file -` value remains a workspace filename. Only exact positional `-`
selects native stdin. Original labels/Unicode/quotes/equals/BOM/CRLF and duplicates
remain original task data, never shell/slash dispatch or executable aliases.

Output-format/file/trace, request/token/wall budgets, gadget grants and all other
unsupported/full flags REFUSE before config/stdin/IDs/store/provider effects;
they are never accepted and ignored. The file route still takes ORIGINAL typed
Destination BEFORE preparation, not public text/JSON/JSONL names or a post-run
path/format switch. No stdout/stderr/TTY printer, flags from repository config,
model-controlled expansion, implicit stdin or public status/usage/receipt.

## Admission and actual ownership

```text
whole original native encoded count/token/total preflight
  -> complete supported control + old source grammar, old provisional policy syntax
  -> original selected absolute scope grammar + old session-ID grammar
  -> owned explicit Options + untouched source-argv projection
  -> SAME old ArgumentInvocation / ArgumentFileInvocation
  -> SAME Bootstrap config/credential-source/policy/ConfinedFs/session/runtime owners
  -> SAME original input bytes/facts -> existing Run / FileRun / terminal/publisher
```

The preliminary old policy object is grammar-only and discarded. It cannot be
used for IO or retained as execution authority: old Bootstrap revalidates ORIGINAL
selectors against configured captured credential-source names before source reads,
then captures frozen credentials, physical roots and the actual all-role gate.
A harmless-looking `${NAME}` auth source passed via `--env-pass NAME` is still
denied there BEFORE input/store/provider effects. No raw gate/seeds/Runtime/Run/
Prepared/target/data/status constructor or resume/permission bypass is added.
Source/controls are captured before preparation; later argv/file/config mutation
cannot replace the owned admitted source or widen its actual policy.

Private resource preflight derives from EXISTING bounds:256 rules/direction,
4096 rule scalars (16384 native byte ceiling),64 env names/128 bytes; original128
selected file paths,4096-scalar/16KiB labels and1MiB instruction bytes. Combined
native argv is at most1428 entries,1048608 bytes/token and11581664 total encoded
bytes, checked BEFORE strict UTF8 decoding/copies. These overhead limits do NOT
raise original source, selector, file/read/native clock, result or call budgets.
No raw argv/default-scope/invocation Debug/Serialize/Clone; fixed error Display is
`[REDACTED]`. Typed error values contain no caller paths/rules/credentials/OS text.

## Collected evidence and remaining gates

The first locked source-only production check passed warning-free. The initial58
normal cases plus one argv-only ignored helper passed on the local source build;
marked subprocesses exercise real native stdin through both old invocation chains,
invalid control/policy/session/scope/target/credential no-consumption and original
literal option-looking input followed by untouched named pipe input. Both actual
provider requests remain inspected without any history/mock reset or fake zero.
Unmarked helper returns BEFORE fixture/config/stdin inspection. Only isolated
config/pipes/synthetic providers; no real user config/guest/SDK/env mutation.

Real runtime tests also prove actual selected workspace/config, trust discovery,
manual/plan/deny/fail and env policy, captured original source/BOM/duplicate labels,
old fresh/resume/continue/canonical primary scope, typed partial approval and paired
ASCII publication/single attempt. They are not live quality/full flags/default-on.
The existing scalar/session/permission/source logic is reused through append-only
helpers; ALL older Rust bodies/tests/native readers/write/record/codec/lease/seal/
limits and SDK/WIT/framework/deps/lock/pin/frontends/hosts/frames/schemas/goldens/
trace/corpus stay exact. #130's final argv/guest CI is historical, not this slice's
fresh exact-final-head all-platform evidence. Every partial slice updates docs.

Current source-local/links/protected checks and ALL FOUR fresh exact-final-head
jobs AND EVERY mandatory step, BOTH builds and Windows included, precede any
missing-review-only admin merge. No old assertion/deadline/process/source fixture
workaround, unchanged full-CI retry or independently blocked provider correction.
Public exec/full relative scope/output flags/stdin/stdio/R2/results/events/redacted
storage/trace/compatible provenance/headless gadget callables/catalog/shared usage/
#97/P2g/live/default-on/release remain independent/open. Same native/shared-reader/
namespace/Windows ACL/source-root/OS-call/cancellation limitations remain; grammar
admission is NOT an external writer CAS, OS confinement or shutdown guarantee.

Collected current-source evidence: all32 bounded local rows passed warning-free,
including new58 controls and old58 source argv/48 native/42 composition in BOTH
modes, old37 guest tests locally (35 compiled on Windows), every selected prior
suite, both locked production checks, marked/unmarked existing compile probes,
fixture/root fmt/reference126/driver9/whitespace/lineage. The exact17-path audit
keeps EVERY older Rust body/test/fixture/host/read/write/codec/limit unchanged after
four narrow append-only declarations/old grammar helpers;78 local links/fences
pass. This is source-local/synthetic evidence, not fresh all-platform CI or a
public/default-on/live/usage/source-OS guarantee. ALL FOUR exact-final-head jobs
AND EVERY mandatory step/both modes/Windows still independently gate merge.


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
