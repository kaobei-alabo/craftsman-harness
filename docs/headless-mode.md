# Headless mode — accepted contract v1

> **Identity migration (November 4, 2026):** names/paths in this document use
> current Craftsman identifiers. Historical dates, hashes, counts, accepted
> semantics and failure outcomes remain original evidence, not a new run or
> proof of release. The schema/ACP/WIT namespace change is breaking; see
> [repository-name.md](repository-name.md).

> **Status:** R1–R7 and the result/event schemas explicitly accepted by the
> user/maintainer in this development thread on October 20, 2026 (section 10).
> This is an accepted specification, not a shipped command. The current unattended
> entry point remains `craftsman -m`.
> **Implementation gate:** merge this accepted specification after green CI before
> implementing P2a. Acceptance does not waive implementation or live-evaluation gates.
> **Plan:** `docs/craftsman-harness-plan.md` sections 5.4, 5.5, 5.9 and P2a/P2g.
> **Compatibility:** trace v1 is adopted unchanged; desktop protocol v4 is frozen.
> **Current architecture/status:** `runtime-status.md`; sections 11 onward track
> private implementation without claiming that `craftsman exec` is available.
> **November1 contract update:** #97's optional token-admission representation and
> v1 compatibility judgment are explicitly accepted in `token-admission.md`.
> Earlier slice references to that then-open prerequisite are historical gate
> descriptions, not a still-pending decision. The actual builder now selects a
> private shared request-only default cap (`request-admission.md`), plus a separate
> private atomic token-reservation opt-in (`token-reservations.md`). Usage/receipts/
> settlement/public producers remain unwired. Private send/header correlation now
> exists (`admission-receipts.md`), not measured usage or a public ledger.

## 1. Scope and delivery boundaries

`craftsman exec` is one predictable, noninteractive run of the local workspace
harness. TTY presence, CI variables and ambient approval settings must not
change its permission or output contract. Exit 0 means the harness completed
its turn; correctness of a coding task is separately established by receipts
or the scoreboard's deterministic grader, not assumed from fluent final text.

P2a implements input/output, explicit-only permission construction, workspace
confinement, round/token limits, aggregate usage, redaction, trace v1,
configuration isolation and ephemeral/persistent session selection. P2g adds
wall deadlines, cancellation, signals and final-result shutdown ordering on
P2c's cancellation work. The OS shell sandbox is P2f, not filesystem-tool
confinement. Verification receipts/stuck detection are P4c; content-block
storage is P6c. Do not claim those later guarantees in the first P2a build.

`-m` keeps its legacy invocation and exits, except its three mandated fixes:
non-TTY prompts deny instead of approving or blocking; subagent/compaction
output does not contaminate stdout; a broken stdout pipe ends quietly rather
than panicking. Both frontends share `build_session_ext`, with an explicit
execution profile rather than ad hoc TTY checks.

## 2. Input and configuration

Accepted grammar (options ship only when implemented):

```text
craftsman exec [PROMPT | -] [--prompt-file PATH] [--file PATH ...]
  [--cwd DIR] [--config-dir DIR] [--trust-workspace]
  [--output-format text|json|jsonl] [--output-file PATH]
  [--permission-mode MODE] [--allow RULE ...] [--deny RULE ...]
  [--on-unapproved deny|fail] [--env-pass NAME ...]
  [--max-rounds N] [--token-budget N] [--timeout SECONDS]
  [--ephemeral | --session ID | --continue]
  [--allow-gadget ID ...] [--trace-file PATH]
```

- Exactly one instruction source: a positional prompt, `--prompt-file`, or the
  explicit positional `-`. An unnamed stdin pipe is never consumed. Conflicting
  sources, malformed UTF-8 and empty/whitespace-only instructions are usage
  errors before provider traffic. File input may begin with one UTF-8 BOM.
- **Accepted input bounds:** 1 MiB instruction bytes, 1 MiB per attached file,
  8 MiB total attachments; reject oversized input, never truncate silently.
  Named stdin has a 10 s first-byte and 30 s total-read deadline. A single byte
  followed by an idle, open pipe must not evade the deadline. R2 fixes these accepted bounds; they are not existing CLI options.
- `--file` contents have explicit filename/source labels and data delimiters.
  Framing is not protection against prompt injection. The same path/type/read
  bounds apply; FIFOs, devices and dangling links are not instruction files.
- Resolve `--cwd` before project discovery and before session construction.
  Relative task paths are interpreted against that workspace, not the caller's
  ambient directory. `--config-dir` must govern config, user skills and storage
  consistently; `${ENV_VAR}` resolution follows the existing token-configured
  platform design. Do not dump resolved config or credentials into diagnostics.
- Without `--trust-workspace`, do not load project `.craftsman/AGENTS.md`, skills,
  plugins or gadget declarations. User configuration is not project trust.
  Headless v1 does not enable MCP, LSP or durable memory; the result records the
  effective feature set rather than promising unavailable tools.

## 3. Permissions without a human

Construct a fresh invocation permission state. Ignore ambient `permission_mode`,
`auto_approve`, shell allow lists and persisted broad session "always" switches,
with a fixed warning that does not disclose their values. The accepted default
is `manual` plus `--on-unapproved deny`: an unapproved call returns a denied tool
result so the model can choose a permitted path. `fail` seals the partial turn
and returns exit 4. `ask_user` returns an error result, never reads stdin or
waits for a GUI callback, even when a terminal is attached.

Permissions come only from explicit invocation selectors. Deny wins over allow;
unknown/invalid selectors fail before a run. The accepted selector grammar and
protected-path overrides are in R1/R3. They are not inferred from today's permissive
first-token legacy allow list. Requested tools in delegated agents remain
wrapped in the same permission state; `task`/`send_message` recursion stays
filtered. Delegation, compaction or a later model-driven mode change cannot add
capabilities beyond the explicit invocation policy.

For plain-token `execute` rules, parse tokens before matching. Shell operators,
substitution, redirection, newline chaining and ambiguous command syntax do not
match a plain-token rule. A `git` rule also rejects writing options such as
`--output`, including joined and split forms; "git log" is not categorically
read-only. General command classification remains fail-closed on unsupported
shells. An explicit broad execute allowance is an acknowledged host-operation
permission, not an OS sandbox.

Gadgets retain their source/hash/capability first-load approval. Bypass mode
alone never approves a component, including `describe`. `--allow-gadget` is
invocation-scoped; project components also need `--trust-workspace`. Any reuse
of a source-bound user grant must be recorded with its origin. Rebuilds may
inherit only equal/narrower capabilities. Host writes still pass the ordinary
per-write gate. The agent cannot self-grant by editing grant/config files.

## 4. Workspace confinement and honest containment

A `ConfinedFs` wrapper covers read, write, edit, ls, glob and grep; `--cwd` is the
root. Relative and absolute paths resolve to that root's capability operations.
On Unix, walk/open component-relative to a root fd with `O_NOFOLLOW`; reject
symlink escapes, dangling paths and parent-directory swap races. Listings/globs
cannot reintroduce an out-of-root path. Creation checks every existing parent,
not just a lexical prefix. Windows is explicitly best-effort until P2d's safe
write/confinement work; tests pin its supported behavior rather than claiming
Unix-strength protection.

Protected writes include `.git/`, `.craftsman/`, CI configurations, build scripts,
`.env*` and the selected config directory. A broad tool allow must not silently
permit them. R3 hard-denies config/credential access and `.git`/`.craftsman` writes;
only its overridable CI/build inputs accept an exact-path allow plus tool permission.

In-process path checks do **not** contain `execute` or native Cargo build
scripts. Before P2f, record shell containment as unavailable/off. Every live
scoreboard run therefore requires a throwaway container/VM and its scoped
mount/network/key policy; never substitute an unrestricted developer-host run
with real credentials. No release may advertise filesystem-tool confinement
as a general OS containment boundary.

## 5. Limits, usage and run identity

- Accepted shared request cap: 50 logical calls across main/subagent/compaction
  (R5). `--max-rounds` and token budgets are positive bounded integers; invalid/
  overflowing values are usage errors. R5 replaces the original main-only wording
  with shared all-role admission; `rounds` separately counts main requests.
- One meter covers main, delegated and compaction providers. Use provider
  totals when supplied; otherwise count with the existing tokenizer and mark
  usage as estimated. Do not interpret missing usage as zero or permit it to
  evade `--token-budget`. The ledger and trace identify the source per call.
  Cache/reasoning details are optional when the provider supplies them.
- Enforce remaining token admission before another request; clamp requested
  output to the remaining allowance and account for streaming/partial results.
  Estimation error and failed retry-attempt billing must be explicit. No price
  table or dollar cap is introduced into Craftsman; the runner owns cost conversion.
- `--timeout` defaults to 1800 s, but its implementation and interrupted exits
  are P2g. It bounds an in-flight shell call by the remaining run budget and
  preserves the partial transcript. A build must not accept and then silently
  ignore an unsupported wall-deadline option during staged P2a delivery.
- Run ids contain sufficient random bits to prevent cross-container collisions.
  A new run is distinct from its conversation thread. `--ephemeral` creates no
  thread DB, durable memory, snapshots or journal; its thread id is null. It
  still emits a best-effort trace outside the agent's workspace.
- `--session` resumes the named thread, and `--continue` selects a deterministic
  documented existing thread. They are incompatible with `--ephemeral`. The
  accepted selection rule and context mismatch behavior are fixed in R6.

## 6. Output and redaction

`--output-format text` is the accepted default. Text emits only the final answer
for exits 0/1; other exits put a safe diagnostic on stderr. JSON emits exactly
one result object. JSONL emits versioned run/message/tool/result events, ending
with exactly one result. Stderr is human-oriented and not a parseable contract.
No progress/token/child-provider output may escape into JSON/JSONL stdout.

The result id is **`craftsman-harness/exec-result@1`**. Accepted always-present keys are:

| Key | Accepted value |
|---|---|
| `schema`, `run_id`, `thread_id` | result id, opaque run id, thread id or null |
| `status`, `exit_code` | structured outcome and matching process exit |
| `stop_reason` | standard ACP stop reason for a completed prompt, otherwise null |
| `_meta.craftsman` | detailed reason, retryability and limit information; never a new ACP stop-reason dialect |
| `output`, `output_truncated`, `messages`, `message_scope`, `messages_omitted` | bounded final text and explicitly retained/redacted state |
| `usage` | aggregate totals, estimation/source markers and role ledger |
| `rounds`, `wall_ms`, `limits`, `session` | main requests, run-clock duration, shared admission bounds and persistence mode |
| `workspace`, `features`, `permissions` | redacted effective scope/settings, never credential values |
| `sandbox` | truthful containment/support status, including unavailable shell containment |
| `verification` | `not_supported` until a reviewed implemented verification gate supplies receipts |
| `trace` | trace path/status or a safe reason it was disabled |
| `error` | typed safe error or null, with retryability where known |

Exact accepted nested types and event-schema id are in the R7 schemas.
Implementation must satisfy the schemas and conformance vectors; field additions are additive,
while renames, unit/type changes or newly required fields need a new schema id.
A consumer ignores unknown fields/events. JSONL uses the plan 5.4 envelope
(`schema`, `seq`, `ts`, `run_id`, `thread_id`, agent path) and ACP-compatible
vocabulary from its first release. It is not trace v1 and is not a dump of
whatever internal enum happens to exist. P6a later changes its producer, not
its external vocabulary. Failed requests are errors, not invented stop reasons.

All machine JSON is ASCII-only. Test Unicode, ANSI/C0/C1 text and PowerShell
5.1 redirection/code-page cases. Defang runner command prefixes such as `::`
and `##vso[` on human channels, and ensure encoded machine data cannot become
a runner command. Seed a redactor with every resolved API key/token and apply
it before any human channel, model tool result, journal, spill or trace writer.
Test secrets spanning stream chunks and partial/error outputs. This is seeded
redaction, not a detector for arbitrary secrets in repository files.

`--output-file` must not destroy an existing unrelated file or follow a planted
symlink. Accepted policy: create-only, atomic final publication for JSON/text;
append-free, one-writer event streaming for JSONL. Output failures are explicit;
trace failure remains best-effort. Exact crash/prefix behavior is fixed in R2.

## 7. Exit outcomes and stop ordering

The accepted exit table (staged features are not claimed before implementation):

| Exit | Outcome |
|---|---|
| 0 | end of turn |
| 1 | structured provider refusal; verification outcomes are deferred to P4c |
| 2 | usage error before a run |
| 3 | round/token/wall limit, with kind and partial transcript |
| 4 | approval required under `--on-unapproved fail` |
| 5 | retryable provider failure |
| 6 | nonretryable provider failure |
| 7 | internal failure |
| 129/130/141/143 | HUP/INT/PIPE/TERM interruption, P2g |

Do not label task correctness from arbitrary final-answer phrase matching.
R4 accepts structured refusal only for P2a exit 1, without a brittle text
heuristic. Refusal, verification failure and the
external grader's verdict are distinct from successful conversation completion.

P2g ordering: stop admission, cancel the active turn/tools/children, await the
turn task under a bound, seal output, write one partial result, perform bounded
persistence and then exit/re-raise the Unix signal. An emitter closed flag
prevents anything after the result. Synchronous filesystem work uses blocking
workers; run runtime has at least two worker threads. SIGKILL/crashes may leave
no final result; a missing `run_end` in trace v1 is documented crash evidence.
A broken output pipe stops quietly, never through an uncontrolled `print!`
panic. These guarantees must have signal/pipe/one-CPU tests before being claimed.

## 8. Trace, retry and release compatibility

Adopt `docs/trace-schema-v1.md` and its vectors **unchanged**. Trace is a
measurement record, not a transcript or event feed: no prompts, outputs, tool
arguments or file contents. It includes model calls by role, tool arguments/
redacted-result hashes, timestamps/status/bytes, edit paths/before-after hashes/
hunks and per-round tree hashes independent of the checkpoint store. Start the
run clock at the P0-defined point after parsing/config and before session build.

Trace writing cannot block the run or change its behavior, even in ephemeral
mode. Reject unsafe paths/FIFOs/devices; a trace path inside the task workspace
is disabled rather than discoverable by the agent. The default outside-workspace
path and disabled-trace result metadata are fixed in R2.

P2a introduces typed provider errors and bounded interim `Retry-After` handling
before P4b's resilient layer. Respect `Retry-After: 2` for at least 2 s; do not
retry indefinitely or restart a partially streamed answer as a duplicate turn.
Support extra CA roots from `SSL_CERT_FILE`; never log certificate/key material.

Ship a Linux no-default-features asset for minimal images. That feature set
excludes voice and the optional Wasmtime host; do not advertise gadget functions
from a binary that omits them. A gadget-capable non-voice build is separately
`--no-default-features --features gadgets`. The TUI, standard ACP profile,
acp-bridge health/stats and frozen desktop-v4 frames remain unchanged. New
agent-facing ACP capabilities require the existing namespaced extension path,
not a desktop-frame addition or an undocumented dialect.

## 9. Conformance and rollout

Before declaring P2a complete:

- One real-output/schema test for each produced exit; exactly one JSON result
  or final JSONL result event, and every stdout line parses in JSONL mode.
- Argument/file/stdin selection, BOM/invalid UTF-8/oversize rejection, idle open
  pipe and slow partial input tests; no provider calls on usage errors.
- Main/child/compaction reported usage sums equal the mock server; omitted usage
  is estimated and still trips the token budget. Limits retain partial output.
- Seeded credentials absent from every human/machine/tool-result/storage channel,
  including chunk boundaries and error paths; no inherited broad approvals.
- No project instructions/skills/plugins/gadgets without trust, no MCP/LSP/memory
  in v1, and isolated config-dir skills are correctly discovered when allowed.
- Filesystem escape, dangling-link and parent-swap tests; exact protected-path
  overrides and delegate enforcement. Document Windows best-effort limitations.
- Ephemeral runs create no durable artifacts other than the permitted trace;
  resume/continue isolation and output-file non-overwrite tests.
- Provider retry/CA tests and the three legacy `-m` regressions; minimal Linux
  image plus all non-POSIX contract tests on Windows.
- P2g signal/deadline/process-tree/pipe tests before its features are released.

Mock conformance does not clear a live model-quality gate. P3a/P3b still require
a throwaway runner, deterministic/held-out graders, scoped keys, approved budget,
provider review, baseline/A/A and per-model live evidence. Gadgets remain opt-in;
legacy loader removal waits for the independently published deprecation cycle.

## 10. Accepted resolutions — October 20, 2026

The user/maintainer explicitly approved these concrete R1–R7 choices and the
result/event schemas in this development thread. The schemas/reference checks
remain specification artifacts: they do not enforce permissions on a real CLI,
launch a model, or establish a passed live suite. Runtime implementation starts
only after this accepted spec merges with green CI.

### R1 — invocation permissions

Headless v1 accepts `--permission-mode manual|plan`, default `manual`, and
`--on-unapproved deny|fail`, default `deny`. The other legacy modes and explicit
`--auto-approve` alias are usage errors for `exec`; values in ambient config are
ignored with a fixed warning. Plan cannot permit mutation even through an allow.
The same immutable invocation policy governs delegated tools. Ask-user never
prompts. Background tasks, MCP, LSP and durable memory stay off in v1.

The exact allow/deny grammar is:

| Rule | Meaning |
|---|---|
| `tool:NAME` | A recognized tool name; no glob or prefix match. A broad execute allowance is explicit native host-operation permission, not containment. |
| `exec:["PROGRAM","TOKEN",...]` | JSON array of 1–16 nonempty plain-token prefix elements, each at most 256 characters, entire rule at most 4096. Match parsed tokens, never a string/head-word guess. |
| `path:WORKSPACE/RELATIVE/FILE` | Exact normalized file path. For allow it overrides only an overridable protected write after the corresponding write/edit tool is allowed; it never grants a tool itself. For deny it forbids that path. |

Deny wins, including a tool deny over a matching command allow. Ordinary
workspace read/control tools have their fixed baseline permissions; gated
writes, shell and network need explicit allows. Unknown tools/rules fail before
provider traffic. A path is not an absolute/drive/UNC path, directory, glob or
parent traversal: no empty/dot components, backslashes, colons, wildcard syntax
or control characters. The reference checker validates grammar only, not a real
filesystem, executable or policy decision.

Command-prefix rules require a bare executable name and reject shell operators,
substitution, redirection and control characters. Shell/sudo/env/xargs wrappers
cannot be used to disguise unrestricted execution as a prefix rule. Git prefixes
must name status/diff/log/show/rev-parse/ls-files, and reject output/context/config/
exec-path/pagination overrides, including joined/abbreviated output forms and
short `-c`/`-C` forms. These token restrictions do not make a configured git
repository or executable side-effect-free. Unparsed/native behavior remains an
OS-containment issue, not a proof supplied by an allow rule. Windows command-
prefix matching is not claimed until its classifier exists; a build must reject
unsupported command-prefix rules, never silently reinterpret them.

`--env-pass` names must be valid, unique environment identifiers. Conventional
credential names (`*_API_KEY`, `*_TOKEN`, `*_SECRET`, `*_PASSWORD`) and any name
identified as a resolved provider credential source are not passable overrides.
Only values explicitly selected for this invocation can cross the normal scrub.

Gadget function visibility/approval must remain opt-in and source-bound.
`--allow-gadget` is an explicit invocation grant, not a global default change;
compiled support must be present, and project sources require workspace trust.
Writes remain separately gated. The authoring skill is not advertised by
headless v1 when its source-write workflow cannot satisfy confinement; do not
promise user-directory/project-config writes through confined file tools.

### R2 — input, output and trace bounds

Accept exactly one instruction source, with bounds of 1 MiB instruction bytes,
1 MiB per attached file and 8 MiB total attachments. Explicit named stdin has
10 s first-byte and 30 s total-read deadlines. Validate input, flags and output
paths before assigning a run or making provider calls. Usage errors (exit 2)
produce no run result or trace. Input is never silently truncated.

Human/machine output is separately bounded: at most 1 MiB final-answer UTF-8
bytes and 16 MiB encoded final result. If output/state must be reduced, set
`output_truncated` and `messages_omitted` explicitly, keeping whole retained
messages and the final outcome. `message_scope=retained_state` means the post-
compaction state, not an assertion that the transcript is a complete audit log.
A message/event payload stays within the schema's per-string limits; emitters
chunk text and expose tool-result truncation rather than changing outcome data.
The reference checker's 16 MiB input bound is a validation-tool resource limit,
not a claim that a long live JSONL feed is limited to that size.

