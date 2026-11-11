# ACP compatibility audit — November 11, 2026

**The original adapter failed the stable-v1 audit. The compatibility repairs
now pass the same 39 checks locally.** Original failure evidence remains below;
the [current ACP contract](acp.md) describes the fixes and their limits. Native
cross-platform/full-mode validation remains a separate delivery gate.

## Original baseline and method

- Audited source: `9e5d96e291ca4e7e8b8db84a5462d265f2b133b3`.
- Installed default-build binary SHA-256:
  `c971058861b4104e3da4972d2748f2821ee3abf0d57e4e9f4e00d2bd648a1d96`.
- Official stable schema: [schema-v1.31.0](https://github.com/agentclientprotocol/agent-client-protocol/releases/tag/schema-v1.31.0),
  protocol major **1**, source commit
  `0849001ca66fb975fc01111ddab9093bc8304e98`.
- Schema SHA-256:
  `e747a7bec3c02708b70885bf3cccac5c72fa421390dd7e85758fd61e33b1ae21`.
- The separately documented v2 draft is outside this audit. Optional new v1
  capabilities/methods are not treated as required when they are not advertised.

An isolated strict JSON-RPC client drove the actual installed binary in nine
disposable fixtures, through stdio and the WebSocket bridge. A loopback-only
scripted HTTP model produced ordinary text and `write_file` tool calls. Each
fixture used isolated config/home/project directories and synthetic credentials;
the actual tool produced disposable files after client permission responses.
No paid model calls, user config/data changes, native sampling or runtime/golden
modifications occurred.

The client validated standard request/response/notification payloads against the
pinned JSON Schema and separately checked session identity, tool correlation,
permission selections, real file locations, history replay and event ordering.
The nine fixtures completed without harness errors: **39 checks, 19 pass and
20 fail**. Some failures repeat the same defect across transports or aggregate
schema checks; twenty is not the number of independent bugs.

Raw isolated transcripts, the probe, schema/meta assets and machine-readable
`report.json` are retained locally under
`target/verification/acp-compatibility-audit/`. The schema validator dependencies
are isolated in that directory's venv, rather than added to the Rust workspace.
This run covers the current macOS binary and a custom strict client; it does not
claim native Windows/Linux execution, every third-party client or full protocol
fuzzing.

## Original confirmed findings (all repaired in this change)

| ID | Priority | Finding and observed consequence | Source entry point |
| --- | --- | --- | --- |
| ACP-01 | High | `session/new.cwd` is ignored. A request for project B created `ordinary.txt` in the process's project A after valid approval. Relative filesystem operations can target the wrong workspace. | `acp_serve.rs:677`, `build_acp_session` |
| ACP-02 | High | Creating a second session while the first prompt waits for its model changes the shared current-session ID. The first prompt's response chunks were emitted under `audit-second`, rather than `audit-first`. | `ensure_session`, token sink at `acp_serve.rs:278` |
| ACP-03 | High | An unoffered option `allow-unoffered-audit` is accepted as approval because option parsing uses an `allow`/`approve` prefix. The actual disposable file was written. | `decision_for_option` at `acp_serve.rs:492` |
| ACP-04 | High | Permission requests reference `perm-1` as `toolCallId`, while the reported tool call is `audit-call-1`. A client cannot reliably associate approval with the active tool. | `acp_serve.rs:360` |
| ACP-05 | High | `session/load` returns `null`, which fails the stable `LoadSessionResponse` object schema. This was reproduced over stdio and WebSocket. | `acp_serve.rs:698` |
| ACP-06 | High | Loading `audit-never-created` succeeds and silently starts an empty thread instead of reporting that no stored session was loaded. | `ensure_session`, `session/load` |
| ACP-07 | Medium | Loaded conversation replay contains no committed `tool_call`/`tool_call_update` records after an actual approved write. The history projection keeps nonempty user/assistant text and drops tool activity/results. | `live_history`, `history_values` |
| ACP-08 | High | During a permission wait, the cancelled prompt response arrived before a later failed tool update. The client was told the turn had ended while its updates were still arriving. The file itself was correctly prevented. | `cancel_in_flight` at `acp_serve.rs:176` |
| ACP-09 | Medium | Two distinct assistant responses on the same session both supply `messageId: msg-1`. A supplied message ID should distinguish messages while remaining constant within one message's chunks. | token sink at `acp_serve.rs:278` |
| ACP-10 | Medium | A standard stdio MCP server supplied in `session/new.mcpServers` was ignored: session creation succeeded, its marker was never written and its tool was absent from the actual model request. | `acp_serve.rs:677` |
| ACP-11 | Medium | A valid initialize request for unsupported version 0 receives version 0, even though this implementation declares only version 1. The current `min(requested, 1)` rule is incorrect for an older unsupported version. | `negotiate_protocol_version` at `acp_serve.rs:612` |
| ACP-12 | Medium | Required session parameters are not admitted: `{}` and a relative `cwd` both create sessions. A JSON-RPC request with a non-string method is silently ignored instead of producing Invalid Request. | `session/new`, `handle_message` at `acp_serve.rs:624` |

These are observed adapter issues. They do not establish any new filesystem or
OS-containment boundary, nor attribute historical shell stalls to ACP.

## Behavior that passed

- A valid v1 `initialize` response matches the official capability schema.
- Valid session creation and two ordinary text prompts complete successfully.
- Known offered `allow-once` executes the actual disposable write; known
  `reject-once` prevents it.
- Cancelling a pending model request returns `stopReason: cancelled`, and a
  subsequent prompt remains usable.
- Cancelling a parked permission prevents the actual write, although final
  event ordering fails as described in ACP-08.
- Bridge `/healthz` succeeds; `/stats` refuses missing proxy credentials and
  accepts the synthetic configured secret. An unauthenticated WebSocket upgrade
  is refused with HTTP 401.
- The WebSocket bridge relays the same basic ACP requests/results and the same
  observed message/load defects. It is transport plumbing, not their remedy.

## Normative references and interpretation

- [Initialization](https://agentclientprotocol.com/protocol/v1/initialization):
  select a supported protocol version, falling back to the agent's latest
  supported version when the requested version is unsupported.
- [Session setup](https://agentclientprotocol.com/protocol/v1/session-setup):
  session requests include the session working directory and MCP configuration;
  loading resumes a stored conversation and replays its history.
- [Tool calls and permissions](https://agentclientprotocol.com/protocol/v1/tool-calls):
  tool IDs correlate updates and permission requests; an unknown permission
  outcome must not be treated as a known grant.
- [Stable schema](https://agentclientprotocol.com/protocol/v1/schema):
  `LoadSessionResponse` is an object; `ContentChunk.messageId` is optional, but
  when supplied it identifies the message whose chunks share that ID.
- [Prompt turns](https://agentclientprotocol.com/protocol/v1/prompt-turn) and
  [cancellation](https://agentclientprotocol.com/protocol/v1/cancellation): the
  final prompt response closes the turn after its updates have been delivered.

The request's JSON-RPC ID and `toolCallId` serve separate roles. A valid
permission request can use `id: perm-1` while referring to
`toolCallId: audit-call-1`; using the request ID for both loses correlation.
Repeated chunks within one assistant message should keep their ID. The defect
is reuse across distinct messages/turns, not repeated IDs within one stream.

No finding is raised solely because optional authentication, image/audio,
client filesystem/terminal operations, session modes or newer session-list/config
features are absent. Capability declarations and the behavior of supported paths
determine those obligations. Broad feature coverage remains a separate audit.

## Repair sequence and validation

1. **Permission and session integrity:** bind exact offered option IDs to their
   decisions, carry the actual tool-call ID into approval requests, honor the
   requested absolute `cwd`, and bind output to an immutable session/turn owner.
   Reject conflicting session operations if separate live session ownership is
   not implemented; never silently move active work between sessions.
2. **Resume and completion:** return a valid load-response object, refuse unknown
   stored sessions, replay committed tool records without executing them, and
   send the final cancelled prompt result only after owned turn updates finish.
3. **Protocol admission and capabilities:** distinguish message identities,
   implement correct version negotiation/Invalid Request handling, validate
   required session fields, and honor or explicitly refuse requested MCP setup.
4. Run the strict probes against the corrections, preserve original failure
   evidence, add meaningful native regressions and verify a real ACP client.
   Current frozen golden mismatches require a reviewed protocol change with
   explicit expected diffs; do not regenerate merely to make tests green.

The report does not authorize changing SDK/WIT/pins, SQL layout/native limits,
public headless execution, live model expense or publication. Source verification
and the existing green golden suite demonstrate current pinned behavior; they
do not override the compatibility failures above.


The owner subsequently authorized continuing the compatibility repairs. All
twelve findings above are implemented in `acp_serve.rs`, with narrow app-only
correlated approval/delegation callbacks and explicit MCP launch directory
plumbing. Other frontend defaults, SDK/WIT/pins, SQLite layout and native cleanup
limits are retained. The adapter explicitly opts into main-model wait
cancellation; terminal native/durable ownership remains a separate policy.

The first corrected source-built macOS binary passed **39/39 checks** across the
same nine fixtures and pinned stable-v1 schema. Its SHA-256 was
`9b23c0e03326b8cbf8afce91a697ff4747337439aa7d5c3908198f385d3d0087`.
This intermediate result is retained under
`target/verification/acp-compatibility-fixes/probe-first/`; final-head proof must
use the final binary rather than substituting this earlier result.

Seven additional native integration regressions now cover the findings and
session/model replacement, including real delegated writes, repeated raw IDs,
reopen of actual SQLite history, denied results, EOF drain and a marked native
MCP server. Existing ACP unit fixtures now initialize and create sessions in
held real temporary directories; the former null-load assertion is corrected to
the stable response object. The original assertions about real effects, usage,
tool statuses and cancellation are retained. First new-fixture compile and
expectation/discovery failures remain in the local fix artifacts; corrections
are limited to new field access and actual denial text/MCP discovery method.

The explicit golden review changes only two ACP recordings: permission request
and tool-update IDs now correlate, distinct assistant rounds use distinct IDs,
and cancellation carries accumulated completed-response usage through the
normal completion path. No major-version/capability change or bridge-golden
change is hidden by re-recording. The final native/full-mode/hosted verification
and independent-main records remain distinct from the original failures and
intermediate probe results.


### Hosted Linux prerequisite correction

[Run 43050231571](https://github.com/kaobei-alabo/craftsman-harness/actions/runs/43050231571)
at `fe55e809b752fecbaddd4c1b69e761982beb3eb6` failed Ubuntu's prerequisite step:
APT could not resolve `azure.archive.ubuntu.com` and exited 100 while fetching
CMake/ALSA/compiler packages. The default build, core tests and intermediate
contract/provenance checks were skipped. The independently executed gadget mode
passed, but does not substitute for those omitted steps. That failed job/raw log
is retained; an unchanged retry is not treated as delivery proof.

The narrow CI correction replaces only the runner-selected package URI in
Ubuntu 24.04's deb822 source file with the HTTPS Ubuntu package endpoint hosted by kernel.org, keeping
suites/components/signing keys and all four required package names. APT update
now fails on any index error rather than continuing with stale metadata. Runner
labels, toolchain, actions, test bodies/assertions, time limits, job/step
conditions and required contexts are unchanged. Ubuntu archive/security hostnames also failed local DNS resolution; the
selected kernel.org endpoint returned HTTP 200 for noble-security metadata.
This local observation does not substitute for successful runner setup. Fresh
all-four/every-step/both-mode validation is required after this correction and again on merged main.