Text/JSON output-file publication is create-only and atomic. Reserve a new
JSONL file with restrictive permissions and one writer; no append/replacement
of an existing run. On storage/pipe failure, return the defined safe error or
interruption without claiming a complete result on a failed sink. A prefix may
remain; the offline checker accepts it only with explicit `--allow-prefix`.

Default trace goes to a private random run directory under the OS temporary
root, outside the task workspace. If no safe outside-workspace location exists,
disable tracing with an explicit reason. An explicit unsafe/inside-workspace
trace destination is disabled, not exposed to the agent. Trace failure remains
best-effort and cannot change the run outcome; trace-v1 prefix/overwrite/hashing
semantics are unchanged. Local trace paths may reveal a username and are private.

### R3 — protected files

Confinement is unconditional: no rule, mode, trust flag or permission state
permits an in-process file operation outside the canonical workspace. The
selected config directory and credential `.env*` paths are hard-denied for read
and write; `.git`/`.craftsman` writes never get a path override in v1. Reserved path
components are compared conservatively across supported platforms. Grant/config
files cannot be an agent's self-approval mechanism.

Overridable protected writes include CI configuration and executable build/setup
inputs: `.github/`, `.gitlab-ci.yml`, `azure-pipelines.yml`, `Jenkinsfile`,
`build.rs`, `build.sh`, `build.ps1`, `install.sh`, `install.ps1`, `Makefile`,
`GNUmakefile`, `CMakeLists.txt`, `Cargo.toml`, `package.json`, `pyproject.toml`,
`setup.py`, `setup.cfg` and workspace automation configuration. Only a matching
exact `path:` allow plus its write/edit tool permission can override them; deny
still wins. No broad tool rule or repository-proposed configuration widens this
set. Native shell/build commands are explicitly not contained by `ConfinedFs`.

### R4 — outcomes, not phrase heuristics

Exit 0/`completed` means an end of turn, not a passed coding-task grader. P2a
emits exit 1/`task_failed` only for a structured provider refusal, with standard
`refusal` stop reason. A plain answer saying "the tests failed" remains a normal
end of turn; arbitrary final-answer phrase searching is not permitted. A later
implemented verification/explicit task-outcome mechanism can extend the reason
vocabulary under its reviewed contract. It cannot be claimed by P2a's mocks.

Provider/internal/approval/limit/interruption outcomes have exact schema mappings.
Application error details go in `_meta.craftsman`; they never become invented ACP
stop reasons. A failed provider request has `stop_reason=null`, not
`provider_failure` as a stop reason. Usage errors remain outside run records.

### R5 — shared admission and observed usage

`--max-rounds` is the accepted shared **model-request admission** limit,
default 50, maximum 10000, covering main, subagent and compaction logical calls.
This accepted choice replaces the original main-only wording; the recorded
result field is `limits.max_requests`. `rounds` separately counts
main-agent requests. Internal retries stay within their logical request and
bounded retry policy; the role ledger records attempts whose usage is unknown.
Background delegation cannot escape this budget because it is unavailable in v1.

Each role row has requests, input/output tokens, reported/estimated-call counts
and unknown attempts. Every admitted logical call is classified reported or
estimated; missing usage is never zero by default. Aggregate totals equal the
role sums. `source` is none/reported/estimated/mixed, and `billing_complete=false`
when any retry/failed attempt has unknown billing. This is a measurement ledger,
not a provider-dollar guarantee or invented precision for failed attempts.

Admission reserves estimated input and bounded requested output against the
shared remaining allowance. No request starts without room for its required
input/reply, and no further admission follows an exhausted ledger. Final reported
usage replaces estimates, not clamps; an overrun is exposed. A completed result
cannot hide a known token overrun. A token-limit result records the actual total,
which may exceed its budget because of estimation/provider reporting. A response
that ends normally at the exact limit can complete without another admission.

**Explicitly accepted #97 clarification — November1:**
[token-admission.md](token-admission.md) records optional pre-exhaustion reservation
detail and strict native/reference vectors. The user explicitly approved the field
shape, strict truthful reservation semantics and v1/older-validator/collision
compatibility judgment. R1–R7's October20 acceptance stays unchanged. Shared meter/
public producer implementation remains unwired; accepting the representation does
not settle concurrent reservations, provider receipts or publication. The separate
private [request-only foundation](request-admission.md) implements the accepted
shared logical-call ceiling; a separate [atomic reservation opt-in](token-reservations.md)
adds checked-source estimates/clamping before charge, not settled token usage or
public token-budget/flag/result delivery.

### R6 — persistence and workspace identity

No session selector means a fresh durable thread. `--ephemeral` has null thread
id and no DB/memory/snapshot/journal; its best-effort trace is independent.
`--session ID` resumes an existing thread in the selected config directory.
`--continue` selects its most recently updated thread whose canonical primary
workspace matches `--cwd`, using ascending lexical thread id as a deterministic tie-break.
No match is a usage error, not an implicit new conversation.

Resume/continue cannot silently switch workspaces or inherit stored broad
permissions. Mismatched, missing or legacy-unscoped primary workspace metadata
is a usage error for headless v1; migrate/choose scope through an explicit
interactive workflow first. Additional project contexts do not widen `ConfinedFs`.
The three explicit selectors are mutually exclusive; fresh run ids use random
bits independently of conversation ids. Effective session mode is always recorded.

### R7 — schemas, stream semantics and additive readers

Implement the accepted self-contained common contract and local result/event wrappers:

- `docs/schemas/exec-contract-v1.schema.json`: shared definitions and the two
  record families, resource `urn:craftsman-harness:exec-contract:1`.
- `exec-result-v1.schema.json`: `craftsman-harness/exec-result@1` record entry point.
- `exec-events-v1.schema.json`: `craftsman-harness/exec-event@1` event entry point.
- `exec-v1.vectors.json` and `check_exec.py`: structural, aggregate, sequence,
  identity, lifecycle, grammar and wire-format conformance. Native JSON Schema
  tests independently check the same structural cases with local resources.

These are CLI records, not stock ACP JSON-RPC frames. Their stop/status values
follow the standard v1 vocabulary; tool denial/cancellation maps to failed plus
`_meta.craftsman.tool_status`, never new stock tool statuses. The record/event schemas
are distinct from the unchanged `craftsman-harness/trace@1` measurement file. Unknown
fields and unknown event kinds are additive for tolerant readers; known writers
must meet all required fields and outcome mappings.

Streams have one run start at seq 0, gap-free seq, stable run/thread identity,
rooted agent paths and one final main result last. Tool ids are scoped by agent
path; every started call is terminal before a final result. Model role/path and
emitted role ledgers match the result, including denial counts. Effective policy,
workspace, features, limits and session cannot change between start and result.
Calendar timestamps must exist; counters are JSON integer literals, not decimal
spellings (the semantic layer is stricter than JSON Schema mathematical integers).
Malformed/nonfinite/duplicate-member/BOM/non-ASCII machine JSON and unpaired
Unicode surrogates are rejected. An explicitly allowed prefix is incomplete evidence,
not a successful final result. Seeded redaction is still the producer's duty;
the checker does not discover arbitrary credentials.

Result keys and nested types are now exact in the schemas. Notable clarifications:
`message_scope`, `messages_omitted` and `output_truncated` expose retained/bounded
state; `verification.status=not_supported` makes P2a's lack of receipts explicit;
`usage.billing_complete` exposes unknown billing; `sandbox.shell=unavailable`
cannot falsely claim a pre-P2f OS shell boundary. P2g/P2f/P4c changes need reviewed
additive schema revisions and conformance vectors before being claimed.

### Acceptance record

- Technical proposals and conformance review: coding assistant, October 20, 2026.
- **User/maintainer acceptance:** explicit approval in this development thread on
  October 20, 2026, in response to the question asking approval of the concrete
  R1–R7 contract in #95, its merge after green CI, and the start of `craftsman exec`
  implementation. The response was “yes approve”; this is not inferred from a
  general request to continue, and no personal name or GitHub review is invented.
- **Accepted choices:** R1 manual/plan and exact invocation rules; R2 bounded
  inputs/outputs and create-only publication; R3 unconditional confinement and
  hard-denied credential/config paths; R4 structured refusal, not phrase
  heuristics; R5 shared all-role admission/usage; R6 deterministic workspace-bound
  sessions; R7 exact schemas and semantic stream conformance, as written above.
- **Merge gate:** make #95 ready and merge this accepted spec only after the
  latest-head formatting/Linux/macOS/Windows checks pass. The approved admin
  merge may bypass an outstanding review requirement, not failed checks.
- **Not approved by this record:** a claim that P2a has shipped, OS shell
  containment before P2f, signal/deadline guarantees before P2g, verification
  receipts before P4c, live model-quality/default-on approval, or premature
  removal of the legacy plugin loader.


## 11. Implementation progress — not a shipped `exec` command

The accepted specification merged as #95 on October 20, 2026, after its exact
acceptance head passed formatting and Linux/macOS/Windows CI. The authorized
admin merge bypassed only the remaining review requirement, not failed checks.

The first P2a implementation slice prepares `src/exec/permissions.rs`: immutable
explicit-only selectors, narrow manual/plan modes, deny precedence, exact token
and path matching, whole-command git restrictions, unsupported-platform prefix
rejection and credential-source environment-pass validation. It exercises all
33 accepted rule vectors plus native boundary cases. A broad native allow cannot
bypass a command-prefix deny by making the command unclassifiable: classification
failure with a prefix deny fails closed. This private module does not yet enforce
a real headless invocation or filesystem confinement. Plan is
conservative: native execution is denied even with an allow, because a prefix
match alone does not establish mutation-free behavior. Environment pass names
with case-ambiguous duplicates are rejected for portability; no values are read.

The existing `craftsman -m` path now uses `build_session_ext` with a fallible main
sink: non-terminal approval input denies without reading (including idle open
pipes and approval-like bytes); delegated/compaction deltas stay off stdout; a
broken stdout pipe stops quietly. All role providers check a latched sink failure
before and after a response, preventing same-poll tool dispatch after a failed
content delta. In-flight turn cancellation belongs to the legacy caller; this is
not the P2g ordered partial-result/signal/deadline implementation. Tests use the
real binary and a synthetic local provider, not hosted models or live credentials.
They also cover an unfinished SSE response, final-newline pipe failure and the
unchanged legacy round-cap exit 2 (not headless v1’s future limit exit 3).

A separate private instruction/attachment-file boundary now admits bounded original
UTF-8 data through `PreparedInput` before session/store assignment and the SAME runtime
builder. It preserves data/labels without granting capabilities or selecting a shell/
slash-command route. Named stdin is still refused by the old input routes, not implicitly consumed.
A separate private `StdinInvocation` now captures only explicitly selected native
byte pipes under original1MiB/10s-first-byte/30s-total limits before sessions;
console/regular-file/public frontend support remains separate. See
[named-stdin.md](named-stdin.md).
See [headless-input.md](headless-input.md) (#115) for the input-first ordering,
retained filesystem and remaining limits; this is not a completed public input/run
producer. The actual runtime additionally selects a bounded checked view of its
main retained messages under the state lock, before copying their original data.
Ids/names/argument keys/scalars remain original or refuse; other state and public
result/omission/terminal publication stay separate. See
[retained-message-views.md](retained-message-views.md). A separate private
`View::select` now keeps the newest whole-message suffix within a tightening encoded
budget and rechecks omission/framing literals. It cannot rescue unadmitted source,
change the original outcome or publish a terminal result; see
[message-selection.md](message-selection.md). A separate private actual-Run terminal authority now
seals execution without fabricating approval, owns original outcome/runtime and
checks an independent publication lease/typed epoch. It can capture bounded checked
partial approval state while live views stay stopped; public status/IO/ASCII and
all-role/process shutdown remain open. See
[terminal-publication.md](terminal-publication.md). A separate private terminal-message selector now
keeps full checked output/return/approval facts, reducing only whole newest messages
after both complete channels pass admission. The all-message snapshot and live-latch
semantics remain unchanged; public IO/outcome/R2 gates stay open. See
[terminal-message-selection.md](terminal-message-selection.md). A separate private terminal ASCII
encoder now projects/verifies complete admitted channels and selects whole messages
using exact escaped-byte sizes under the SAME lease/seal. It is not public schema,
human defanging, OS redirection or IO/result production; see
[terminal-ascii.md](terminal-ascii.md). A separate private human renderer now uses
that actual terminal scope and immutable final output for whole-answer/fixed-fact
display, visible terminal-control/runner defanging and exact combined UTF-8/LF
bounds, with SAME lease/epoch checks. It does not decode retained/ASCII state,
choose public text outcomes, enable stdout/stderr or provide OS redirection; see
[terminal-human.md](terminal-human.md). A separate private FileInvocation route now
captures an explicit existing external output directory/name/format/budget before
session IDs/store/provider effects, consumes its actual Run and stages only checked
terminal bytes for one guarded create-only file publication. It is not public
result/text/stdio/JSONL/inside-workspace output or full R2 support; see
[guarded-output.md](guarded-output.md). Windows directory-rename fixtures exercise
one real attempt and unchanged physical authority whether native handles deny
movement or identity checks refuse a genuinely rebound path. The two failed PR125
Windows runs are preserved; all FOUR final-head jobs remain merge prerequisites.
Final content-pass accounting includes its seek and EOF probe inside the same
4096-call cap; actual native boundary fixtures bring private coverage to66
(64 on Windows), not a larger source/public budget.
Native cleanup errors retain their result until SAME lease/epoch recheck; the
paired controller also checks after early results/Drop cleanup, never fake success.

A separate private StdinFileInvocation now combines exact positional-`-` source
admission with the original guarded Destination: read-only target capture before
stdin inspection and a SECOND retained-target/absence check after complete input
but before session/IDs/store/providers. Opaque preparation alone constructs the
existing FileInvocation; FileRun/FileTerminal/Record/native publisher stay unchanged.
It is not a caller Run/target or new reader/writer/stdio constructor. See
[stdin-file-output.md](stdin-file-output.md) for ordering and honest late-change/
source-consumption/OS/shared-reader limitations. This does not complete full flags,
public source/output routing or the remaining R2/#97/P2g/provenance/usage gates.

The separate private instruction-argv factories now parse ONLY source/attachment
arguments, reject unsupported/conflicting flags before any config/stdin IO, and
route to the SAME original argument/file/native/composed factories. Returned
Run/FileRun and all reader/publisher/epoch/source limits stay exact. Explicit
Options/Destination are still pre-run typed inputs, not full frontend flag/output
format handling; see [input-arguments.md](input-arguments.md).

**Still open before exposing `craftsman exec`:** complete named-stdin ownership/console/file support and remaining full frontend
input/flag/output-path validation around the private source-argv subset; complete invocation/run production around the private preparation/
builder adapters (sections 12–24); redacted guest integration and
session persistence; shared request/token admission and the all-role ledger;
real result/event production,
seeded redaction, create-only output and trace-v1 instrumentation; typed provider
errors/retry/CA handling and the remaining P2a exit/asset tests. A matching rule
is not OS containment, and conformance mocks are not live-quality evidence.
The full P2a exit criteria remain open. Gadgets stay opt-in and their independent
live-evaluation/published-deprecation gates are unchanged.


## 12. Confined filesystem implementation — private backend, not CLI rollout

`src/exec/confined_fs.rs` implements the app-layer `Backend` for read, create-only
write, exact edit, ls, glob and grep. It is available in a no-default-features
build, independently of the optional gadget host. The private runtime adapter
now selects its retained authority (section 14), but no CLI frontend does:
`craftsman exec` stays unexposed while strict guest-host, usage, input and real output
contracts are completed.

The backend retains directory authority for the canonical workspace. Subsequent
IO never checks a path and then reopens it through ambient `RealFs`. Existing
parents are opened component-by-component without following symlinks, and their
identities are checked again before file access/publication. Regular-file handles
are checked after opening; nonblocking Unix opens prevent a swapped-in FIFO from
hanging a worker. Symlinks (including in-root aliases), special files and multiply
hard-linked files are rejected or excluded from discovery. The original root
handle remains authoritative if its reported pathname is renamed/rebound.

Every operation enforces the same immutable invocation policy, including calls
through a backend-bound delegated/custom tool. Read, listing and search filter
config/`.env*` and path denies; write/edit additionally hard-deny `.git`/`.craftsman`.
Overridable CI/build inputs still need an exact file allow **and** that tool's
permission. The automation directory set is conservatively `.github`, `.gitlab`,
`.circleci`, `.superset`, `.devcontainer` and `.vscode`, alongside the named R3
build/setup files. Existing file selectors cannot name directories, symlinks or
special files; a future denied filename cannot be created as a parent directory.
Config/deny comparisons use conservative NFC/lowercase keys to cover Unicode and
case aliases, but actual opens and protected-write allows retain exact original
spelling: normalization never grants a different file. The config directory's
captured identity remains denied if it is renamed within the workspace.

Create-only writes publish a fully written private temporary file by a
handle-relative no-replacement hard link, with no unsafe overwrite fallback.
Edits use same-directory replacement after identity/content checks and serialize
this backend's own operations. This is **not** a transactional compare-and-swap
against arbitrary external writers. Unix new files/directories are private;
edits preserve ordinary permission bits, not special set-id bits. Temporary
`.craftsman-exec-tmp-*` names are hidden/hard-denied to file tools and cleanup is best
effort (a crash/IO/ACL error can leave one). Windows remains best effort until P2d.
Pinned directory authority and in-process policy are not a general OS sandbox;
ongoing external namespace changes or already-authorized native processes are
not contained by these checks. Native execution is a separate `ShellRunner`/
`ProcessTree` path; this backend does not advertise execution support.

IO runs on blocking workers. Resource bounds are 10 MiB per raw file, 64 MiB of
aggregate grep reads, 100000 traversed entries, 128 path components, 8 MiB of walk
names and 16777216 glob-matcher work units. Glob patterns are at most 256 bytes;
result names/text are bounded at the established 100000-byte tool-output limit.
Text uses the existing explicit truncation marker; the later event producer must
also expose truncation metadata. Bounds do not claim P2g wall-deadline/cancellation
or final-result ordering; a started blocking filesystem operation may finish
when its awaiting turn is cancelled.

Confinement tests use temporary workspaces and synthetic scripted models, covering
absolute/relative escapes, protected overrides, main/delegated enforcement,
symlinks/hard links, parent/final/temp swaps, FIFO substitution, Unicode aliases,
concurrent edits, private creation, text bounds and a responsive single-thread
async runtime. `RealFs`, default TUI/gadget behavior, SDK/WIT, frozen goldens and
trace-v1 are unchanged. Selecting this backend alone must not be treated as
headless configuration/session isolation, guest-host enforcement, live model
quality, default-on approval or permission to remove the legacy plugin loader.


## 13. Explicit preparation implementation — October 21, 2026 (Asia/Taipei)

`exec/construction` prepares a private context from explicit absolute workspace
and config-directory paths. It does not read an ambient config path, mutate HOME,
`CRAFTSMAN_CONFIG_DIR` or cwd, connect a provider/MCP/LSP, run a guest, materialize an
SDK, open memory, take snapshots, or create a journal. Existing frontends still
use their established construction path; `craftsman exec` remains unexposed.

Config and discovery use bounded no-follow root handles. The selected config
file is at most 1 MiB; optional instructions/skills/manifests are at most 256 KiB
per file, with 64 visible candidates/1024 total entries per discovery root and
512 KiB effective context. Section 16 retains the selected config source handle
and a clone of the same ConfinedFs workspace handle after discovery; neither
later source construction nor root replacement triggers an ambient reopen.
User agent instructions/skills, user skills/plugins
and opted-in declarative gadget metadata come only from the selected config
root. Project `.craftsman/AGENTS.md`, `.craftsman`/`.agents` skills and project manifests
load only with explicit workspace trust. Symlink/special/hard-link sources are
rejected or skipped; skips are counted without printing their data. This stricter
private loader does not change legacy TUI dotfile/symlink behavior. Gadget
metadata parsing shares the existing validator and merge/order rules but never
compiles, instantiates, or calls `describe`.

Skills are supplied as bounded read-only context, not advertised as an absolute
user-config `SKILL.md` path that confined file tools cannot read. `build-gadget`
guides are omitted while their workflow cannot satisfy confinement. Native
command/file tool permission remains independent of instructions or framing.

Ambient permission mode/auto-approvals/shell allow lists are discarded for the
prepared config. Scrubbing is forced and env-pass names come only from explicit
validated selectors; captured shell settings retain credential source names even
from services disabled for headless construction. The preparation config clears
MCP/LSP declarations and limits delegated parallelism, but **is not a drop-in
legacy SessionExtras/config**: adapters must omit built-in LSP/memory/background
tools and use the captured shell/policy, not reconstruct them from ambient or
sanitized config. Section 14 implements that private shared-builder path;
preparation alone still makes no runtime/input/output isolation claim.

Ephemeral preparation returns before any thread-store inspection/open and has
null thread identity, no store or journal path. Durable modes use only the selected
`config_dir/threads.db`; no global cached thread path or implicit legacy-file
migration is invoked. Named resume/continue first read the existing catalog with
SQLite read-only open flags, without schema application or legacy workspace
retirement. Canonical primary metadata must match the workspace and agree with
the stored workspace; missing/mismatched/legacy metadata is an error, not a claim
or a switch. Continue orders matching rows by updated descending and lexical id
ascending. Checkpoint text is bounded at 16 MiB before decoding; no match creates
no conversation. SQLite read-only behavior may involve its existing WAL/shared
sidecars; do not equate a read-only catalog query with OS-level filesystem
containment or a universal no-sidecar guarantee.

Only after valid selection/schema checks does durable preparation open a writer
that deliberately skips the existing frontend's legacy-workspace retirement.
Fresh threads use 128 bits of OS randomness and create-only identity insertion;
a collision cannot overwrite another conversation. The existing DB layout/stamp
and old fixtures are unchanged. Resume preserves conversation messages but drops
stale custom permission/cache/file state and supplies error results for missing
pending tool responses; it never invokes the middleware-bypassing approve/resume
path. Additional project contexts do not widen the selected `ConfinedFs` root.
The journal directory is planned explicitly but no journal writer is constructed.
The private runtime retains this plan without wiring the raw checkpointer or
journal: seeded storage/output redaction must precede durable turn publication.

Preparation tests cover isolated config/discovery roots, trust, bounds, credentials,
unsafe sources, ephemeral artifacts, random/create-only IDs, deterministic selection,
canonical workspace checks, legacy metadata preservation, saved-state bounds,
interrupted approvals and data-only migrated gadgets. Section 14 adds the private
shared-builder adapter. Still open: strict guest-host filesystem/policy wiring,
Complete stdin ownership/console/file and frontend input integration (the separate
`PreparedInput` path now admits instruction/attachment files before sessions),
shared admission/usage (#97), seeded
storage/output redaction and event/result/trace production, typed providers and
phase-specific shutdown/asset tests. This
is not shipped `exec`, verification, OS containment or live/default-on approval.


## 14. Private shared-builder runtime adapter — October 21, 2026 (Asia/Taipei)

The preparation PR #99 and shared-builder adapter PR #100 merged after green
exact-head formatting and all three OS checks. #100's earlier Windows fixture
failure was fixed and reverified, not bypassed. #99 merged on October 20, 2026 UTC
(October 21 in Asia/Taipei); #100 merged on October 21, 2026 in both UTC and
Asia/Taipei. The implementation now adds `exec::runtime::Runtime::build`,
which **consumes** `Prepared` and enters the same session-construction core used
by `agent::build_session_ext`. There is no `craftsman exec` route, argument parser,
real result/event writer or complete headless run producer. Only synthetic local
providers exercise this unfinished adapter; no live model-quality/default-on or
release gate is cleared.

The typed construction choice is separate from legacy frontend sinks/config.
It derives its config, canonical workspace, retained `ConfinedFs`, bounded scoped
context, initial cleaned state and immutable permissions from preparation. The
caller cannot substitute an unrelated approval gate, UI callbacks or arbitrary
legacy SessionExtras. Existing frontend construction, prompts, tools, storage
behavior and frozen request/frame goldens remain unchanged.

The private profile explicitly omits MCP loading, builtin/custom LSP tools and
automatic write diagnostics, durable memory/store opening, workspace snapshots,
background task parameters/execution and callable gadgets. An explicitly disabled
LSP manager has no built-in fallback; clearing declarations alone would not achieve
this. Main and declared foreground roles use the prepared inline instruction/
skill/extension snapshot without new home/project reads. The headless base does
not advertise the legacy memory protocol; model-family overlays remain selected.
Project trust does not grant write/native/network permission.

One immutable invocation gate wraps **all** shared host/web tools and middleware-
contributed tools. Every delegated tool uses that same policy; `task`/`send_message`
remain absent from child capabilities. Profile-added child tool factories are also
wrapped, and unavailable service middleware is omitted before materialization.
The main gate precedes tool middleware and cannot be excluded by a profile. Tool
terminals recheck policy after any middleware transformation; no persisted approval
or model-driven mode change can widen the captured policy. Ordinary denials remain
denied error results, and `ask_user` never reads stdin, even with an idle open pipe.

Under `on-unapproved=fail`, the first unapproved call latches the invocation as
approval-required. Later tool calls in that **same** model response cannot execute;
main/delegated/compaction providers check before/after completion, and compaction's
direct provider path is guarded too. The private turn wrapper returns an explicit
approval-required error with retained partial state, never a completed turn or a
human-approval suspension. This is not yet the schema-backed exit-4 producer or
P2g cancellation/final-result shutdown; started blocking operations are not rolled
back, and no OS-shell containment or signal guarantee is implied.

Shell construction uses the captured settings, including credential-source names
from services already disabled in routing config. Those sources cannot survive via
the legacy toolchain environment pass list. Invocation env-pass names stay explicit
and validated. `ShellRunner` still uses `ProcessTree`; a private headless capture
profile retains bounded head/tail **in memory only**, under legacy or head/tail
format choices, including timeout and late formatter cuts. It creates no unredacted
spill file and states when full output was not retained. Frontend/native-authoring
spill behavior and default Unix process-group settings are unchanged. This is not
seeded redaction or a filesystem sandbox for a permitted native command.

Main/child/compaction token and activity sinks are explicitly silent. The runtime
copies only preparation's cleaned initial conversation; it never uses the legacy
`load_thread`/approve/resume bypass. Durable preparation still reserves/selects its
explicit thread, but the live agent has **no raw checkpointer or journal writer**
until redacted storage adapters exist. Ephemeral construction and turns open no
store/memory/snapshot/journal/SDK; the internal session's empty bookkeeping thread
string is not a published persistent identity, and preparation's thread id is null.
Disabling writers/sinks is not redaction of retained/model/tool data.

Tests cover real shared-builder model/tool/delegation paths with synthetic providers,
trusted versus untrusted context, source/root replacement after preparation, exact
permission/plan enforcement, same-response and child approval-fail latching, contributed
and profile-added tool terminals, idle-stdin/no-stdio-fallback behavior, disabled-
service credential sources, resumed state/no raw durable turn writes, memory-only
capture and data-only gadgets without guest execution. An initial Windows CI
attempt caught the pre-existing bridge fixture's cold `sh` startup timeout; bridge
IO tests now use a native ready/echo/exit child with a bounded startup handshake.
The existing three-second final-frame/close checks begin only after readiness,
and a close timeout is now a failure rather than a silently ignored condition.
Production bridge frames/code and frozen goldens are unchanged; this is not a
headless signal/shutdown implementation or a relaxation of latest-head CI gates.
Full output/usage/redaction/trace and composed source-approved guest-host construction
remain next boundaries. Section 16 adds private source/approval snapshots without
constructing or executing a guest host.
Issue #97's truthful pre-exhaustion token-admission outcome must be resolved before
a shared meter is wired; do not invent usage or label an admission stop completed.


## 15. Byte-safe guest filesystem facet — October 21, 2026 (Asia/Taipei)

PR #101 merged on October 21, 2026 after all four exact-head CI jobs succeeded,
including the Windows fixture fix described below; no failed check was bypassed.
`ConfinedFs` now exposes private typed raw read, create-only write and bounded
listing operations, using the same retained directory authority, immutable
permissions, no-follow parent walks, regular/single-link handle checks, atomic
creation and blocking worker as its text `Backend`. Binary reads/writes do not
insert line numbers or empty-file sentinels, replace invalid UTF-8, or silently
truncate bytes. Text Backend behavior and its error messages remain unchanged.
The raw file bound is 10 MiB. Guest-facing listings charge **all** candidates
before filtering at 1024 entries; ordinary text `ls` retains its prior larger
traversal/output bounds.

`gadgets::confined::ConfinedGuestFs` is a private filesystem facet built from
`Prepared` and a validated capability declaration. It captures both authority
and the exact same invocation policy rather than accepting a separately supplied
backend/gate. A declared `workspace` read capability is needed for byte reads and
directory metadata; a declared write capability still does not grant invocation
write permission. Explicit tool/path denies and plan remain final. Overridable
CI/build writes need exact path plus write-tool permission; config/credential/
runtime-artifact and `.git`/`.craftsman` hard denies cannot be overridden. No mutable
ApprovalState, human prompt, ambient config reload or grant-file read is used.
Guest writes retain the existing 1 MiB import bound and create-only behavior.

Directory results are deterministic structured names/type/size, without exposing
absolute paths or filtered entries. Filesystem failures are classified from typed
`FsError`, with fixed-text diagnostics—not by matching arbitrary error strings.
Unapproved/overridable-protected writes yield `NeedsApproval`; hard denials stay
denied. The future composed host must observe these typed failures through the
**same** invocation fail latch even if guest code catches an import error.

**Not wired/approved by this facet:** source/hash/capability first-load grants,
project-source trust/provenance, a headless `GadgetRuntime`/`HostState` constructor,
HTTP/log host composition, shared fail-latch/call-budget observation, seeded tool/
output redaction or callable model visibility. A declaration is a restriction,
**not** approval to compile/instantiate/call a component or even `describe`.
Neither existing terminal HostState nor the private headless runtime selects
this facet. Section 16 adds separate source/grant snapshots; section 17 now
selects the facet in a PRIVATE composed guest host, not the terminal host or
model tool list. Callable headless gadgets remain absent. There is no CLI rollout,
OS native-shell containment, full-run signal/deadline or P2a completion claim.

Tests exercise invalid-UTF-8/NUL/CRLF byte preservation, non-overwrite and size
bounds, declared-versus-invocation authority, explicit denies/plan, exact protected
writes, config/credential/temp filtering, structured directory metadata, untrusted
path inputs and no raw diagnostic leakage, candidate counting and immutable
configuration/declarations. Existing adversarial parent/file/temp-swap tests still
exercise the shared worker primitives; raw root-rebinding and Unix symlink/hardlink/
FIFO cases add byte-oriented evidence. Windows retains section 12's best-effort
limits. Only temporary roots/synthetic data are used; no guest/provider is executed
by the filesystem-facet tests. A Windows CI follow-up found an unrelated cold G3
CLI build exceeding the fixture's default 120 s shell limit. Both CLI/model-driven
cold-build fixtures now select the same explicit 240 s budget within their existing
300 s process bound; safety assertions remain, and production defaults/SDK source
are unchanged. A failed check is fixed/reverified, not waived by informational CI.


## 16. Retained gadget sources and approval snapshots — October 21, 2026 (Asia/Taipei)

PR #102 merged on October 21, 2026 after all four exact-head CI jobs passed.
Preparation retains the exact selected config
source handle instead of dropping it after discovery, plus a clone of the same
workspace authority already held by `ConfinedFs`. `SourceRoots` are capabilities,
not ambient path names or a grant of project trust. Existing config/discovery,
legacy frontends, store layout and SDK/WIT behavior are unchanged.

Private `gadgets::sources::SourceAuthority::from_prepared` captures those handles,
the same immutable invocation policy, explicit workspace trust and user gadget
opt-in. It validates at most 256 distinct explicit ids before source/grant IO.
A caller selects a typed user/project source; this is not a new CLI flag or a
catalog scanner. Project trust is checked before reading project files. An
invalid selected project source returns an error, never an automatic fallback
to an approved user source with the same id.

All subsequent source IO runs on blocking workers, cloning retained handles and
walking each parent with no-follow opens. Files must be regular and single-link:
256 KiB manifest and 32 MiB component bounds preserve the existing gadget limits.
The complete raw manifest and component bytes are hashed with SHA-256, including
an optional manifest BOM; parsing removes only that BOM and never modifies the
component. No lossy text conversion or silent truncation occurs. Before approval
is bound, both complete byte snapshots, opened-file identities/metadata and the
retained parent chain are revalidated. This detects in-place edits (including
restored size/mtime), file replacement and parent rebinding. It is not a portable
external-writer CAS or an OS-containment guarantee; Windows keeps the documented
best-effort limits. The returned `ApprovedSource` owns immutable raw bytes,
validated manifest, source label and provenance. Section 17 also binds its
original ConfinedFs/policy to the snapshot and compiles only those owned bytes,
never ambient-reopening the source label after approval.

There are two distinct origins, neither using mutable legacy `ApprovalState`:

- **Explicit invocation:** the first valid snapshot of a named id binds it to
  one source/kind and its capability ceiling for this invocation. Shared/cloned
  authorities retain that same monotonic binding. Rebuilds may inherit only
  equal/narrower read, write and HTTP declarations; a source change or widening
  requires new approval. Failed reads/validation do not commit a binding.
- **Source-bound user grant:** read only the selected config handle's
  `gadgets/.grants.json`, never a project-provided grant file, HOME fallback,
  cached global path or lock/writer. The existing version-1 format is unchanged.
  Grants are bounded at 1 MiB/1024 entries and validated with the same no-follow,
  regular/single-link checks; Unix mode must exclude group/other access. No
  Windows ownership/DACL guarantee is claimed. Reuse needs the exact id/source
  and equal/narrower capabilities, even if the component hash is unchanged.
  Prefer exact component evidence, then stable hash/capability order, independent
  of entry order. A rebuild records its inherited origin rather than pretending
  the stored grant approved its new bytes directly.

The user-grant snapshot is immutable for the authority's lifetime: later external
edits cannot add grants mid-invocation. An absent file creates nothing; an unsafe,
malformed or oversized file yields a fixed typed approval failure on fallback,
not repair/migration. Independent explicit approval does not depend on that file
being valid. No grant, lock, cache, SDK, store or journal is created by this layer.

Internal provenance records current id/source/kind, **full current manifest and
component hashes**, capabilities and origin. Invocation origin retains the first
snapshot's hashes/ceiling; user-grant origin retains the original component/
capabilities and exact grant-file hash. Version-1 stored grants do **not** contain
a manifest hash, so this record does not invent one for the original approval.
It is not serialized into results/events/trace or storage yet; source labels and
manifest data still need seeded redaction before publication. Typed `SourceError`
uses fixed text and maps to `GadgetError` without echoing OS/parser diagnostics,
paths, bytes or config/credential values.

**Source-only layer versus composition:** `SourceAuthority` itself still performs
no Wasmtime compilation, instantiation, ABI check, `describe` or guest call—even
invalid component bytes can be approved as a bounded data snapshot. Section 17's
separate private guest host now consumes those approved bytes and applies strict
filesystem/HTTP policy, empty-capability describe and the shared caught-import
fail latch. Legacy `GadgetRuntime`/terminal `HostState` are unchanged. Normal
`exec::runtime` does not auto-create/register callable tools/catalog; seeded
log/tool/output redaction and compatible provenance publication remain required.
Source approval still is not a filesystem/HTTP/native operation allow. Existing
terminal first-load grants and deterministic change-only catalog refresh before
main/delegated rounds and after compaction remain unchanged; newly installed
approved terminal gadgets still need no model-driven list polling.

Tests use temporary roots/synthetic bytes only: raw hashes/BOM/invalid executable
bytes, trust/opt-in/denies, exact source/id binding, non-widening read/write/HTTP
inheritance, deterministic provenance, immutable grant snapshots, explicit grant
independence, bounds/unsafe types and fixed diagnostics. Retained config/workspace
root rebinding, parent/file replacement, same-size/mtime-restored mutation and
concurrent cloned-authority binding add adversarial evidence. Source approval
still cannot permit an unapproved write or override plan/config denies.
`craftsman exec`, real redacted provenance/output/trace, bounded instruction input,
shared admission/usage (#97), typed providers and phase-specific shutdown remain
open. This is not a CLI, live-quality/default-on or completed P2a release gate.


## 17. Private source-approved guest host — October 21, 2026 (Asia/Taipei)

This records #103's private host. Section 19 now selects frozen-seed guest JSON/log
adapters; original approval/description metadata and public producer gates remain private/open.

PR #103 merged on October 21, 2026 after all four exact-head jobs passed. Its
initial Linux/Windows timing fixture failures were fixed/reverified, not bypassed.
After #102's green merge, `gadgets::headless::GuestHost` implements a separate
private execution host, not a sanitized legacy `RuntimeOptions` adapter or a
shipped `craftsman exec` tool. Construction takes an opaque `Runtime::guest_scope()`
lease: only the actual shared-builder runtime can pair its Prepared context with
its unique all-role fail latch. A new gate with the same policy is not an API
substitute. No caller-supplied paths/backends/declarations/hash, broad approval
state or human callback enters the host. Source opt-in, explicit ids, project
trust and non-widening approval still come from section 16. Each host retains its
single source-authority/grant snapshot; future registration must retain/reuse
that host, not reconstruct approval authority per model call.

`ApprovedSource` now carries its original ConfinedFs and permission Arc as well
as immutable raw manifest/component bytes/provenance. The store checks the same
policy and derives filesystem/HTTP facets from that source, never a separately
supplied Prepared/backend. Source failure is classified/observed before any
component compilation. A valid approval permits compilation from **owned bytes**,
then ABI-checked instantiation/describe/call; invalid bytes fail with fixed typed
diagnostics. There is no disk cache/native-code deserialization, source-label
reopen, SDK/grant/lock/store/journal write or legacy host selection.

Every describe uses a fresh store with **empty filesystem and HTTP capabilities**,
even for a fully approved read/write/network source. A source-built adversarial
guest actually attempts all three imports during describe; no operation succeeds
or creates a file/admitted request. Description/function/JSON bounds and bounded
schema regex validation remain; explicit no-retrieval schema validation cannot
fetch HTTP/file/config resources through guest-controlled references. Local
self-contained references remain valid. Parser/schema/guest failure strings are
not echoed or interpreted as authority/error kinds.

The call store selects the binary-safe filesystem facet and SAME immutable
invocation policy. Reads/lists/create-only writes retain section-15 bounds and
no-follow/regular-single-link authority. Config/credential/runtime-artifact and
`.git`/`.craftsman` restrictions remain final; protected CI/build writes require exact
file plus write-tool permission. Source grants never grant writes/network, and
plan cannot permit mutation. List results remain structured/filtering; no lossy
text conversion or ambient reopening is substituted.

The HTTP facet requires a declared exact HTTPS host **and** the ordinary explicit
`web_fetch` invocation allowance. URL credentials/non-443 destinations and
routing/proxy header overrides are denied. Plan denies methods other than GET/HEAD;
these methods are not a proof that a remote server is side-effect-free. No ambient
proxy routing, redirects, cookies, credentials or default user headers are loaded.
Request bounds: 16 KiB URL/32-byte method, 64 headers/64 KiB combined header text,
1 MiB body, 64 initiated attempts per call. Response bounds: 10 MiB body and
64 headers/64 KiB header text, without silent truncation/replacement. Failed or
oversized admitted attempts still count, denied/unapproved/expired requests do
not. Client/parse/transport diagnostics are fixed text, not raw URLs/OS errors.
Admission/response cases use a test-only synthetic transport; no external HTTPS
or live-model quality result is inferred from it.

`InvocationGate` now observes **typed** NeedsApproval from inner tool/middleware
outcomes and each source/import failure, never a message containing that word.
Under fail, the same invocation seal persists even if a guest, parent tool or
foreground child catches the error and returns success text. Later same-response
main/child tools and all main/subagent/direct-compaction provider calls remain
blocked; private runtime returns retained partial approval-required state, not a
completed turn or human suspension. Under deny, a caught unapproved write may
still choose a permitted read. Hard denials/plan and successful metadata do not
become approval failures. The opaque scope prevents substituting a fresh fail
latch; registration-before-check notifications wake all active waiters without a
lost-wakeup race.

Default guest limits are 30 s per operation/256 MiB linear memory, bounded to
positive <=300 s and 4–256 MiB settings. The operation budget covers source reads,
compilation waits, instantiation/describe/call and imports; expired admission is
checked before polling/spawning IO. Epoch checks bound guest CPU, and an outer
budget also bounds WASI sleeps/async imports. The approval notification wakes the
same guarded work even after a caught error followed by a CPU loop/WASI sleep,
without waiting for the deadline. No human budget pause is possible. Already
started blocking IO/compilation may outlive dropped waits: there is **no rollback,
portable writer CAS, native compiler/process memory quota, OS sandbox or whole-run
signal/deadline/final-result guarantee**. Native parsing/validation is bounded by
input limits and checked deadlines, not preemptively OS-contained.

WASI stores have no inherited environment/arguments/stdio, preopens or sockets.
Stdout/stderr/log imports drain into bounded 64 KiB private memory capture, never
a terminal/file; no untrusted direct terminal write is added. Raw guest return
JSON is bounded at 1 MiB and classified without guest-controlled error-string
matching. The private result/provenance/log object has no Serialize/ToolOutcome
adapter and is **not seeded redaction or safe publication**. It must not reach
model channels, checkpoints, journal, trace or result/events before redaction.

Tests include actual approved component execution with temporary roots, malicious
describe, byte preservation/non-overwrite, authority/root replacement, permission/
plan/hard-path separation, raw/typed error distinction, memory/log/JSON bounds,
post-instantiation CPU/WASI deadlines, stop notifications and started-worker
non-rollback evidence. Same-response main and explicit foreground-child probes
catch the guest failure yet still seal all roles/compaction. The app-only fixture
reuses WIT unchanged; SDK, old database/benchmark fixtures, legacy terminal host/
grants/catalog, model-request/frame goldens and trace-v1 are unchanged. Automatic
terminal catalog refresh still needs no model-driven list polling.

**Still open:** no automatic guest-host/tool/catalog registration in the private
shared builder, seeded guest/log/tool/error/storage redaction, compatible structured
origin publication, bounded instructions/attachments/stdin, real result/events/
trace producers, shared meter/admission (#97), typed providers and phase-specific
shutdown/verification. `craftsman exec` stays unexposed. This private host does not
clear live-quality/default-on, published deprecation, OS containment or full P2a
release gates.

CI follow-up: the initial source head passed formatting/macOS but Linux/Windows
caught the prompt-stop regression timing cold compilation/instantiation inside
its 10 s assertion. The fixture now separates bounded 120 s cold setup from the
unchanged 10 s **post-instantiation** stop assertion, verifies a typed import
NeedsApproval on the same latch and checks that the 30 s guest deadline has not
expired. Production host/budget code and limits are unchanged. No failed check
is bypassed; all four final-head jobs were rerun successfully before #103 merged.


## 18. Captured credentials and private seeded redactor — October 21, 2026 (Asia/Taipei)

PR #104 merged on October 21, 2026 (UTC), after all four exact-head CI jobs,
including Windows, passed; required review was the only admin-overridden gate.
After #103's green merge, preparation captures a bounded, immutable credential
snapshot BEFORE discarding disabled service declarations or opening session
storage. Known sources are the resolved provider key, configured platform token/
known fallback, configured search key, current model/global caller default-header
values and MCP header/environment values, plus their explicitly referenced
credential-source variables. Lookup visits each known name once; it never scans
the whole process environment or repository files. Nonconventional configured
sources (for example LANG when selected as a credential) are treated the same.
Unconfigured disabled services may have unresolved placeholders, but overflow
or unsafe capture is not silently ignored. No MCP/LSP/service activation, SDK/
grant/store writer, global env/config mutation or secret-valued diagnostic is
introduced by capture.

Active provider placeholders MUST resolve. Invalid/missing/oversized credentials
or unsafe seed sets fail with fixed text before session-store/provider side effects;
invalid invocation selectors are checked even earlier. A frozen ChatRoute carries
the captured resolved provider key into the SAME shared builder for main,
foreground child and direct compaction. A literal `${...}` inside a resolved
credential is not expanded again, and later environment changes cannot produce
a different auth value than the redactor's seed. Legacy frontend route resolution
is unchanged. Prepared routing Config keeps its original template so captured
shell credential-source names are not lost; no resolved Config is dumped.

Private `exec::redaction::Redactor` is seeded only by captured literal byte values.
It has no Debug/Serialize representation exposing seed or matcher state. Duplicate
values are coalesced deterministically. Constructor bounds are 256 supplied values,
64 unique nonempty seeds, 16 KiB per value and 64 KiB total unique seed bytes;
credential capture also bounds 64 known environment names/128 bytes per name,
32 KiB templates and 256 captured source values. Empty credentials add no pattern;
an empty seed set is a no-op, not evidence that arbitrary content is secret-free.
Short/single-byte seeds are not silently ignored. If a seed itself occurs in the
fixed `[REDACTED]` marker, construction fails closed instead of rendering the
credential as part of its replacement. This private conservative failure is not
a reason to change user credentials automatically or infer a safe public output.

The byte stream uses per-pattern prefix state and original-byte coverage intervals.
It delays at most the longest seed minus one original byte, so a key spanning
chunks cannot escape. **All overlapping/adjacent matches are covered as one
region**, not a longest-only replacement leaving another token's tail visible.
Replacement bytes are not fed back into matching; this is literal coverage of
captured values, not detection of transformed/encoded/arbitrary repository
secrets. Pending plaintext and interval/matcher state are bounded by seed limits,
not total stream length. Each push is at most 64 KiB, positions are checked and
finish is single-use. An incomplete unmatched suffix is released only at finish;
a complete terminal secret stays redacted. Different streams share only immutable
patterns, not pending data/coverage or mutable authority.

The text adapter preserves incomplete UTF-8 output across chunks rather than
lossy-replacing bytes. Captured credentials are UTF-8; a manually supplied binary
seed that would corrupt text causes a fixed failure and closes text output. Binary
IO retains non-secret invalid-UTF-8/NUL/CRLF bytes. This layer must run **before**
head/tail/log truncation, otherwise a partial credential cut can escape. It is not
itself that shell/guest capture adapter, ASCII machine encoder, terminal sanitizer
or CI-command-prefix defanger; none is implicitly supplied by calling a string
replacement helper.

Whole byte/text operations bound input AND expanded output at 1 MiB without
truncation. Structured untrusted JSON data redacts string values and member names
before serialization, preserves other value types, and rejects redacted-name
collisions instead of overwriting a member. Structured traversal limits depth to
64, work to 100,000 nodes, combined string/structural work to 1 MiB, and final
encoded output to 1 MiB; overflow/errors return fixed diagnostics, no partial
record or raw member name/value. A future typed public-schema adapter must keep
trusted structural field names distinct from untrusted maps; this generic helper
is not the schema-backed exec result/event writer.

Tests use synthetic seeds/lookup maps only: all split positions, one-byte chunks,
intersecting/adjacent/repeated prefixes, generated partitioned reference coverage,
terminal/incomplete matches, Unicode/UTF-8/BOM/NUL/CRLF behavior, independent streams,
short/duplicate/marker/boundary/budget seeds, fixed errors and structured collisions/
depth/work/encoded-size limits. Capture tests prove known-only once-per-name lookup,
provider/platform/search/default-header/disabled-service coverage, fallback and
nonconventional sources, missing/oversized capture without store effects and
selector precedence. A synthetic provider verifies the same literal captured auth
and header across actual main, foreground child and direct-compaction requests,
without process-global environment mutation.

**Scope at #104 and later:** #104 was the foundation only; section 19 now selects
it for private guest JSON and stdout/stderr/log cuts through the actual runtime
scope; sections 20–24 additionally select scoped shell/filesystem/web/concrete-framework
results, todo context, copied provider requests and fixed gate denials. Provider deltas,
alternative outcomes, original registry/inbox/state, checkpoints, journal, trace
and public result/events remain unselected/private/disconnected, not claimed redacted. Invocation inputs and original authorization arguments must
remain separate from redacted publication. Typed ToolMeta/fail-latch facts must
survive future adapters; redacted text never becomes permission evidence. No
callable guest/catalog registration or `craftsman exec` route is enabled. Compatible
structured source-origin publication, bounded instructions/stdin/attachments,
real create-only output/trace/usage producers (#97 before meter), typed providers,
P2g shutdown and live/default-on/release gates remain open. SDK/WIT/framework,
legacy host/grants/catalog/frozen frames/goldens/trace and original fixture data
are unchanged; deterministic terminal gadget refresh still needs no model-driven
list polling.


## 19. Private guest JSON and shared-log adapters — October 21, 2026 (UTC)

PR #105 merged after corrected final-head fmt/Linux/macOS/Windows jobs all passed,
with review as the sole admin-overridden gate. Its initial Windows baseline failure
was fixed and all four jobs rerun, not bypassed. Section 20 now adds private shell
views; other tool/provider/storage/public producer boundaries remain open.

After #104's green merge, `gadgets::headless::GuestHost::from_scope` selects the
frozen `Prepared.credentials().redactor()` through its existing opaque lease of
the actual runtime. There is no Prepared/arbitrary-gate/arbitrary-redactor host
constructor. Source approval, original ConfinedFs/policy and the SAME typed all-role
fail latch still precede compilation/empty-capability describe and every import.
Neither the normal private builder nor a CLI/frontend opts into callable tools.

**Guest JSON:** raw arguments, input schemas, path/payload and authorization are
consumed unchanged by the guest. Only the completed return view is redacted:
string values AND member names before serialization, key collisions/depth/work/
record/encoded-size failures without a partial result or guest-valued diagnostic.
The 1 MiB input/output and section 18 structured work bounds remain. After encoding,
a literal-match check rejects a captured value in a numeric/boolean/null, syntax or
new replacement/escaping boundary instead of corrupting JSON types. This is a
conservative private output failure, not permission evidence or a transformed/
arbitrary repository-secret detector. A future public-schema encoder must keep
trusted structural fields distinct from untrusted guest data.

**Shared logs:** the private host no longer selects legacy `host::Capture`, which
cuts original bytes and lossy-decodes. Separate `headless::output::RedactedCapture`
shares one locked original-byte order across stdout, stderr and host-log imports.
The original-byte stream redacts literal overlapping/chunked seeds. A SECOND delayed
stream checks the rendered bytes and rejects a captured credential reintroduced at
replacement boundaries. Only verified bytes enter 64 KiB bounded retention; neither
matcher releases a prefix before enough original/rendered lookahead exists. Matcher
holdback is bounded by `2 × (longest_seed − 1)` (at most 32 KiB); it is not
unbounded plaintext history. No seed/matcher Debug/Serialize or raw capture accessor
exists, and nothing spills to disk.

Once retention is full, both delayed suffixes are discarded, `logs_truncated`
reflects omitted data, and later writes are drained without unbounded matching or
storage. Each capture write is at most 64 KiB; a host-log payload reserves 16 bytes
for its fixed level/newline inside that bound. Oversized import records fail fixed
before prefix allocation/retention; a guest cannot hide a capture failure by returning
successful JSON. Completed capture validates UTF-8 with no lossy substitution. If the
retention cut alone splits the final non-secret scalar, only its incomplete suffix
is omitted with truncation recorded; otherwise invalid retained bytes cause a fixed failure.
NUL/CRLF/BOM/control bytes are still data, not a terminal sanitizer/ASCII encoder or
CI-command defanger.

**WASI ordering and closure:** native synchronous WASIp2 OutputStream writes replace
the default background AsyncWriteStream queue in this private host. Independent
handles get the same bounded 8192-byte permits, write immediately into the SAME
capture, and recheck the actual gate/budget before AND after acceptance. A granted
permit cannot bypass a later approval seal. No inherited stdio, queued late writes,
asynchronous flush worker or capacity-dependent deadlock is introduced. Readiness
is immediate; a stopped stream reports a fixed error. Async writers also share this
capture; flush and per-handle shutdown/cancel NEVER finalize the shared matchers or
release a credential prefix. Natural completed exports finalize once. Failure/abort
closes without releasing either delayed suffix; `ByteStream::abort()` reports only
whether data was omitted and `matched()` only a monotonic literal-match fact.

**Authority and metadata:** approval-required/expiry still outrank formatting faults;
redaction errors never forge NeedsApproval or latch deny/plan/success metadata.
Private original approval records and description/schema metadata remain unredacted
and unpublished; the result still has no Serialize/ToolOutcome/public producer.
Trusted duration/network counts and truncation flags stay separate from redacted
text. This does not roll back started blocking IO/compile, OS-contain native commands,
implement P2g shutdown, or complete the provider/tool/storage/result/trace boundaries.

Synthetic tests cover all split/cut positions and overlapping seeds, shared stdout/
stderr/import order, flush/shutdown/permits and post-seal writes, UTF-8/BOM/NUL/CRLF,
invalid bytes, cap draining/oversize/abort/single finish, replacement collisions,
JSON scalar/syntax/member/depth/work/size failures, actual source-built guest output,
unchanged original writes and frozen configuration seeds. Existing strict caught-import
and CPU/WASI post-readiness assertions remain; test budget changes rebind the SAME
capture only through test-only helpers. Source fixture exports exercise these cases
without changing SDK/WIT/old fixtures or benchmark data. No live-model/default-on/
release evidence follows from them. Final-head fmt/Linux/macOS/Windows success,
including individual informational Windows, remains mandatory before merge.

**Still open:** guest ToolOutcome/catalog integration and compatible structured
source-origin publication, other provider/model/tool capture adapters (section 20
now selects the scoped shell path), seeded checkpoint/journal/trace/result/events,
bounded instructions/stdin/attachments,
typed providers, #97 before a shared meter, P2g and live/default-on/release gates.
`craftsman exec`, raw durable turn writers and callable headless gadgets remain disabled.
Legacy host/Capture/grants/catalog/defaults, standard ACP/frozen desktop frames,
framework dependencies, schemas/goldens/trace-v1, SDK/WIT and old fixture data are
unchanged. Deterministic terminal catalog refresh still needs no model list polling.


### Windows CI fixture follow-up for section 19

The first #105 head passed fmt/Linux/macOS and the Windows gadget-enabled suite,
but Windows's feature-off `acp_bridge_http_and_relay` failed after the health and
unauthenticated-stats frames: the first authorized `/stats` cold home walk exceeded
its 30 s HTTP test bound. The configured HOME/USERPROFILE fixture is not a portable
override of Windows native profile discovery; metrics may walk runner caches.
The fixture now separately prepares and validates one real authorized stats response
within a bounded 60 s setup before the original recorded 30 s request. No retries,
synthetic stats, removed auth/schema/relay assertions, changed seven-step golden,
production home resolver/cache/deadline or 120 s scenario bound is introduced.
Full final-head fmt/Linux/macOS/Windows checks must rerun; informational Windows
failure is never admin-overridden.


## 20. Private scoped shell-output redaction — October 22, 2026 (UTC)

PR #106 merged after all four exact-head jobs and every mandatory test/golden step
succeeded (Linux/macOS 835 feature-off / 988 gadgets; Windows 815 / 957), with review
the only admin-bypassed gate. Failed/aborted local evidence below was not relabeled
green. Section 21 now additionally selects private filesystem views and fixed denials.

After #105's green merge, the SAME session builder issues an opaque `agent::ShellScope`
only from its existing Headless construction branch. Its private fields pair original
Prepared workspace/settings/captured credential patterns with that branch's ACTUAL
InvocationGate. ShellRunner's private constructor consumes this lease; it does not
accept arbitrary roots/settings/redactors/approval callbacks. Frontend construction
still uses the unchanged legacy runner. Main and foreground children share the same
scoped runner, permissions and fail latch; no callable guest/catalog/exec is enabled.

Before native spawn the exact ORIGINAL command is checked by the invocation gate.
Empty, missing and non-string command arguments also reach this scoped runner, rather
than a raw HostTool validation message: fixed InvalidArgs metadata has no spawn byte/exit
facts and the next model round sees only safe text. Legacy frontend empty-command
validation is unchanged. Deny/plan/unapproved rules are not rewritten through redacted
text. Source names stay captured before disabled services were cleared and use the
existing strict scrub.
Every process is still spawned/killed through platform_os::ProcessTree: existing
Unix group configuration (default off), Windows Job Object rules and bounded
post-kill drain are unchanged. A latched approval failure also interrupts an active
shell and prevents subsequent spawn; dropping active work is not native OS containment,
rollback, signal routing or the P2g whole-run shutdown contract.

Each private stdout/stderr pipe feeds ORIGINAL bytes into two bounded literal streams:
redaction first, then delayed rendered-literal verification. Only verified output
enters bounded head/tail retention, so a key crossing a read chunk or head/tail cut
cannot leave its original tail. Original and rendered UTF-8 are validated incrementally
without lossy decoding; invalid original bytes fail immediately, even if redaction
lookahead would otherwise delay them until a later timeout. Invalid UTF-8/read/render
failures stop the command with fixed output-error metadata, not guest/OS-valued text.
Matching state and UTF-8 suffixes have no Debug/Serialize/raw accessor or spill authority.

The existing per-pipe ORIGINAL read ceilings remain: legacy 100,000 bytes and head_tail
64 MiB. Legacy retains at most 100,000 redacted bytes; head_tail retains 50,000 head and
50,000 tail bytes. Matcher holdback is bounded by twice `(longest_seed − 1)` plus small
UTF-8 suffixes, not total stream length. EOF alone does not finalize: only EOF AND
normal command completion releases unmatched holdback. Read caps, timeout, cancellation,
errors or approval stop discard delayed suffixes and record truncation. Safe retained
UTF-8 head/tail boundaries are chosen before text formatting; no late spill writer exists.

Formatting and rendered-literal checks precede the final model text budget (50,000 bytes).
Known values introduced by section/exit/timeout/omission diagnostics are redacted as text,
never treated as policy or numerical authority. Rendered omission counts describe the
retained text; ToolMeta.bytes separately records ORIGINAL observed pipe bytes, not
redacted character counts. Original exit status, duration and typed Spawn/Timeout/
NeedsApproval/Failed facts stay separate. Failed/oversized/unsafe publication falls
back to the fixed `[REDACTED]` marker (constructor-safe), with truthful failure/truncation
metadata; it does not fabricate successful completion, provider requests or token usage.

**Conservative current limits:** stdout/stderr are distinct logical pipes, not an
invented cross-stream chronological audit. The old `clean_output` transformer runs
after capture and could re-create/cut credential values. Until a bounded streaming
cleaner exists, this PRIVATE path rejects clean_output=true before process effects
with fixed InvalidArgs; it is not silently ignored, and other frontends retain their
existing cleanup. Non-UTF-8 Windows/OEM output fails closed; explicitly configured
powershell_utf8 remains the existing option, not a new default. This does not detect
arbitrary transformed/encoded repository secrets or provide terminal/ASCII/CI-command
sanitization to the public result/event producer.

Tests use synthetic credentials and the real shared builder, not a substitute
Prepared/gate/seed constructor. Coverage includes all chunk/cut positions, overlapping
and replacement-boundary values, strict original/rendered UTF-8, bounded retention,
EOF versus read cap/timeout/abort, original path/payload effects, actual raw byte/exit
metadata, active same-gate approval stop, no later spawn, deny/plan, unsupported cleanup,
fixed spawn errors, frozen config, private argument errors versus unchanged frontend
validation, and actual main/child shell results before the next
synthetic model request. Fixture process groups are explicit cleanup configuration,
not a production default or OS guarantee. Root dependency/lock/pin, SDK/WIT/framework,
legacy capture/formats/terminal defaults, standard ACP/frozen desktop frames, schemas/
goldens/trace-v1 and original fixture corpus stay unchanged.

**Still open:** section 21 now selects filesystem/local-search views; web and
alternative framework-contributed tool results still need their own pre-cut adapters.
Provider deltas/assistant state, guest ToolOutcome/catalog
and compatible origin publication, seeded checkpoint/journal/trace/result/events,
bounded input, typed providers, #97 before the shared meter, P2g and live/default-on/
release gates. Headless human output and raw durable turn writers remain disconnected;
`craftsman exec` and callable headless gadgets remain unavailable. Passing shell tests is
not a full outbound-channel guarantee or a completed P2a release.


### Local verification note for section 20

The initial full local run also hit native launch stalls: the new six-second partial
shell test observed zero bytes, while unchanged legacy tests saw a three-second sleep
exceed all ten-second completion attempts and no started grandchild in the flag-off
control. The completed main binary/failed assertions are not labeled green, nor is
the subsequent pre-test alias launch stall. The partial fixture now requires an actual
ready-file signal AND all eleven original bytes before it can prove suffix redaction;
at most three bounded unready attempts are allowed, with the SAME six-second call
budget and every timeout/error/count/content assertion retained. No production timer
or legacy assertion is widened. Fresh final-head full CI on all platforms remains
mandatory, including individual Windows success.


## 21. Private scoped filesystem views — October 22, 2026 (UTC)

PR #107 merged after all four exact-head jobs and every mandatory test/golden step
succeeded (Linux/macOS 870 feature-off / 1,023 gadgets; Windows 849 / 991), with review
the only admin-overridden blocker. Failed/aborted local full runs below were retained,
not relabeled green. Section 22 now additionally selects private web views.

After #106's four-job green merge, the existing Headless branch of the SAME session
builder issues an opaque `agent::FilesystemScope`. Its private fields pair Prepared's
original retained ConfinedFs/captured seeds with that builder's unique InvocationGate.
`confined_fs::TextFs::from_scope` has no arbitrary filesystem/path/gate/seed constructor.
The main backend and bound main/delegated filesystem host tools use this adapter;
terminal/transport RealFs and the byte-oriented guest facet/legacy host remain unchanged.
No public exec, callable guest/catalog, raw store/trace writer or new protocol is enabled.

### Original authority versus views

All six filesystem operations retain ORIGINAL paths, queries, edit old/new strings,
write bytes and exact policy decisions. They reuse the same no-follow directory/file
handles, single-link checks, identity/content validation, create-only publication and
serialized blocking workers. They never open a redacted alias, reverse-map a marker,
reopen an ambient path after checks, widen a grant or derive a latch from displayed text.

For reads/local grep, original UTF-8 is validated after the complete bounded file
snapshot is read. `redaction::MaskedText` uses the SAME literal KMP/coverage core as the
existing byte stream, recording one coverage bit per ORIGINAL byte (at most 1.25 MiB
for a 10 MiB file), not one allocation per match. Coverage is complete BEFORE selecting
a page or matching-line view. A credential spanning lines/CRLF/page boundaries cannot
leave its selected middle/tail. Each covered run in an original line renders a marker;
original line indices and Rust's line semantics stay separate, not renumbered through a
shorter redacted string. Original query matching uses original text/names, not markers.
Only bounded safe chunks are emitted; no whole 9× expanded file is materialized.

Source-body rendered-literal verification precedes framing/retention and rejects
replacement-boundary collisions. Framing, original line numbers, empty-result query
echoes and write/edit summaries then pass through literal redaction PLUS delayed
rendered verification before the 100,000-byte text budget. Complete logical EOF alone
finishes streams. Forced output cuts abort all held suffixes; a safe UTF-8 head and
redacted truncation notice are checked again as the exact published text, with no late
unsafe clamp. Privacy/UTF-8/resource collisions fail fixed, never lossy-replace data.
No raw Debug/Serialize accessor or spill/store writer exists for source/matcher views.

Names remain whole independent records before field/output bounds. Returned name fields
are redacted display labels, NOT reusable authority aliases. Original sorting and size
facts are retained. Colliding redacted names/path labels fail closed rather than silently
misattribute data; arrays fail their aggregate rendered bound instead of returning a
truncated filename. Bound host-tool text may show an explicitly truncated safe prefix.
Explicit IO to a literal marker-named file still targets that exact original file.

### Bounds and trusted facts

Original authority bounds stay unchanged: 10 MiB per file/read/write operation,
64 MiB aggregate successful grep reads, 100,000 traversal candidates, 8 MiB walk-name
storage, 16 MiB glob-match work, 100,000 original returned name bytes, 4096 path scalars
and depth 128. The guest facet still has its separate 1 MiB write / 1024 ALL-entry bound.
Grep keeps its established binary/NUL-prefix/invalid-UTF-8 skip behavior and charges
successfully read bytes even for skipped/nonmatching files. It cannot evade the aggregate
limit with seven large nonmatching sparse files. Read errors remain fixed typed errors.

`ToolMeta.bytes` records original successful source reads for read/grep, or original
returned name bytes for list/glob; it is not redacted character/token usage. Failed reads
without a complete observed snapshot do not invent zero bytes. Mutation summaries do
not invent process exit/network/byte facts. Duration/truncation/typed error kinds stay
separate from views. Protected-write/missing-tool NeedsApproval is observed through the
SAME gate BEFORE formatting, even if a direct backend caller catches the eventual safe
AgentError. Denied/plan/ordinary source strings are not false approval failures.

The builder now constructs `InvocationGate::captured(Prepared)` so fixed gate denials
also use frozen seeds before an earlier middleware/terminal short-circuit could bypass
a producer. Permission-only seedless construction is test-only; actual construction
never accepts a separate redactor or re-expands environment/config. This selects ONLY
fixed gate diagnostics, not arbitrary middleware/provider/error/state output bodies.

### Limits and remaining gates

Already-started blocking IO may complete after a fail latch; this is not rollback,
external-writer CAS, native OS containment, a fast cancellation/deadline guarantee or
P2g shutdown. Unix process groups/defaults and Windows Job Object rules are unchanged.
The existing child framework context backend remains in-memory; delegated filesystem
HOST tools hold the same scoped app backend. Alternative framework-contributed tools
and their own data/formatters still require separate adapters, not a claimed universal
post-ToolOutcome scrub. Main direct-backend reads/errors use the scoped view boundary.

Section 22 now selects web fetch/search body/HTML/JSON views; alternative framework task/todo/message outcomes,
provider deltas/assistant state, redacted checkpoint/journal/trace/result/events, reviewed
compatible gadget provenance/ToolOutcome/catalog, bounded stdin/input, typed providers,
#97 before a shared meter, P2g and live/default-on/release gates remain open. Observed
30/cap 100 with next estimate 80 plus reply ≥1 must refuse before admission without
fabricating measured usage or requests. `craftsman exec`, callable headless gadgets and raw
durable turn writers remain unavailable. Passing private tests is not a P2a release.

Tests accompany the original multiline/page/CRLF/Unicode/line/query semantics, byte
and output bounds, original mutation payloads, name collisions/non-alias IO, source
replacement failures, binary/search budgets, typed caught failures, retained-root
rebinding and real same-gate shell stop. Synthetic main/child model rounds verify
selected tool messages before the next request, not live quality or all outbound
channels. Full final-head fmt/Linux/macOS/Windows success remains mandatory, including
individual informational Windows, before any review-only admin merge. Framework/root
lock/toolchain, SDK/WIT/old fixtures, frontend defaults/model definitions, standard ACP/
frozen desktop frames, schemas/goldens/trace-v1 and historical baseline/targets stay intact.

### Local verification observations for section 21

Initial focused matcher/scoped-filesystem/formatter/fixed-denial checks and production
feature-off typecheck passed. The first full local baseline was NOT green: its main
binary reported 672 passed / 3 failed / 2 ignored. The new same-gate filesystem stop
fixture and the unchanged shell stop fixture missed their real 20 s startup-ready
markers; an unchanged invalid-pipe fixture returned Timeout rather than its required
Failed fact. The subsequent zero-test legacy-alias launch had not started its test
harness after several minutes. These observations are recorded, not substituted for
successful startup/UTF-8/fast-stop evidence. The focused stop test had passed separately;
no fixture/production limit, legacy assertion or OS service was changed to paper over
the full run. Later source assertions and final-head full platform checks must be
verified independently, including individual Windows success before any admin merge.


## 22. Private scoped web-output views — October 22, 2026 (UTC)

PR #108 merged after all four exact-final-head jobs and EVERY mandatory step succeeded
(Linux/macOS 908 feature-off / 1,061 gadgets; Windows 887 / 1,029). Missing review was
the only admin-overridden blocker; tested and merged trees were identical. Failed/
aborted local evidence below remains non-green, not a claimed native build/lint pass.
Section 23 now additionally selects concrete built-in framework views.

After #107's verified green merge, only the Headless branch of the SAME session
builder issues `agent::WebScope`. Its private fields pair original immutable Prepared
configuration/captured seeds with that builder's unique InvocationGate. `exec::web::Web`
accepts only this lease, never arbitrary URLs/clients/gates/seeds or later config/env
reloads. Bound main/foreground-child web producers are replaced before legacy body,
HTML, error and snippet cuts, while original trusted tool definitions stay unchanged.
Normal WebFetchTool/Brave/DuckDuckGo/service-agent behavior and frozen frontends stay
unchanged. No public exec, headless guest/catalog registration or raw writer is enabled.

### Original request authority and transport facts

Original fetch URLs and search queries are never substituted with redacted strings.
Search keeps the established trimmed-query semantics and 2048-byte query limit. Its
key is the SAME configured literal captured in Prepared (current search clients pass
it literally), not recursive/new environment expansion. Supported private search is
configured Brave; missing/unsupported/invalid configuration fails fixed before any
attempt, never silently chooses a keyless provider or fakes empty results. A test-only
loopback endpoint field exercises actual builder/model rounds and has no production
routing effect. Provider/default/MCP headers are not ambient fetch credentials.

The private client has no proxy, redirect, cookie-store or credential defaults, and no
fallback client. Fetch is explicit HTTP(S), as originally supplied: unlike the legacy
interactive upgrade helper, it does not silently rewrite non-loopback HTTP to HTTPS.
URL credentials/non-HTTP schemes/literal link-local targets (including mapped IPv4)
are rejected. Up to five fetch redirects are followed manually, with original-target
permission rechecks before each initiated attempt; redirected literal private/unique-
local targets are also rejected. This is an application URL boundary, not DNS pinning,
a public-IP-only OS sandbox or a claim that approved GETs have no remote side effects.
Search's captured auth goes only to its fixed provider and redirects are never followed.

One 15 s whole-call deadline and client timeout bound network/body work. The same gate
interrupts an actually admitted pending request and forbids later attempts, including
when another scoped operation's typed NeedsApproval causes the seal. Owned request/
body futures are dropped BEFORE their final facts are read; cancellation does not
promise rollback or that an already-sent request was not received remotely. A final
same-gate recheck after success/error framing discards a late-stopped view and reports
typed NeedsApproval while preserving original attempt/byte/duration facts. No human
budget pause, provider traffic or shared token meter is introduced.

`ToolMeta.network_calls` counts initiated attempts, including failures/redirects, not
remote receipts or success. `ToolMeta.bytes` counts original received body chunks, not
redacted text bytes; a final network chunk can contain more bytes than the admitted
read prefix. Unread unsuccessful-status bodies and pre-body rejections do not fabricate
zero observed bytes. Original HTTP status remains a private typed fact and its diagnostic
view is redacted separately. Duration/error kind/truncation survive privacy failures;
no exit status, usage or admitted provider request is invented.

### Capture and transformations before cuts

Fetch admits at most 200,000 original body bytes. Original UTF-8 is validated BEFORE
literal lookahead could hide an invalid byte. Original-byte redaction and a delayed
rendered-literal check precede parsing/retention. Real EOF alone finishes matchers;
read caps/output cuts/cancellation/errors discard all original/rendered/UTF-8/parser
holdback. A strict bounded HTML state machine drops tags/script/style, handles quoted
tag boundaries, named/numeric entities, and bounded blank-line/trailing-space state.
Decoded angle brackets are text, not new executable tags. Tag/whitespace pending state
is capped at 4096 bytes and entity state at 64; malformed/oversized state fails fixed.
It is a rough text view, not a DOM or general-purpose HTML/terminal sanitizer.

HTML entity decoding, tag removal and whitespace decisions can re-create a credential.
Their output therefore passes through ANOTHER literal-redaction + rendered-verification
stage BEFORE bounded text retention. Plain text skips HTML normalization. Complete
JSON-like fetch bodies remain text views, not an invented typed JSON authority.
Non-UTF-8/OEM/other encodings fail closed; no lossy replacement/default encoding change.

Search admits a complete original JSON record up to 512 KiB, with original UTF-8/literal
matching before input admission. Oversized/partial records fail rather than publish an
unredacted or repaired JSON prefix. Original parsing metadata stays private; the ENTIRE
untrusted JSON value is structurally redacted (including keys) and encoded-literal
checked before result selection. Key collisions, scalar/boolean/null/syntax/replacement/
escaping/resource conflicts fail fixed instead of corrupting types or treating malformed
schema as “No results.” Original schema keys select original result fields; they are not
renamed authority aliases. Only an actual empty provider results array is empty success.
Original duplicate provider keys retain serde_json's existing parser behavior; this is
not a custom duplicate-key parser or the machine result/event contract decoder.

Titles/URLs are redacted/verified as complete fields. Snippets also undergo full-field
redaction AFTER whitespace collapse and BEFORE their 300-scalar cut. At most ten results
are selected after whole-record validation; result/snippet omissions have truncation
facts. Ranked framing and ellipses pass through the same publisher. Untrusted provider
error bodies are not echoed or truncated into diagnostics; the typed fixed failure is
rendered separately. This does not parse arbitrary alternate provider schemas.

`redaction::publication` now shares §21's unchanged bounded text codec between FS and
web. Literal redaction plus delayed verification precedes the final 100,000-byte model
text budget (below/equal to the existing result middleware budget). Forced cuts abort
suffixes, and the exact safe UTF-8 head/truncation-notice join is checked again. There
is no raw source/matcher Debug/Serialize/accessor or spill/store authority. Framework/
SDK/WIT/root lock/pin/old fixtures, FS/shell/guest behavior, model definitions, standard
ACP/frozen desktop frames and schemas/goldens/trace-v1 remain unchanged.

### Evidence and remaining boundaries

Pure and actual-builder tests cover split keys/UTF-8, original read versus model caps,
parser states, entity/tag joining, complete JSON/keys/scalars/normalization/snippet/result
cuts, original URL/query/literal auth, immutable config, no default fetch headers,
redirect admission/counts, setup/errors/bounds and same-gate stop after real HTTP receipt.
A deterministic late-publication test seals after actual successful/invalid-UTF-8 HTTP
results were assembled, then exercises the final check without fabricating facts.
Synthetic main/child model rounds inspect selected tool text before the next request,
not live quality or the whole serialized provider/state/output channel. Initial HTML
fixtures used WireMock's string body MIME; explicit `set_body_raw(..., "text/html")`
corrected the real header without changing production detection. A truncated short
safe prefix was legitimately held; the fixture now supplies a longer real released
prefix, not finish-on-cut, wider limits or fabricated output.

Local focused verification passed 58 web tests, including all 38 added in this slice;
the unchanged shared filesystem publisher tests passed 5 and the captured-redactor
regressions passed 40. An earlier full feature-off local run was NOT green: the main
binary passed 711 tests but failed the existing shell/FS real-startup assertions, then
the workspace run stalled and was aborted at its 600 s outer bound. These startup
failures are not UTF-8/stop evidence and no fixture/production limit or OS service was
changed to hide them. The later full local gadgets run was also NOT green: its main
binary had 852 passed/3 failed/3 ignored, including the same real-startup assertions
and invalid-pipe UTF-8 returning Timeout instead of Failed, before a 600 s abort.
Default voice-enabled workspace check and all-target Clippy were separately aborted
at 240 s and 300 s without success; no successful native build/lint or full-suite claim
follows from those partial logs. Final-head full platform checks remain independent
requirements; failed local assertions are not successful UTF-8/fast-stop evidence.

Section 23 now selects concrete built-in task/todo/message result views and todo
context. Alternative middleware outcomes, registry/inbox/provider deltas/assistant/request
state and serialization, redacted checkpoint/journal/trace/result/events, reviewed
compatible gadget provenance/ToolOutcome/catalog, bounded input, typed providers,
#97 before a shared meter, P2g and live/default-on/release gates remain open. Observed
30/cap 100 with next estimate80+reply≥1 must refuse before admission without fabricated
usage or requests. `craftsman exec`, callable headless gadgets and raw durable turn writers
remain unavailable. All four final-head CI jobs AND mandatory steps, Windows included,
are required before any review-only admin merge; this private slice is not a P2a release.


## 23. Private concrete framework-output views — October 22, 2026 (UTC)

PR #109 merged after all FOUR exact-head jobs and EVERY setup/test/golden/cleanup
step succeeded (Linux/macOS 938 feature-off / 1,091 gadgets; Windows 917 / 1,059).
Missing review was the sole admin-overridden blocker; tested/merged trees matched.
Local non-green evidence below is retained, not relabeled green. Section 24 now
selects copied outgoing provider requests, not original state or response publication.

After #108's four-job green merge, only the Headless branch of the SAME session
builder issues `agent::FrameworkScope`: frozen Prepared seeds and that builder's
unique InvocationGate, never arbitrary config/seed/gate or later reload. A second
opaque `SubagentOutputSource` is issued after the actual builder installs the SAME
all-role tools/providers/child gate. The framework view consumes this concrete source
only when its gate identity matches; it cannot accept a caller-built middleware from
a name or arbitrary specs/gate. The app-layer adapter adds no framework/SDK/WIT or
root dependencies, lock/pin, model definitions, frontend, schema or protocol changes.
No public exec, callable headless gadget/catalog, raw writer or P2g shutdown is enabled.

### Known producer selection, not a universal string scrub

`exec::framework` owns the concrete built-in TodoListMiddleware source/factory and
wraps the builder-issued SubAgentMiddleware task/message source. The wrapper travels
with those producers before ResultBudgetsMiddleware's existing 100,000-byte cut; there
is no separately excludable scrub that could leave a raw producer behind. Todo removal
still removes that capability. Unknown profile/middleware tools and their own hooks/
formatters are NOT identified or trusted from a matching name, and are not claimed
redacted by this slice. The existing all-role permission gates still apply to them.

Concrete todo factory materialization shares only frozen policy/seeds, not state or
stream holdback. An explicitly composed profile can reuse it during the framework's
normal child compilation, but no todo/delegation/message capability is automatically
added to a role. Normal frontend factories/definitions/order and child tool sets remain
unchanged. Main runtime, real synthetic provider rounds and a separately composed
scoped child-factory fixture exercise these distinct paths; fixtures are not new public
configuration flags, live-model evidence or arbitrary producer constructors.

### Original typed todos before output JSON encoding

The private todo producer uses the SAME Vec<Todo>/TodoStatus parser and original state
assignment as the built-in, without feeding redacted arguments back into it. Complete
original `todos` input is admitted before clone/parse: a memory-only JSON byte counter
is capped at 1 MiB, with an iterative 64-depth/100,000-node check before recursive
counting. It retains no raw encoded buffer, prints nothing, and is not a storage/IO
writer. Oversize/deep/work overflow fails, never parses a JSON prefix. Malformed/missing/
non-string/unknown-status inputs fail fixed InvalidArgs without raw serde diagnostics.
Other invocation/model input remains a separate unimplemented boundary.

After successful original parsing, original typed todos are assigned to state. The
output view structurally redacts complete decoded content before JSON escaping and
verifies the encoded record. Fixed `content`/`status` schema fields and original enum
status values are not renamed authority aliases: key/status/scalar/syntax/replacement/
escaping conflicts fail fixed. The exact prefix/JSON framing join is checked again;
it cannot silently redact JSON syntax. Todo result JSON is whole or fixed failure at
the shared text budget, never a truncated/repaired array or shortened field alias.
A privacy failure after assignment is not rollback or a fake successful state update.

The known todo middleware also appends complete plaintext context from original state.
Restored/original typed state gets the same byte/work admission without cloning BEFORE
the known formatter can allocate that section, not just when a new write_todos call runs.
Only its newly appended section is redacted/verified before bounded retention; original
todos and other existing context are untouched. A real cut aborts holdback and carries
the shared truncation notice; an unsafe section fails without leaving its raw view in
system_extras. This is not whole request/state sanitization: original assistant call
arguments, provider/model messages, other system extras and persisted state remain
private/unselected. Synthetic tests inspect the selected tool/context fields separately.

### Task/message text, lifecycle and typed facts

The concrete foreground task/message tools still execute ORIGINAL arguments. Child
prompts, type/id lookup, file/state merge, registry lifecycle and notification/inbox
recipient/payload/delivered semantics remain unchanged. Unknown-role output keeps the
original classification but does not invent a spawn/request/completion; background/
resume stays unavailable. Task return text and message acknowledgements are complete
plaintext records before the framework cut, not arbitrary typed JSON to repair.
One MiB complete-source bounds precede the unchanged shared redacting/verifying text
publisher; forced cuts abort delayed suffixes. Already-truncated source metadata is
rejected, not finished as a complete record. Original error text is never an authority
string; untyped raised errors fail fixed instead of echoing raw Debug/Display.

Actual ToolMeta bytes/duration/exit/initiated-attempt facts are preserved, never inferred
from redacted text or fabricated when absent. Genuine typed error NeedsApproval is
observed before view work, and final publication rechecks the SAME latch even when a
child failure was caught as plain text. Success metadata and strings naming approval
remain non-latching; a failed success-view must use its privacy failure kind, not promote
stale success metadata to approval authority. Later todo/task/message effects are denied
by the original gate. A foreground child is not dropped to fabricate a terminal registry
status; existing provider before/after checks and actual lifecycle completion remain,
not a new immediate cancellation/OS-containment guarantee.

Task definition/available-agent metadata, the registry's original 100-scalar progress
preview, notifications/inbox injection, child assistant/provider text and all-state/
serialization/persistence/result/event/trace channels are separate unselected producers.
Preserving ORIGINAL message delivery deliberately does not claim its raw queued payload
or next model request is already redacted. Only these unfinished adapters' synthetic
fixtures run; exec/callable gadgets/raw durable turn writers remain unavailable.

### Evidence and remaining boundaries

Actual-builder lease tests and pure codecs cover original parsed state versus views,
escaped complete fields, schema/status/replacement/framing collisions, encoded input/
depth/work bounds, whole JSON versus text cuts, immutable config, concrete task/file/
recipient/inbox semantics, scoped factory materialization, cross-invocation source
rejection, trusted metadata and late same-gate stop. Synthetic actual main/child model
rounds inspect selected result/context text before the next request, original auth/
prompts and caught child failure sealing later same-response effects, not live quality.

Final local focused tests passed all 30 new cases in both feature-off and gadgets
builds; both production checks passed without compiler warnings. Unchanged web,
captured-redactor and shared-FS publisher regressions passed 58/40/5. Offline accepted
contract cases (126), benchmark driver tests (9), formatting/whitespace/lineage and
six-document fence/local-link checks passed independently of full platform CI.

Strict feature-off all-target workspace Clippy was NOT green: it failed (101) on
existing async_trait-generated double_must_use diagnostics in untouched framework
traits. A separate warning diagnostic run found no findings on new/changed source
lines after a test MutexGuard was placed in a lexical block; this is NOT strict lint
success, and no lint suppression, dependency/framework edit or toolchain downgrade
was used to hide baseline warnings. An earlier full local feature-off run (before
the final restored-state guard) completed all 21 test summaries in 563.51 s with
935 passed/2 failed/3 ignored. The unchanged shell/FS real-startup assertions failed;
that is not successful startup/fast-stop evidence and NOT a green baseline or abort.
No native assertion, production deadline or OS service was weakened. Full local
native retries/voice checks were not queued to paper over the previously documented
startup/abort evidence. Final-head full CI remains an independent mandatory gate.

Section 24 now selects copied final provider request views. Alternative producer hooks/
cuts, original registry/progress/inbox/assistant state, response/transport, seeded checkpoint/journal/result/
events/trace, compatible gadget origin/ToolOutcome/catalog registration, bounded
invocation input, typed providers, #97 before the shared meter, P2g and live/default-on/
release gates remain open. Observed30/cap100 with next estimate80+reply>=1 must refuse
BEFORE admission without invented usage or requests. Require all FOUR exact-final-head
CI jobs and every mandatory step, informational Windows included, before a review-only
admin merge; failures/aborted native runs are never relabeled green.


## 24. Private all-role provider request views — October 22, 2026 (UTC)

PR #110 merged on October 23, 2026 (UTC) after corrected exact-head fmt/Linux/macOS/
Windows jobs and EVERY mandatory step passed. Main-unit counts were Linux/macOS770
feature-off /911 gadgets and Windows751 /881, all zero failed. Missing review was the
sole admin override; tested/merged tree identity was verified. Initial Windows/local
failures remain recorded below. Section 25 now selects HTTP/error diagnostics only.

After #109's verified merge, only the Headless branch of the SAME session builder
issues `agent::ProviderScope`. Original Prepared/captured seeds, effective provider
options/model/declared agent identities, per-role usage Arc and the actual unique
InvocationGate are paired there, never caller config/client/gate/seeds or later env
reload. Main, foreground-child and direct-compaction roles select the private provider;
normal frontend/service-agent StreamingLlmProvider and wire shapes remain unchanged.
No public exec, headless callable gadgets/catalog or raw persistence writer is enabled.

### Checked request copies, original control/state/auth

`exec::provider::request` builds a separate request view from ORIGINAL ModelRequest
fields after middleware preparation and before historical argument strings or HTTP
JSON encoding. System/user/assistant/tool text, decoded historical argument values,
known schema annotation text and declared metadata values are redacted/verified as
complete records. Original messages/state/dispatch/file paths/command bytes/recipient
payloads/ids stay untouched; the response/control parser still receives ORIGINAL data.
This is not redacted persistence or response publication, and a redacted request value
is never substituted into execution, a permission decision or a reverse alias lookup.

Tool/function/model/call/declared-agent identities and ALL JSON member names remain
original or cause a fixed refusal. Declared agent names embedded in prose are protected
too, not silently renamed as display labels. Fixed protocol keys/roles/scalars/syntax
and generation choices remain unchanged; a credential collision fails BEFORE traffic.
No incomplete identity, shortened schema field, redacted enum/default/regex, fabricated
request/usage or repaired prefix is admitted. Data-only argument values may have views,
while their original keys/types and actual runtime arguments remain authoritative.

The privacy-aware schema traversal selects only description/title/$comment annotations
at schema positions, including supported nested/map/array schema keywords; property
maps and annotation-named defaults are not mistaken for display text. Required/property/
enum/default/const/pattern/unknown constraints remain original or refuse. Response-format
schema annotations have the same checks; format/schema names and strict flags are not
aliases. This is a bounded privacy traversal, not a new general schema validator or
remote schema resolver. Unsupported/unknown identity-bearing defaults are protected.

Effective provider defaults keep established fixed-body precedence. `metadata` values
have data views; other generation/identity choices are preserved. Fixed model/messages/
stream/stream-options cannot be overridden by caller defaults. Default headers and
use_responses_api are not body fields. A config tools array is not an extra source grant:
actual ModelRequest tools win, and a default tools array when no actual tools are present
refuses instead of introducing unregistered functions into a compaction/model request.
Original API token/default header literals and route remain separate transport authority.

### Whole bounds, exact bytes and per-client binding

Request construction shares a 1 MiB original/view string-work budget, 100,000-node and
64-depth limits. Source snapshots are admitted before immutable copies; dynamic fields
are bounded before transformation. Complete historical argument JSON is checked before
its string escaping; the complete final HTTP JSON is checked again for encoded/scalar/
replacement/cross-encoding collisions. Bounded memory-only serialization retains at
most 1 MiB of final safe bytes. No raw serialized argument/body buffer, truncation notice,
partial JSON repair, spill, Debug/Serialize source or disk/network output writer escapes.
A resource/UTF-8/identity/schema/escaping conflict is fixed failure before admission.

A Checked request is opaque and bound to its originating client-source Arc, not merely
an equal permission policy. The headless LlmClient constructor accepts only the actual
builder scope; its legacy pre-encoded message API cannot bypass this path, and an ordinary
client cannot send a scoped request. HTTP sends use the EXACT checked bytes, not a later
unverified re-encoding; content type remains application/json. Every initiated attempt/
retry rechecks the SAME source/latch before send (and after the safe byte copy), and
response return rechecks it too. No already-stopped response can dispatch new tools even
without relying solely on an outer cancellation select or parsing diagnostic strings.

Only known fixed refusal diagnostics escape the private provider; all delta/status
callbacks stay silent until a response/transport publication adapter exists. Original
successful usage accounting is preserved; a pre-request refusal does not invent usage,
a remote receipt, retry or completion. #97 and all-role admission/unknown-billing ledger
semantics remain unresolved; this source bound is NOT token-budget admission or a meter.

### Preserved legacy transport and unfinished response boundaries

The existing LlmClient transport/retry/SSE completion block was extracted unchanged into
a shared internal routine; legacy clients still use the original JSON builder/serializer.
Private clients only select the checked-body branch plus mandatory source/gate guards.
Old transport/SSE fixtures, deadlines, retries, UTF-8/control semantics and ordinary
frontends/goldens stay unchanged. This is NOT new DNS/public-IP/OS containment, provider
retry/typed-error policy, immediate in-flight cancellation or rollback of a sent request.

At #110, raw response events/argument fragments, the 4 MB SSE event buffer, original
completion/tail parsing and response-text/400-scalar error handling were private/unselected.
Section 25 now selects only scoped HTTP/error body diagnostics before those diagnostic
cuts; ordinary error handling and successful SSE/control remain original. No complete
response/state redaction, typed public receipt/usage or transport-admission claim follows. Original model responses/assistant state, registry/progress/inbox and unknown
middleware outcomes still need their own source/persistence/publication boundaries.
Their copied outgoing model-request fields are now selected after middleware transforms,
not their original storage/control values or every intermediate callback.

### Evidence and remaining gates

Actual builder/client-bound fixtures check full text/decoded arguments versus original
state, exact serialized bytes/original auth/headers, effective defaults, schema/identity/
agent-name/numeric/boolean/null/encoding/replacement collisions, work/depth/source bounds,
immutable config, refused API/client bypass and typed stop after actual HTTP receipt
preventing a second retry. Synthetic real main/child/direct-compaction request rounds
inspect outgoing views, original dispatch/files/response state/auth and zero admitted
traffic/usage on refusal. Prior §23 negative request assertions are explicitly updated:
raw original argument/notification state still exists, but its outgoing copies are now
checked; schema-colliding short seeds refuse before a model call instead of reaching an
unpublishable tool operation. Direct scoped producer tests retain those operation proofs.
No native/protocol/old-corpus assertion is weakened to manufacture evidence.

Local request fixtures passed 22 cases; the updated full private runtime module passed
39, the broader agent/scoped producer run passed 104, and gadget-enabled provider-filter
coverage passed 54. All 26 added tests are included in those suites. Both feature-off/
gadgets production checks passed without compiler warnings; unchanged legacy low-level
LLM fixtures passed 10. Offline accepted contracts (126), benchmark tests (9), formatting/
whitespace/lineage and six-document fences/local links/protected-source audits passed.
Official stable manifest verification again matched Rust1.99.1 (October15 distribution)
without pin/dependency/toolchain/config/grant/installed-CLI changes.

Diagnostic Clippy found no findings on new/changed lines; existing warnings are retained,
NOT a strict-lint pass or a downgrade/suppression of the §23 baseline diagnostics. The
full local feature-off workspace run was NOT green: main unit tests had 768 passed/
2 failed/2 ignored (the unchanged real-shell startup assertions), then native integration
startup stalled and the 600 s outer bound aborted the run (124). This is not a completed
full baseline or successful startup/fast-stop evidence. No legacy assertion, production
limit or OS service was weakened, and no duplicate full native/voice retries were queued.
Require independent complete final-head platform CI rather than relabel these logs.

Response/transport/typed provider/alternative intermediate-state producers, redacted
checkpoint/journal/result/events/trace, compatible gadget origin/ToolOutcome/catalog,
bounded invocation input, #97 before the shared meter, P2g/live/default-on/release remain
open. Preserve framework/SDK/WIT/root deps/lock/pin/old publishers/hosts/process limits,
normal definitions/frontends/frames/schemas/goldens/trace-v1/old fixtures and historical
baseline/target diagrams. Require ALL FOUR exact-final-head CI jobs AND EVERY mandatory
step, informational Windows included, before missing-review-only admin merge. Failed/
aborted local evidence is never reclassified as successful full verification.

### Windows CI fixture follow-up for section 24

The initial #110 head `eb7ff31` passed fmt/Linux/macOS and Windows's feature-off suite
on October 22, 2026 (UTC), but the Windows gadget-enabled main target had 878 passed /
3 failed / 3 ignored. The existing private shell/FS stop fixtures missed their REAL
20-second startup-ready signal. The six-second partial-output fixture observed all
11 original bytes on three attempts but never the ready file; bytes without readiness
are not proof of the intended running-child case. The failed job is NOT merge evidence,
and no informational-status or admin override bypasses it.

The correction is confined to the private Windows fixture commands: flush original
`Console.Out` before the signal, use `IO.File.WriteAllText` for the actual ready file,
and use `Threading.Thread.Sleep` for the same 30-second wait. These static .NET calls
remove ready/sleep cmdlet/provider discovery from the fixture. They do not replace
`ShellRunner`/`ProcessTree`, synthesize readiness or output, change Unix commands, relax
the six-second call budget/three-attempt maximum, 20-second startup bound or strict
ten-second post-readiness approval-stop assertion. All original byte/content/typed
error/timeout/later-spawn assertions and production process rules/deadlines stay intact.
Post-correction local checks reverified the 22 request-source fixtures and the dedicated
filesystem stop fixture (one passed). The focused shell module was NOT green: 12 passed /
1 failed, with its unchanged Unix partial fixture observing zero bytes in three six-second
attempts. That remains recorded native-startup failure, not Windows execution or partial-
output proof. Reference126/benchmark9, fmt/whitespace/lineage, six-document fences/ten
local links and a protected-source/assertion/timer audit passed; the official stable
manifest still matched Rust1.99.1 without installation or pin/lock/dependency changes.
Fresh exact-final-head fmt/Linux/macOS/Windows jobs AND every mandatory step must pass
before any missing-review-only admin merge; the original failures stay recorded.


## 25. Private provider HTTP/error diagnostics — October 23, 2026 (UTC)

After #110's verified four-job merge, the private client now selects
`exec::provider::diagnostics` for HTTP/error diagnostics, not successful SSE/assistant
state or a public typed provider. Main/foreground-child/direct-compaction use that
same actual-builder client branch; ordinary frontend/service-agent request, error,
retry and parser behavior stays original. No exec, callable headless gadget/catalog,
raw checkpoint/journal/result/events/trace writer or token meter is enabled.

### Actual request/send/receipt authority

A checked request now owns a fresh opaque identity in addition to its originating
client-source Arc. Source alone issues its lease, carrying the frozen captured seeds
and SAME invocation latch, never caller seeds/gates/config/env reload. Only the client
module constructs `Sent` immediately before actual awaited send and `Received` after
actual HTTP response receipt. The diagnostic collector accepts those witnesses, not
arbitrary status labels, responses, clients or fabricated attempt ordinals. An opaque
`Detail` is issued only after a real received error body's checked capture/framing;
callers cannot substitute a String or another request's view.

Source/lease/receipt ordinals are checked before capture and release. A diagnostic
cannot bind to an equal fresh request or another client, even one sharing the SAME
builder gate/policy. Error text is data, never approval or success authority. The
same latch is checked before each send/retry, during received-body waits/chunks and
on final diagnostic use; typed NeedsApproval overrides stale HTTP/privacy failure
kinds without replacing already-observed facts. Raw diagnostic words, status codes
or model error strings do not latch or authorize anything.

### Pre-cut UTF-8 views and fixed unsafe failures

The received error body is streamed through the unchanged shared publication codec,
using frozen literal byte matching, strict split-UTF-8 validation and delayed rendered
verification BEFORE its 100,000-byte view budget and the later 400-scalar diagnostic
selection. Original input/work is separately bounded at 1 MiB. The final HTTP framing/
selection join is redacted and verified again. Replacement-created literals or a
400-scalar cut through a replacement marker fail fixed, not as clipped/repaired views.
No response.text()/charset fallback/lossy decoding, full raw text buffer, raw spill,
secret Debug/Serialize or ordinary-output callback is used in this scoped error path.

Only real body EOF finishes delayed state. Read errors, forced source/view cuts,
failed validation, drop or typed stop discard held suffixes and ALL diagnostic detail.
An over-budget received chunk is refusal, never publication of an unadmitted prefix;
its full adapter-observed count is retained separately. A legitimate unmatched prefix
at complete EOF differs from a held prefix on an aborted/truncated body. Unsafe
failures produce fixed text, not an echoed body or invented successful completion.

### Original observations, not a meter or public receipt

At most two transport-attempt records retain original ordinal, actual received status,
optional adapter-observed bytes, EOF/read/connection/truncation and diagnostic-cut
facts. None means no selected body observation, not a fabricated zero; real zero-byte
EOF is represented independently. Received bytes are not socket/on-wire byte counts,
remaining content length, redacted length, model tokens or billing proof. Existing
original incomplete-SSE counters can supply fixed failure facts; that is NOT an SSE
byte/content/argument view or pre-cut control admission.

One bounded last selected diagnostic failure per client remains in memory even after the private provider
maps it to fixed framework Model("[REDACTED]"). A new checked request clears that slot. A test-only actual-byte observer separates bounded
received-body readiness from the unchanged strict two-second post-latch stop assertion,
without a raw sink, fake read or production deadline change.
It is not an ordered all-role ledger, concurrent-call history, a durable event, a
public schema or typed framework provider API. It cannot supply #97's token-admission
semantics or manufacture usage/requests/receipts. Ordinary successful usage remains
original; actual all-role failure fixtures keep real main usage and zero synthetic
usage for refused/error calls, without adding fake Session/role counters.

### Preserved behavior and explicit limits

Original checked request bytes/auth/default headers/route and provider options are
unchanged. Existing 15-second connect /120-second read ceilings, two-attempt maximum
and 250 ms retry delay remain. Valid transient HTTP failures retain the established
retry conditions; privacy/resource/latch refusal does not invent or retry a transport
success. Scoped connection/incomplete-stream diagnostics are fixed before formatting
raw library/URL errors. Ordinary error formatting, successful SSE accumulation,
ingester/finalizer, optional tail and finish_reason/[DONE] semantics stay unchanged.

No proxy/redirect/DNS/IP/OS containment, header/decoder metadata budget, in-flight
send/success-SSE cancellation or rollback claim follows from received-body latch
wakeups. Successful SSE still uses original private event/argument state, lossy event
decoding and existing parser/resource behavior. Its potentially large tool-call
indices and cumulative content/argument fragments MUST receive a separate bounded
control/view adapter before any public exec/state producer. Do not replace executed
arguments with redacted aliases or call this error slice whole-response redaction.

### Verification evidence and remaining gates

The implemented-source diagnostic module passed 25 actual-builder/client/HTTP fixtures
with gadgets both off and on. The complete private runtime module passed 42 tests,
including three new actual main/foreground-child/direct-compaction error cases; all
28 added cases are covered by those suites. The unchanged request-source22 and legacy
LLM10 fixtures passed. Both production feature checks passed without compiler warnings;
reference126, driver9, formatting/whitespace/lineage, six-document fences/ten local
links and protected-source/legacy-block audits passed. Diagnostic Clippy had no new/
changed-line findings among 65 baseline diagnostics, NOT a strict-lint pass.

One full local feature-off workspace run was NOT green: the main target had795 passed /
3 failed /2 ignored. The unchanged shell/FS startup fixtures missed real readiness,
and the existing invalid-pipe-UTF8 fixture timed out before its intended operation.
The following alias/native launch stalled; the600 s outer bound aborted (124). Those
failures are not successful startup/fast-stop/UTF8 or a completed full workspace proof.
No old assertion, production/native deadline, OS service, dependency or fixture corpus
was changed, and no duplicate full retries were queued to manufacture green evidence.

The final test-only byte-read readiness adjustment then hit a240 s native compile stall;
a separate120 s test typecheck stopped while checking existing dependencies. Neither
aborted command is a successful final-source run. Production behavior is unchanged by
that cfg(test) observation hook. Fresh ALL-FOUR exact-final-head CI and every mandatory
step remain required to verify this final candidate, Windows included. Official stable
manifest verification on October23 still matched Rust1.99.1 (October15 distribution),
without any pin/lock/dependency/install change.

Fixture suites cover actual shared-builder clients, real synthetic HTTP/chunked byte
streams, Unicode/overlap/short/marker/cut cases, source/request binding, frozen auth/
config, actual reads/EOF/truncation, genuine retries, same typed protected-write stops,
non-latching diagnostic strings and main/child/direct-compaction errors. They are not
live model-quality/default-on/release or a full local startup guarantee. Every remaining
successful-response/transport/control/alternative-state/storage/public producer,
compatible gadget origin/catalog/ToolOutcome, bounded invocation input, #97 before
meter, P2g/live/release gate remains open. Keep framework/SDK/WIT/root deps/lock/pin,
old publishers/hosts/process limits/frontends/frames/schemas/goldens/trace/corpus and
historical October16 baseline/Appendix A/target diagrams unchanged. Documentation
belongs in every partial implementation PR. ALL FOUR exact-final-head CI jobs AND
EVERY mandatory step, Windows included, precede missing-review-only admin merge.

#### Private stdin Windows correction and evidence boundary

PR128's first50cec5 revision failed both Windows test steps (12 failures each),
even though fmt/Linux/macOS passed and Windows is informational. The separate
native route now distinguishes ERROR_NO_DATA/zero-byte Pending from real EOF,
that corrective attempt requested read/mode-setting attributes without data-write
rights and validated
synchronous readable byte pipes before direct ReadFile. OS/access refusal has no
blocking-reader or reopening fallback. Two new regressions extend the inventory
to41 normal cases/one argv-only ignored helper without changing older fixtures,
source limits, clocks or contracts. The actual inherited-process-stdin cases and
ALL FOUR fresh exact-head jobs/every mandatory step must pass before merge; see
[named-stdin.md](named-stdin.md). This remains a private partial implementation,
not public stdin/frontend, R2, gadget callable/catalog or #97 approval.

#### Original-rights Windows read-only polling after the second failed attempt

The second hosted3c0f3da revision failed five real inherited-stdin cases in EACH
Windows build (1165/5/3 and1295/5/4), despite fixing the false-EOF cases. Current
same-access duplication preserves original grants; fixed actual-HANDLE metadata
selects owned NOWAIT only where supported, otherwise unchanged-mode native
availability polling. Positive readiness tightens a concrete read; terminal peek
permits a read probe, NEVER EOF/bytes/usage on its own. Open-empty pipes issue no
blind read and no background/generic reader/reopen/access upgrade is introduced.
Seven native read-only tests extend coverage to48 normal/one ignored, preserving
all earlier42 test bodies, real-stdin assertions, limits and accepted contracts.
Synchronous peek can block behind foreign IO and foreign readers can defeat
readiness: public exclusive-input/lifecycle and OS/native interruption remain
open, not weakened or claimed from these tests. [named-stdin.md](named-stdin.md)
records both failed hosted revisions and ALL FOUR fresh final-head/every-step
merge gates. Public exec/stdin/R2/callable/catalog/#97/P2g remain separate/off.

### Private native stdin + guarded destination implementation boundary

The new composed route is additive: older input/file/plain-stdin APIs and their
bodies/assertions stay unchanged, including FileInvocation's own stdin refusal.
All original source/count/path/output grammar precedes IO, SAME Bootstrap authority
captures the output before stdin, and complete input plus after-read retained-target
revalidation precedes session selection/store/runtime construction. No generic
reader/writer, post-run destination/format switch, permission, raw IO record/status/
usage source or foreign Runtime/Run pairing is introduced. Existing guarded
publication/epoch/cleanup/source bounds and Windows/namespace/shared-reader/native
IO limitations remain. Current code is private, not accepted-schema public exec or
live/default-on approval. [stdin-file-output.md](stdin-file-output.md) records new
fixture attempts and ALL FOUR final-head/every mandatory-step merge requirements.

The first composed hosted head d869ff7 (CI40147790277) failed both Windows
feature steps despite run-level success: the new raw dot/parent fixture failed
in both, and two unchanged native-shell timeout/output assertions also failed
feature-off. Only the NEW fixture now preserves original OsString spellings;
production grammar/readers/writers/limits and older tests remain exact. Failed
shell evidence is neither waived nor a reason to retry unchanged full CI.
ALL FOUR corrected exact-final-head jobs/EVERY mandatory step must pass, including
both Windows builds and original shell cases (`stdin-file-output.md`).


## 26. Private instruction argv parsing and routing — October25 (Asia/Taipei)

A new app-only source/attachment argument parser is selected by separate private
`ArgumentInvocation` / `ArgumentFileInvocation` factories. No CLI subcommand,
help/error printer, TTY/CI/env-dependent default, public result schema or new
accepted flag is added. Native OsString arguments are preflighted before strict
UTF8 inspection/copies:260 entries,1048592 encoded bytes/token and3146816 total
encoded bytes (derived from old instruction/path/file-count ceilings). Actual
source admission still uses original1MiB instructions,128 selected paths and
4096-scalar/16KiB labels; input caps are NOT raised by argv overhead bounds.

Subset: one positional instruction/exact `-` OR one split/joined `--prompt-file`,
plus repeated one-path split/joined `--file`; `--` ends option parsing. Original
spaces, Unicode, quotes, equals/backslashes and source labels are data, not shell
syntax or permission aliases. Option-looking split file values require a joined
spelling; a `--prompt-file -` filename is NOT stdin. Exact positional `-` remains
stdin even after `--`; spaced minus stays literal. Unknown/abbreviated/legacy/full
scope/permission/output/session/usage/trace/gadget/wall flags fail fixed before
config/stdin/session/provider effects, never accept-and-ignore an unsupported
feature. Options/Destination remain explicit typed pre-run parameters.

The unchanged complete input grammar runs before any effect, and only the
original four concrete factories capture source/authority through SAME Bootstrap/
captured credentials/ConfinedFs/shared runtime. Both old memory invocations return
the EXISTING Run; both old file invocations return EXISTING FileRun/FileTerminal/
Record/publisher. No arbitrary Runtime/Run/target/data/seeds/gates constructor,
new generic reader/writer/selector, raw-state/error producer, output-format alias,
post-run path/format switch, public status/exit/usage/receipt or permission is added.
For native/file input, the old target-before-stdin and after-read retained-target
check remain; regular input/file ordering remains its old construction ordering.
Old Bodies/assertions/readers/limits/scopes/epochs and all stdio/shared-reader/
namespace/Windows ACL/native-call/cancellation limitations remain unchanged.

The first production check's NEW private-interface warning was fixed only by
matching the safe SelectionError visibility. Initial57 normal cases passed with
one unused NEW fixture import warning; new bootstrap coverage now uses it. The
NEW native helper was strengthened to retain BOTH original actual literal and
stdin provider requests, with no history/mock reset or fabricated zero baseline.
Current58 normal cases plus one argv-only ignored helper test exact syntax,
resource/UTF8 bounds, original data/BOM/labels, actual old runtime/file chains,
typed partial approval/provider failure, no early store/output/provider effects
and REAL inherited stdin left unread by rejected/unnamed sources. Unmarked helper
returns before any config/stdin inspection; only isolated config/pipes/synthetic
providers are used. [input-arguments.md](input-arguments.md) records verification.

Public exec/full flag integration/console/file stdin/stdout/stderr/R2/results/
events/redacted storage/trace/compatible provenance/headless gadget callables/
catalog/shared usage/#97/P2g/live/release remain independent/open. No framework/
SDK/WIT/dependency/lock/pin/frontend/host/frame/schema/golden/corpus/trace change.
Docs accompany this incomplete slice; ALL FOUR fresh exact-final-head jobs AND
EVERY mandatory step, BOTH builds and Windows included, precede any missing-
review-only admin merge. #129's native composition CI remains historical evidence,
not current-source/whole-frontend approval.


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


## Token-admission clarification — accepted November1 (Asia/Taipei)

The new [token-admission.md](token-admission.md) review artifacts address #97's
truthful30-used/100-budget refusal before an81-token reservation. Optional
`_meta.craftsman.limit.admission` holds estimated input/requested reply/remaining,
without changing measured role usage, logical call counts, existing outcomes,
units or standard ACP stops. Exhausted/overrun observations keep their old mode;
pre-exhaustion requires coherent context and STRICT insufficient remaining room.

Original25 fixtures/126 reference entries retain values/serialized text;70 new
record/stream/wire cases and eight independent native counter/ledger tests make196
reference cases/11 native tests. Integer/role/event/partial-output constraints are
not weakened. First NEW fixture null-output/missing-run-start failures are preserved;
corrections are NEW data only, not loosened old requirements/assertions/limits.

The user explicitly accepted this narrow contract and compatibility judgment
November1, replying "yes approve" to the specific field/strict reservation/v1/
older-validator/collision proposal. Legacy semantic validators still need an update;
recognizing the optional field can still collide with old unspecified metadata.
Neither risk is erased by acceptance. R1–R7's October20 acceptance is unchanged;
October31's general continue/CI/admin evidence is not retroactive acceptance.
Runtime/native readers/publishers/guest/SDK/WIT/framework/deps/lock/pin/frontends/
SafeBackend/ACP/v4/trace/goldens/corpus remain unchanged. #97's contract question
is resolved; exact-head all-four/every-step/both-mode/Windows verification gates
accepted-artifact merge. Shared meter/public producer/live/release stay unwired/off.


Historical pre-acceptance source-local evidence: all38 valid bounded rows passed warning-free,
including native11 in BOTH modes, reference196, all protected scope/control/source/
native/composition/runtime/guest suites, both locked production checks, existing
marked/unmarked compile probes, source/CLI/compatibility/Markdown/fmt/lineage checks.
The exact12-path audit preserves runtime/framework/SDK/WIT/deps/lock/pin/frontends/
protocols/goldens/trace/corpus and original25/126 serialized entries;62 local links
and balanced fences pass. Initial NEW fixture errors, a NEW comment-wording lineage
failure, and an unsupported EXTRA trace-driver switch are preserved, not called
passing evidence. Only new fixture/comment/driver setup was corrected; no old
check/allow-list/trace/source/limit was relaxed and no full suite/CI retry was used.
These are historical pre-acceptance source-local results, not fresh tests of the
November1 documentation-only acceptance update. Runtime/test/schema/checker/vector
bytes stay exact and fresh exact-head CI/document/source proof gates this update.
The explicit contract decision above is separate; shared meter/public producer/
live/release remain unwired/off.


## Private shared logical-request admission — November1 (Asia/Taipei)

[request-admission.md](request-admission.md) records the next bounded R5 slice after
#133's accepted-contract merge/#97 decision closure. One actual-builder source and
opaque ORIGINAL-provider/concrete-role bindings enforce1..10000/default50 calls
across main/subagent/compaction before supplied-inner entry. Same-gate cap refusal
closes execution deny-only, never fake NeedsApproval; true approval dominates the
private runtime before/after classification. Active/actual inner-return/drop facts
never refund admitted calls, and foreign/equal sources and poisoned state refuse.

Forty additive tests (27 owned-builder synthetic +13 actual input/loopback/runtime)
cover all roles, concurrent cap/last-response boundaries, failures/drop/refusal,
source identity, real approval and later same-response tool denial. Original request
histories and partial state/registry stay private, not reset or promoted into usage.
This is NOT token allowance/clamping/usage classification/HTTP receipt/unknown billing,
public flags/results/events/trace/storage/provenance/callables/catalog/P2g/live/release.
An admitted wrapper can fail before traffic; public token admission must combine
both reservations BEFORE the charge point, not refund/hide a refused call later.

Docs/source/tests accompany the incomplete implementation. Old tests/assertions/
readers/writers/native/guest/host limits, SDK/WIT/framework/deps/lock/pin/frontends/
SafeBackend/schemas/frames/goldens/trace/corpus remain unchanged. Initial new adapter
warning/scope/privacy/input-constructor setup failures are retained; fixes do not
suppress warnings, broaden production visibility or weaken old checks/deadlines.
ALL FOUR fresh exact-head jobs/EVERY mandatory step/BOTH modes/Windows and source/
warning/review proof gate merge, not the older #133 contract CI. No independently
blocked provider correction is inspected, executed, changed, retried or worked around.


## Private atomic token reservations — November2 (Asia/Taipei)

[token-reservations.md](token-reservations.md) records a private R5 opt-in foundation.
The SAME builder binds concrete scoped clients/roles to one meter; bounded whole
checked-input estimates and final reply-body checks precede ONE request/token
allocation decision. Default request-only paths remain unchanged; only a separate
typed private factory opts in, with no public flags/CLI. Allocations stay active or
retained unsettled, never inferred measured usage, refunded calls or #97 public
observed/remaining facts. Fifty additive tests cover27 actual-builder/source/client/
concurrency cases,12 real input/runtime/child/compaction cases and11 arithmetic/
estimator cases. Real histories stay intact; initial NEW `--lib` driver failure is
preserved, corrected only by selecting the binary test target.

Receipt/partial-output witnesses, usage-presence/fallback, concurrent settlement/
public ledger and all other producer/P2g/gadget/live/release gates remain separate.
All old assertions, source codecs/response control/native limits/framework/SDK/WIT/
deps/lock/pin/frontends/protocols/schemas/goldens/trace/history remain protected;
additive tests/docs and ALL FOUR fresh exact-head CI jobs/every mandatory step/both
modes/Windows plus source/warning/review proof precede any missing-review-only merge.


### CI image correction — November2 (Asia/Taipei)

The first #135 head's macOS job was cancelled in four seconds BEFORE runner
assignment/checkout/any test. GitHub's official runner-images#13518 and check
annotation identify the macOS14 retirement brownout (November1 14:00–November2
00:00 UTC); full retirement is November2, 2026. That cancellation is preserved,
not a source-test failure or green evidence, and is DISTINCT from the unchanged
local partial-shell readiness failure above.

`ci.yml` now separates stable check ids from actual images: required
`test (macos-14)` retains its exact context name but runs on explicit `macos-15`.
Ubuntu24.04/Windows2025 names/images and EVERY action/test/build/limit/condition
remain unchanged; no ruleset/branch protection mutation, macos-latest alias,
old assertion relaxation or unchanged full-CI retry. Only real image selection
and current docs change; every Rust byte from the50-case implementation/local
rows is identical. Fresh corrected-head ALL FOUR jobs/every mandatory/setup step/
both modes/Windows plus ACTUAL macOS15 labels/raw logs/source/warning/review proof
are required. Do not call the legacy name macOS14 test coverage or reuse the
cancelled original head's status as coverage of the corrected workflow.


## Private admission-correlated send/header observations — November2 (Asia/Taipei)

[admission-receipts.md](admission-receipts.md) records the next private R5 slice after
#135. Only actual token-admitted body/permit/checked-client identity mints the scope;
SAME builder/meter/gate and concrete main/subagent/compaction keys bind bounded
<=10000 calls/two slots. Client-only witnesses record ACTUAL first send-future poll,
awaited send error or returned HTTP headers; preparation/no-poll, abandonment and
late stop retain truthful distinct facts. No socket/wire/billing/body completion/
usage/settlement inference, extra retry request charge, fake status/bytes or refund.
Default request-only/frontends/flags remain unchanged; only actual Run selects
non-Debug/non-Serialize historical source-bound observations. Forty additive tests
(10 builder/identity,20 actual runtime/loopback,10 coherence/bounds) and docs accompany
the incomplete adapter. Initial5 NEW unused test interfaces are preserved/corrected
only by completing new actual-owner coverage; no suppression/old assertions changed.

Original successful SSE/control/usage/retry/diagnostics, source codecs, native/guest/
input/writer/host limits, framework/SDK/WIT/deps/lock/pin/frontends/SafeBackend/v4/ACP/
schemas/vectors/goldens/trace/history stay unchanged. No independently blocked provider
inspection/execution/workaround. Actual bounded successful response/usage presence/
partial-output, billing/settlement/overrun/public producers/provenance/P2g remain
separate; exec/callable headless gadgets/catalog/live/default-on/release stay OFF.
ALL FOUR fresh exact-head CI jobs/every mandatory/setup step/both modes/Windows plus
actual macOS15 label/source-warning-tree-review proof precede missing-review-only merge.


### Required engine security patch — November2 (Asia/Taipei)

During this slice GitHub surfaced two critical manifest/lock alerts for the SAME
Wasmtime AArch64/large-br_table advisory GHSA-mghf-wjfp-p5xm. The advisory/48.0.7 fix
were published October19, 2026; our repository alerts appeared November1 at20:06UTC.
This is newly surfaced evidence, not a new November2 advisory or proof that earlier
48.0.6 was safe. Vendor48.0.8 (October29) additionally fixes component/WASI issues;
both direct host pins now use exact48.0.8. Rust stays1.99.1, SDK/WIT/guest-host policy/
capabilities/production limits/defaults/assertions and CI/check names remain unchanged.

A separate necessary dependency commit updates ONLY the two host pins and30 existing
Wasmtime/Pulley/Cranelift version/checksum entries; no dependency edges/features/
registry sources, unrelated libraries, standalone gadget SDK sources/lockfiles or
Rust toolchain changes are bundled.
Three additive offline guards pin the reviewed matching runtime/WASI versions/family
and original feature/model-opt-in shape; no exploit or unpatched guest is executed
by these guards. The local machine is nativeARM64. Fresh patched-head locked checks,
all43 NEW receipt/security tests, protected native/guest/SDK/WIT suites and ALL FOUR
CI jobs/every mandatory/setup step/both modes/Windows/actual macOS15 evidence gate merge.

All15 earlier bounded local rows passed warning-free BEFORE this prerequisite; they
are retained as pre-security evidence, NOT relabeled patched-head coverage. No whole
unchanged retry/empty commit/assertion/deadline weakening; only the real dependency
security update justifies fresh source validation. Other open advisories, public
exec/usage settlement, full containment/P2g/live/default-on/release remain separate.


## Maintenance: existing Windows shell partial-output fixture (November 3, 2026)

[windows-shell-fixtures.md](windows-shell-fixtures.md) documents an independent
existing-shell failure: the November2 post-merge main Windows feature-off step
failed the old 2s partial-output assertion, even though the PR and later
Windows gadget-host step passed. Informational run success is not step success.
A Windows-only test-command correction flushes ORIGINAL Console stdout/stderr
before a direct long sleep; three additive native cases check both real partial
byte/Timeout observations and deliberate empty-output timeouts in both formats.

Keep the old assertions, 2s call/30s outer/2s drain, all production shell/process
sources and the separate private6s real-ready-marker tests unchanged. No warmup,
retry, fake output/exit, new authority or public frontend follows. All FOUR fresh
exact-head jobs/every mandatory/setup step/both modes/Windows remain required for
review-only merge. Acceptance of R1–R7/#97 is unchanged; response/usage/settlement,
public producers/redacted persistence/trace, gadget provenance/registration and
P2g/live/release remain separate, including the independently blocked provider
correction. This maintenance does not inspect or work around that correction.


## Test-only scoped shell observations — November 3, 2026

[shell-test-observations.md](shell-test-observations.md) adds bounded fixed
launch/read observations only to cfg(test) actual private Shell owners. Original
filesystem/shell fixtures can report their own recorder on assertion panic,
without source text/global hooks/env/cwd/writers or clock/process changes. PID
and scope release are not execution, completion, usage or OS quiescence facts.
Keep all original20s/15s/10s assertions and source capture/UTF-8/pump semantics.
#138's failed local full suites are preserved, with root cause still unproven;
new instrumented/focused passes are not replacement coverage. Actual all4-job/
every-step/both-mode/native-platform evidence remains the gate. R1–R7/#97 stay
accepted; public exec/usage/settlement/producer/trace/provenance/P2g/live/release
and independently blocked provider work remain separate/off.


### Native failure snapshot refinement for #138

The first instrumented full runs still failed:1554/2/7 off and1697/2/9 gadgets.
Actual launch return/first read polls were under3ms with zero received bytes;
this is not a root-cause fix. The live children were gone when those logs were
inspected, so no post-hoc lookup of possibly reused PIDs is performed.
A cfg(test)/macOS-only panic-teardown query now records fixed BSD image class,
state and flags ONLY for an active actual PID after exact struct/PID/current
parent/group validation. It retains no raw process name/path/argv/environment,
starts no sampling thread and sends no signal. The bounded native-call count is
not an absolute OS-latency/PID-reuse/exec/quiescence guarantee. New source evidence
and all4-job/every-step/both-mode gates remain separate from old failed runs;
all original assertions/deadlines and production behavior are unchanged.


### Native task/thread failure refinement for #138 (November 3)

PR139 merged only the diagnostic slice after exact-head all-four-job/every-step/
both-mode/source/warning/tree audits; #138 is still OPEN. Its refined local full
off run still failed1554/2/7, independently of hosted green evidence. No root
cause or exec/usage/settlement/public-output/P2g/live/release approval follows.
The next cfg(test)/macOS-only failure snapshot adds original kernel task CPU/
thread/running/context-switch/syscall/fault counters and bounded per-thread
run-state/native-time facts. It uses one complete TASKALLINFO snapshot, at most8
native thread entries and a final original PID/start/current-parent/group
revalidation; incomplete, overbound, duplicate or changed subqueries are fixed
unavailable/refusal diagnostics, never a clipped complete set. At most11 native
calls per active entry, with no retry/sampler/signal/sleep/native IO under the
recorder lock or raw name/pthread-address/path/argv/environment rendering.
Native counters are not wall-time/rate/CPU-spin/wait-channel/exec/progress/usage
proof; calls are bounded, not absolutely interruptible OS-latency guarantees.
All old runnable code/reader/process semantics/assertions/deadlines stay exact.
New native metadata and actual-builder cases are separate from full evidence;
full validation of genuinely changed source is not an unchanged broad retry.
See [shell-test-observations.md](shell-test-observations.md) for exact limits and
remaining diagnosis; every partial slice keeps current documentation in sync.


### Native executable/CWD observation refinement for #138 (November 3)

PR140's independently audited merged-main run41631110649 passed all FOUR jobs/
every setup/mandatory/post step/BOTH modes; its failed full local1562/1/7 off
and1704/2/9 gadgets remain failed and #138 is OPEN. The next cfg(test)/macOS-only
snapshot distinguishes a process display-name class from the actual native
executable path class and compares native CWD device/inode to the SAME retained
ConfinedFs workspace captured from the actual ShellScope. Only exact literal
image classes and fixed match/different/unavailable relations are retained or
rendered, never paths, names, argv/environment or device/inode values. A native
path has one initialized4096-byte buffer/exact strlen+NUL validation; CWD uses a
complete exact native vnode struct, ignores embedded paths and checks directory
mode/nonzero identity. No alias normalization, path reopen, metadata as authority
or root rebinding to manufacture a match. Final ORIGINAL PID/start/parent/group
revalidation gates ALL new execution facts and existing thread facts; reads are
not atomic/exec/containment/wait-cause/usage proofs. Maximum13 native queries per
active entry, plus one clone/metadata/close of the retained directory at TEST-ONLY
Shell construction; no sampler/signal/sleep/retry/clock/launcher/default change.
Seven macOS additions include real same-workspace and OWNED namespace-rebind
cases; every older body/assertion/deadline/production source stays exact. Earlier
failed full evidence is never replaced by focused/hosted passes or an unchanged
broad retry. See [shell-test-observations.md](shell-test-observations.md); docs and
all-four exact-head job/step/mode/source/warning/tree gates accompany this slice.


### Bounded pre-kill native snapshot refinement for #138 (November 3 UTC)

PR141 merged-main41647609134 independently passed all FOUR jobs/every setup/
mandatory/post step/BOTH modes with exact source/tree/runner/case/warning proof.
Its FAILED local1566/4/7 off and1709/4/9 gadget results, INCLUDING new integration
readiness failures, remain failed and #138 OPEN. A new cfgtest/macOS-only hook
records the actual original stop decision/current owned tree ID/observed pipe
facts BEFORE ORIGINAL kill/reap, so later invalid-byte Timeout assertions can
report saved fixed native data without querying old/reused PIDs. One non-clone
query ticket per retained call is shared with active failure views; reserve under
lock, query outside, cap4096 safe ASCII bytes/13 native calls, never clip/echo raw
text or refund/rebind an evicted ticket. Prior active data stays labelled active-
view, not fresh before-stop; clocks explicitly distinguish active-query vs call.
No stop/pid mismatch/released call authorizes a saved-PID query. Existing report/
failure_report remain path-free and hide released native facts; ONLY original
fixture-panic teardown/new explicit TEST view includes the bounded saved history.
No sampler/signal/sleep/retry/timer/launcher/production/default/old assertion/
deadline/reader/process change. Twelve macOS additions (eight pure/four actual
builder) test one-shot identity/caps/eviction/before-drain facts and real timeout/
approval/prior-view/normal-return behavior, NOT root cause/exec/quiescence/usage
or replacement full coverage. See [shell-test-observations.md](shell-test-observations.md);
docs plus all-four exact-head job/step/mode/source/warning/tree gates accompany
this incomplete slice. Public/blocked-provider/usage/live/release gates stay off.


## Windows Job/PID fixture follow-up — November4,2026 Asia/Taipei

PR142's separately audited merged-main41668514031 is NOT all-four green:
Windows feature-off1532/2/7 failed the ORIGINAL2s partial-output and3s Job/PID
cases; gadgets1666/0/9 passed, but the interpreter post step was skipped.
All other jobs/inventories and original Windows9 warning signatures/17 occurrences
remain verified. Preserve that failure separately from green PR142 CI, never
substitute the later gadget pass or retry unchanged CI to manufacture green.

Only the old Windows Job/PID fixture input now removes Start-Process/Write-Output:
construct one actual System.Diagnostics.Process, keep UseShellExecute false and
child streams redirected to that Process, Start without implicit Boolean output,
flush its ORIGINAL positive PID through Console, then actual WaitForExit. The
SAME existing PowerShell/ProcessTree/Job Object path and original3s timeout/30s
outer/2s drain/PID/grandchild assertions stay exact. Child creation/output is not
mocked, no startup readiness/bytes/PID/completion is fabricated. Two additive
native Windows cases check exact input order/no cmdlets and actual PID/raw bytes/
typed Timeout/no exit/real duration/grandchild kill under the SAME3s bound.

The ordinary2s PRINT_THEN_HANG/direct Console fixture and ALL its older/new test
bodies remain byte-exact. Its missing-output cause is NOT established or fixed
by this narrower PID-input implementation; no universal cold-start latency is
promised. Production runner/capture/process/defaults/clocks/timers, framework/
SDK/WIT/provider/input/guest/permissions/publishers/frontends/deps/lock/pin/
workflow/frames/schemas/goldens/trace/corpus stay exact. Current implementation
is incomplete pending native Windows verification; local macOS checks with zero
selected Windows cases are NOT Windows execution evidence. Documentation must
accompany every partial slice; historical plan/Appendix A/R1–R7/#97 stay intact.
ALL FOUR fresh exact-head jobs/every setup/mandatory/post step/both modes/actual
macOS15+Windows/old+2-new-inventory/warning/source/tree proof precede review-only
merge. No warmup/unchanged retry/deadline/assertion/launcher workaround, real model
call/user config/env/SDK mutation or blocked-provider work. #138 stays OPEN;
public exec/usage/settlement/callables/P2g/live/default-on/release remain off.


## Read-only native scheduling facts — November4,2026 UTC

PR143/main41683501397 is independently all-four/every-step/both-mode/source/
old+new-inventory/warning/tree verified. Earlier Windows main41668514031 remains
FAILED; prior local macOS1578/4/7 off and1721/4/9 gadgets remain FAILED/#138 OPEN.
Retained pre-kill flags20987920 are not, alone, a wait-cause or startup witness.
A conservative PUBLIC flag projection and original nice/task/thread priority/
policy fields are now copied from ALREADY complete native query buffers.

This is cfgtest/macOS-only, ZERO additional native queries/handles/root reads:
unchanged13-query/8-thread/call/view bounds and original final PID/start/current-
parent/group revalidation gate ALL new process/thread scheduler details. Process
facts are labelled initial-task-info, not atomic last state. Resource POLICY is
one masked enum (0x600=terminate, never two policies); unknown ACTION combinations
and all non-public/unclassified bits remain numeric, never guessed private flag
names or rewritten as none. EXEC/TRACED/PPWAIT/etc are exported bit predicates,
not current executable/script entry/tracer/whole-process wait/permission/usage
proof. Native signed priorities/policies are preserved, not normalized into
high/low/starved/suppressed or treated as a portable scheduling explanation.

The SAME one non-clone active/pre-kill ticket, original phase/clock/pipe facts,
released-native hiding and cached teardown semantics remain intact. Whole fixed
ASCII/LF reports still fit4096 bytes or WHOLE fixed refusal, never clipping/raising
caps to rescue extra fields. No sampler/signal/sleep/poll/clock/timer/launcher/
production/capture/process/default/permission/config/storage/frame change. Eight
additive pure cases plus one real actual-builder1s zero-byte timeout (NOT startup/
partial proof) protect projection, masked enum/unknown bits/native signs/no raw
name/address, final revalidation and saved history without another PID lookup.
All old15s invalid-byte/20s readiness/10s stop/6s partial/Windows2s+3s/30s outer/
2s drain/source/test/assertion bodies remain exact. Docs with incomplete work;
historical plan/Appendix A/R1–R7/#97 and SDK/WIT/framework/deps/lock/pin/workflow/
frontends/schemas/goldens/trace/corpus stay exact. Native and ALL FOUR exact-head
job/every-step/both-mode/actual macOS15+Windows/old+9-new-inventory/warning/source/
tree verification remains required before missing-formal-review-only merge.
No unchanged broad retry/warmup, fake PID/bytes/readiness/usage, stale PID lookup,
real model/gadget/user config/env/SDK mutation, agents/goals/blocked-provider work
or public exec/usage/settlement/callables/P2g/live/default-on/release enablement.


## Test-only original shell poll/selection follow-up — November 4, 2026 UTC

The cfg(test)-only follow-up in [shell-poll-progress.md](shell-poll-progress.md)
records actual reader polls and the existing reader/wait select branches through
the SAME installed Call. Original Context/waker/buffer/result are delegated
first; counts/time/last fixed classification are metadata, not readiness,
wait-cause/inner-wait polling/OS quiescence or usage. Before-stop history is copied
before original kill/drain, with no new native query/ticket or active-query clock
rebinding. Whole4096-byte fixed ASCII/LF section or fixed refusal; no clipping.

All old R1–R7/#97 semantics, tests, native/reader/process/deadline limits and
framework/SDK/WIT/deps/lock/pin/frontends/schemas/goldens/trace/corpus remain exact.
All13 new focused macOS cases pass both modes warning-free. ONE full run per
changed mode then yields FAILED1600/4/7 off (including one NEW normal-output case)
and1747/0/9 gadgets passed, with every old inventory present. These are separate
evidence, not a root fix or replaced failures. Exact-head hosted native validation
is in progress; prior failed full evidence and OPEN #138 are preserved. Public exec/usage/settlement/producers/
callables/P2g/live/default-on/release and blocked-provider work stay separate/off.


## Corrective environment-test isolation — November 4, 2026 UTC

[test-environment-isolation.md](test-environment-isolation.md) follows verified
PR146 with a correction to unsafe global environment setup in parallel tests.
The four original config/MCP/route assertion bodies move unchanged (minus only
unsafe setup/cleanup) into actual marker-gated immutable child environments.
The existing concurrent serve protocol/auth exchange retains every assertion,
using a per-router cfgtest source and the same real auth comparison instead of
mutating global environment. Production env/auth/shell behavior and all old
native/IO/SDK/framework/clock/permission/schema/frame/golden/trace/R1–R7/#97
boundaries stay unchanged; no real model/gadget/user-env/SDK effects. New
validation is in progress and cannot substitute for old/new FAILED evidence or
prove #138's cause. Public exec/usage/settlement/callables/catalog/P2g/live/
default-on/release and blocked-provider work stay separate/off.


Corrective full validation (November 4 UTC): all five original environment/
protocol cases and eight new substantive cases pass both modes, warning-free.
Full off still FAILED1605/4/8 with the same four shell/readiness failures;
gadgets1752/0/10 passes. Unsafe environment mutation is removed, but it is NOT
proved #138's sole cause/fix; no old assertion/deadline/process/reader/SDK or
provider workaround. All prior failed evidence is retained and #138 OPEN;
all-four/every-step/both-mode hosted verification remains required.


Windows PR147 first-head41829354220 is preserved FAILED in BOTH modes only for
the NEW offline declaration guard's LF-only source match (native main1552/0/8
and1684/0/10 and every old/env assertion pass; interpreter post step skipped).
Correction normalizes ONLY CRLF pairs to LF in that NEW guard; original exact
cfg/declaration checks remain, with an additive LF/CRLF-versus-lone-CR/wrong-cfg/
wrong-name/tab negative case. No production/old assertion/deadline/pin/SDK/
workflow/warning suppression or unchanged CI retry. Require ALL FOUR fresh
corrected-head jobs/every step/both modes/native/inventory/warning/source/tree
proof before review-only merge; #138 and old local failures remain unresolved.


## PR147 corrected-head Windows failure and test-only phases — November 4 UTC

Corrected head3919f640/CI41835883954 attempt1 passes ALL four new CRLF/environment
source guards and ALL five original environment/protocol cases BOTH modes on
Ubuntu24.04/actual macOS15/Windows2025. It is still NOT all-four green: Windows
feature-off Test fails the ORIGINAL2s partial-output stdout assertion with empty
wording (1551/1/8); later gadgets1684/0/10 does not replace it and interpreter post is
skipped. All other jobs/steps/old+new inventories and Windows9 warning signatures/
17 occurrences remain verified. PR147 remains DRAFT/unmerged; #138 stays OPEN.
No startup/reader/wake/capture cause follows from the missing output.

The next [test-only legacy phase observer](legacy-shell-observations.md) records original spawn return,
exact delegated read polls and existing reader/wait/kill/drain selections, owned
by the actual ordinary test ShellRunner. At most8 calls/WHOLE4096 ASCII-LF report
or fixed refusal; original clock/counters and pre-stop/current histories remain
separate. No extra poll/waker/native lookup/raw bytes/error/env/clock reset/global
hook/launcher/warmup/retry. Only original assertion-panic teardown prints it;
headless retains its SAME existing recorder/native bounds. Shared executor
extraction and cfgtest-only seams keep original pump/process/capture/defaults/
old commands/assertions/Windows2s+3s/30s outer/2s drain/private15s+20s+10s+6s and
env20s+5s limits intact. Eight additive cases plus a CRLF-safe source guard are
not a root fix/Windows proof. Docs with incomplete work; all FOUR fresh exact-head
jobs/EVERY step/BOTH modes/native/source/tree/inventory/warning/review proof still
gate missing-review-only merge. No unchanged broad retry/admin failure bypass,
real model/user config/SDK/global-env mutation, agents/goals/blocked-provider work
or public exec/usage/settlement/callables/P2g/live/default-on/release enablement.


One genuinely changed-source full command per mode at c1cac545 (November4 UTC)
completed warning-free with every old native name/ignored/doctest plus the eight
new main cases/new guard/CRLF guard preserved. All original environment/auth and
all18 new substantive cases relative to PR146 pass BOTH modes. Off remains
FAILED1613/4/8, exit101,998.48s whole command: the SAME four scoped-shell cases
still fail. Gadgets1760/0/10 passes, exit0,73.85s; never substitute that pass.
Temporary lack of startup output from an offline test binary later cleared; its
original source assertions pass. No root-cause/PID/startup-latency inference,
clock/assertion/launcher workaround, unbounded wait or unchanged broad retry.
Full evidence stays retained; #138 OPEN and PR147 draft pending ALL FOUR fresh
exact-head jobs/EVERY step/BOTH native modes/source/tree/inventory/warning/review
proof. Public/usage/settlement/callable/P2g/live/default-on/release remain off.


## Native current-child pipe snapshots — November 5 Asia/Taipei / November 4 UTC

PR147/main86704e4 are independently ALL FOUR/every-step/both-mode/source/tree/
inventory/old Windows9-signature17-occurrence verified; live main/p0-integration
branches were FF-only/data-preserving. The transient null Windows main-job
metadata was strictly refused, then SAME-run/job metadata converged before
verification/FF; no CI retry/faked completion. Local off1613/4/8 remains FAILED,
gadgets1760/0/10 passes, #138 OPEN and all older failed evidence sealed/read-only.

The next [read-only pipe facet](native-pipe-snapshots.md) queries ONLY the existing live actual
native snapshot ticket's CURRENT child fd1/2 metadata; no read/dup/poll/waker/
launcher/environment/clock/signal/sampler/public output change. Complete exact
public ABI/write-pipe/FIFO/signed-count checks and SAME final PID/start/parent/
group validation gate all facts. Buffer counts are not original pipeline
identity/read/emission/EOF/usage or cause proof; child fd rebinding is explicit.
SAME13-query/8-thread/call/view/4096-byte whole-report bounds:5 fixed + whole-list
reserved threads; two pipe queries only if<=6 fit,7/8 preserves old threads and
refuses new facet, no refund/retry/clip. Old phase/ticket/clock/limits/assertions/
production/SDK/WIT/deps/lock/pin/frontends/frames/schemas/goldens/trace stay exact.
Additive ABI/pure/real-unread-pipe/actual-builder-zero-timeout tests and docs are
not a root fix. Require fresh all FOUR exact-head jobs/EVERY step/both modes/
native/source/tree/inventory/warning/review proof before review-only merge and
separate main verification before safe live FF. No warmed/unchanged broad retry,
real model/user config/SDK/env mutation, agents/goals/blocked-provider work or
public exec/usage/settlement/callables/P2g/live/default-on/release enablement.


Changed-source full validation completed November5 Asia/Taipei/November4 UTC:
off remains **FAILED1622/4/8**, exit101,427.17s whole command; gadgets1769/0/10
passes, exit0,77.20s. All nine additive cases and original environment/auth cases
pass BOTH modes; every old named/ignored/doctest inventory is retained, no warnings.
The SAME four failures each have zero buffered bytes in CURRENT child fd1/2 at
their original native query, with one original Pending poll/no wait-selection
return. That is not original pipeline/prior emission/startup/wait-cause proof or
a root fix. The offline guard's temporary absent startup output later cleared
and its original assertion passes. No unchanged broad retry; #138 stays OPEN.
See [the complete pipe diagnostic validation](native-pipe-snapshots.md).


## Explicit owned-child native stack experiment — November 5, 2026

The user approved ONE bounded native stack capture of a newly owned failing
child, expanding no-sampling only for an explicitly marked cfgtest/macOS
experiment. PR148/main151d4bf are all-four/every-step/both-mode verified; local
off1622/4/8 remains FAILED/#138 OPEN. The new private seam borrows the SAME actual
pre-kill ProcessTree/Call, selects only a fixed libtest diagnostic case plus
explicit owned directory marker, never old/reused/caller PID or runtime role
authority. Normal CI/unmarked/production paths do not sample. One consumed budget,
fixed1s/1000ms tool request,5s execution/2s owned cleanup/8KiB pipes/1MiB trace
and sampler-only file limit; actual return/signal/identity/hash facts, no retry.
Sampling SUSPENDS/RESUMES threads: perturbation is explicit, not passive wait/exec/
EOF/usage/quiescence/cause proof. Original native13-query/8-thread/call/view/4096
report/ticket/clock and old assertions/deadlines/production/pump/reader/waker/
ProcessTree/environment/framework/SDK/WIT/deps/pins/workflows/frontends/frames/
schemas/goldens/trace remain intact. Guards/docs in progress; no inferred root
fix, public exec/usage/settlement/callables/P2g/live/default-on/release, model/
installed gadget/user config/env/SDK mutation, agents/goals or blocked-provider
work. See [the owned-stack scope and validation](owned-child-stack-capture.md).


Owned-stack experiment completed: ONE newly owned pre-kill child trace succeeded
(exit0/no signal,1.127321s,859 bytes) with a single `_dyld_start + 0` frame and
no binary-image description; symbol-lookup warning is retained. It is a perturbed
loader-entry userspace observation, not kernel wait cause/exec/EOF/usage proof or
a root fix. Original off1630/4/8 remains FAILED, gadgets1777/0/10 passes; all8 new
guards and original environment/auth cases pass, warnings0. The initial free
marker filtered two rustdoc cases: a targeted unmarked doc-only companion restores
exact combined inventory, NOT a claim of first-command full doctest coverage.
Future authorized markers use a nonmatching --skip VALUE; metadata-only unit/doc
lists prove no actual case exclusion. No unchanged broad retry/second sampler/old
PID lookup/assertion/deadline/launcher/permission workaround; #138 OPEN. See
[the complete owned-stack evidence](owned-child-stack-capture.md).
