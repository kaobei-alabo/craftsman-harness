# Privacy provenance: source receipts and a separate model view

Status, November 22, 2026: this page records the contract the private privacy
work follows for getting text to a model. Private source, model-view and
restoration candidates exist, with the open consumers and validation gates below.
Nothing normally selects them, sessions are unchanged and privacy remains inactive.

It replaces an earlier idea of rewriting conversation history into
placeholders and deciding what was already masked from a role, a tool name or a
digest. That idea is not implemented and will not be.

## The contract

1. **Truth stays where it is.** The accepted input, the returned tool outcome
   with its metadata, the model's actual output and usage, the journal and the
   checkpoints remain exact. Nothing privacy-related replaces a real result
   with a "withheld" one, invents an empty answer, drops an edited or denied
   input, or resets usage.
2. **The model sees a separate view.** A model request is composed from views
   of sources, not from the canonical text. A failure to produce a view is a
   separate stop that happens after the real facts are retained.
3. **Authority comes from actual events.** An admitted node comes from an
   actual raw capture and the vault's successful commit of it, including a
   commit that found nothing. A model node comes from an actual transport
   receipt and the actual commit after review. A role, name, index, digest or
   flag creates neither.
4. **Parts keep their source until the request is composed.** System text,
   todo text, memory, context notes, tool descriptions, compaction summaries,
   task results and inbox messages are made of parts with different sources.
   They are joined only by a composer that still knows each part.
5. **Unsupported means refused.** A selected producer, middleware or provider
   that has no source-preserving path refuses. It is not skipped, guessed or
   silently removed from the user's tools.
6. **No storage claim.** This design does not encrypt or redact stored
   history, checkpoints or exports. If a policy requires that, activation must
   refuse until a separate design exists.

The first supported scope is masking of supported values in raw-origin text,
plus the existing guard on final request bytes for values already known. It is
not detection of everything a model writes or of opaque provider content.

## State of each part

| Part | State |
| --- | --- |
| Raw captures, admitted nodes, conversation binding ([admission](privacy-admission.md)) | implemented, private |
| Framework boundary: sources, typed turn, steering and approval entries, dispatch check, the agent's own messages, middleware statements, tracked review, tracked system additions and tool outcomes as source parts | implemented, private; see below |
| Conversation boundary: sources, composition, result records, checked operations and fields | implemented, private; see below |
| Providers for all four wires (Chat, Responses, Claude Messages, Gemini) through the existing clients and guard, with receipts | implemented, private; see below |
| Protected repair of tool calls (`patch_tool_calls`) as a recorded change of the received reply | implemented, private; see below |
| Todo list, memory section, todo reminders, task workflow notes, workspace checkpoints, and the system prompt as sections | implemented, private; see "Normal parts" below |
| Compaction: automatic, and manual or idle through a typed entry | private explicit summary projection, actual stage/commit witnesses and retained native items for unchanged moved replies; see "Compaction" below |
| Session builder selection and typed session entries | private full ordinary component set; no limited-set constructor. Actual policy/frontend selection remains absent; configured unsupported components refuse; see below |
| Delegated agents with `task`, notifications and inbox | private actual child/parent/retained-envelope adapters; task returns retain real partial state and facts |
| Verification, context and failure controls | private adapters around the original controllers, installed by the session builder. Context and failure controls, a check the model asks for, a check the verification controller schedules, its stop and a manual compaction run in built sessions on all four wires; see "Controller adapters" below |
| Sealed parked-call human decisions | private registered input producer; actual decision and raw edit retained, no caller action factory |
| Decisions inside the approval gate | local registered policy/input owners and opaque request/presentation/original-decision transport; actual A UI receiver/modal consumer still pending |
| `ask_user` answers as drafts | actual receiver consumer still pending |
| Terminal ingress, `/clear` | not implemented |
| Drawing replies in the terminal | implemented, private: the root boundary tells a frontend's display when the agent has retained a reply, and the terminal's bridge and application draw its human view once through the safe backend. The stream is held and never drawn; see [privacy-restore.md](privacy-restore.md) |
| Reopening, transferring or rebinding a privacy conversation | refused |
| Restoration into what a person reads and into the content of `write_file` and `edit_file` | implemented, private, for the root agent and for delegated agents in their own lanes. The file operation is the source the boundary returns from `before_review` for the agent's plan, its digest is what `reviewed_operation_digest` gives, and the independently registered gate input/policy decision travels as a non-authoritative annotation and is validated against its actual issuer, plan, preview and final invocation at the file tool; the file tool runs its own checkpoint and diagnostics around the derived write; see [privacy-restore.md](privacy-restore.md). Every other tool argument, memory files, delete and rename are not restored |
| Key envelope, unlock and captured user policy | a private backend exists on a separate local branch and is not part of this one. The builder selection takes the user's captured policy through the `CapturedUserPolicy` trait. Nothing in this branch implements that trait outside tests, so no selection can be made here |
| Secret input, frontend selector, activation | not implemented |

## Framework boundary

The framework crate has an optional boundary, `ModelViewBoundary`, selected
per agent with `AgentBuilder::model_view_boundary`. The default is none, and an
agent without it runs exactly its existing paths. The crate gained no
dependency and knows nothing about what a source is: a `SourceRef` is opaque
evidence that the selecting host supplies and checks itself.

With a boundary selected, the agent keeps one slot beside each recorded
message, in memory only. Nothing is read back from a checkpoint.

**Input.** A turn is accepted only through `run_turn_with_view(input, source)`
and steering only through `steer_with_view(input, source)`. The text is
recorded exactly as the ordinary entries record it, at the same points, and
its source is kept beside it.

**Requests.** Each request is a `TrackedModelRequest`: the recorded messages
and tool definitions with one source per message and per definition, and the
sources of the system channel from the boundary. The provider receives it
through `ModelProvider::complete_with_view`. A provider that does not implement
that method refuses; the recorded text is not sent in its place.

**Replies and results.** Returned provider data and actual usage are kept in a
private fact owner before receipt, projection or review can fail. This record
grants no transport or committed-origin authority. A reply is committed, journaled and saved first, then
reported to the boundary with the provider's receipt. A tool result is
journaled and committed first, and saved there when the agent saves after each
result, then reported with what produced it: every run of the tool itself with
the exact call and its own outcome and metadata, before any middleware saw it,
and what the dispatch chain finally returned. If the boundary returns no
source, the real reply or result stays recorded with its usage and the turn
stops.

**Before a call runs.** A source check precedes dispatch. After middleware,
each actual terminal invocation has its own immutable call/decision witness,
source check and final synchronous live check before the tool is polled. Actual
terminal outcomes are retained before middleware can project or fail them.
Source approval grants no execution permission. A refusal never invents effects
or results for a tool that did not run.

The final frontier also checks the real turn owner after fallible source work
and immediately before a new selected tool is polled. Cancellation seals new
invocations and steering; a running tool keeps its existing cleanup ownership.
Already accepted input is retained before the typed entry returns, including a
parked return, and late cancellation is sampled under the same guard that seals
further admission. A completed cancellation uses the actual cancellation
handler's return; a stop after an already completed end/save remains an error
without inventing another answer or replaying the end handler.

A selected entry/source failure that has not started its end handler runs the
original handler and final save over retained canonical facts. An already
attempted handler or failed save is not retried. Idle compaction keeps its own
lifecycle. Actual entry, handler and save errors remain in private memory and
in a child's actual task return as data; the initial entry error and original
save-error precedence remain, and none of these observations is a provenance
or completion certificate.

The framework also issues an `ActualPlannedInvocation` before the real review
chain. Its source and the host's source-bound preview digest stay with a parked
call and with each terminal that follows that review. Equal arguments or a reused
call id cannot recreate the event. `was_planned_as` proves event linkage only;
the terminal must still check its exact final arguments, real decision and live
source. The registered human input callback receives that same optional digest
and retains it with the actual decision variant. A middleware can carry a
host-owned decision source to the terminal, but the annotation is no permission
certificate: an application verifier must require its private actual human or
policy producer. File and inline approval adapters remain separate pending work.

Control helpers keep real input/source events and host-scheduled checks distinct
from model-origin requests. An `ActualBoundaryInput` is issued only after the
actual accepted input append, with its immutable commit/source and installed
controller, before that input's checkpoint. Its callback entry closes at actual
return/drop. The kind and index are locators, not human or policy authority; an
application adapter must require its own private input producer and consume the
event once. `AgentBoundaryPolicy::user_input_with_view` defaults to refusal. A
host-scheduled completion check similarly holds its real carrier commit and
installed controller, and the plan carries that actual scheduling event. Its
source hook also defaults to refusal. These helpers do not lift the current
startup refusal for selected boundary/completion policies, and add no app
dependencies, checkpoint fields or default request changes.

The core compatibility gate now defaults to refusal per installed controller:
a boundary policy must declare its actual tracked adapter and supply a source;
a completion controller must supply its source. The actual builder captures
those objects and sources in `ActualControllerBindings`, and the selected host
must validate its private issuer and exact installed object before input/save
or custom mutation. `validate_controller_sources` defaults to refusal for any
controller. Declaration and `Some(SourceRef)` are feasibility only. The
unadapted helper remains refused. The application's adapters for its original
controllers are described under "Controller adapters" below.

**Approvals.** A parked call is resumed only through
`resume_with_view(action, deny_default, decision)`. The boundary checks the
host's evidence of the decision against the parked call and the action before
anything is applied. A refusal leaves the call parked. An approved call, with
edited arguments if the person edited them, then passes the check above.

**The agent's own messages.** The agent appends some messages itself: the
result of a call skipped by a cancellation, the closing note of a cancelled
turn, a recorded denial, the result of a call left unanswered by an
interruption, and a notification from another agent. Each is recorded exactly
as an unselected agent records it. The event is noted where the message is
appended, and the boundary is asked for its source afterwards, at the end of
the entry or before the next request. An ordinary cancellation therefore no
longer ends a conversation. A boundary that does not know such a message
refuses, and no later request is composed.

**Messages that arrive with parts.** A notification can be posted with the
parts its text is made of (`NotificationQueue::push_with_parts`), and a
delegated agent's inbox takes parts. The receiving agent records the same text
as always and keeps the parts beside it: for a notification, its own fixed
framing and the sender's name around what the sender said. The boundary is
asked through `observe_envelope(event, parts, committed)`. A notification that
was posted without parts still has no source. A tracked middleware can also
add a message of the host's own before a round
(`TrackedPrepare::append_host_message`, with `set_own_custom` for its own
bookkeeping key): the message is recorded with the user role where the
ordinary hook records it, its source comes from its parts through
`observe_envelope` with the event `HostNote`, and it is committed with the
round. Every other receiver of the
queue sees the notification exactly as before.

**Middleware.** A middleware runs on a selected agent only if its author made a
statement through `AgentMiddleware::view_support`. Without one, the typed
entries refuse before any effect.

- *Preserving*: its hooks leave alone everything the agent records or carries
  between rounds, which is messages, todos, files, custom state and system
  additions, and it does not need the request text. The statement is checked,
  not trusted. The agent compares that state before and after every hook phase
  and stops the turn if anything changed. Around a tool call only the tool
  itself may change it: the agent compares the state when the tool is entered
  and after the chain returns, and stops the turn after recording the result
  if a middleware changed something. `around_model` is never called on a
  selected agent; `around_model_with_view` is, with a request that can be read
  and passed on but not changed.
- *Tracked reply*: as above, except that its review changes the received reply.
  Its `review_model` is not called. `review_model_with_view` is, and receives
  only the reply, through `TrackedReply`. The single operation there replaces
  the arguments of one call and records who replaced what with what. Those
  records go to the boundary with the commit. A boundary that does not know
  them refuses the changed reply; nothing is worked out afterwards by comparing
  texts.

The protected `patch_tool_calls` middleware is the one tracked reviewer. On a
selected agent it repairs string-encoded or malformed call arguments exactly as
it always does, and the record is the same as an unselected agent's.

**Source parts.** A `SourcePart` is one piece of text with where it came from:
fixed application text, text read from a store or file, or text that a tool
call carried in its arguments. Pieces stay apart until the host composes the
request. A piece is owned by its producer: handing over the same owned piece
again is the same source, while equal text in another piece is not.

- *Tracked prepare.* A tracked middleware's `prepare_model` is not called.
  `prepare_model_with_view` is, with the state read-only and bounded operations:
  add a section of system text as parts or stage a sourced summary. Each section is given to the boundary
  and becomes one more source of the system channel. Bounds: 64 sections a
  round, 1024 parts and 1 MiB a section.
- *Tracked dispatch.* During a tool call, a middleware reaches a handle through
  `ToolNext::tracked` and a tool through `Tool::call_with_view`. A middleware
  announces a bookkeeping key of its own in the custom state, or fixed text it
  appended to the outcome. A tool announces that it stored strings of its own
  arguments as the todo list, or what its own outcome is made of. Anything
  changed without an announcement stops the turn, as before.
- *Stored todos.* The agent keeps, beside the stored list, the reply, the call
  and the argument place each todo's text came from. A list changed by
  something that did not say so has no origin, and the next request is
  refused.
- *Outcome parts.* When a tool says what its own outcome is made of, the agent
  uses the statement only if the pieces join to exactly what the tool returned
  and the recorded content is exactly that plus what middleware appended.

- *Summaries.* A tracked preparation can replace recorded messages with a
  summary of them. It names the positions to summarize and to keep, the
  summarizer, and the application's own text. The agent builds the
  summarizer's request from the summarized messages with their own sources,
  asks the summarizer through its view path, records the summary message as
  the ordinary compaction does, and asks the boundary for its source. The
  kept messages keep their slots. The replacement belongs to the round's
  working state: it takes effect when that state is committed, and is dropped
  with it when the round fails or is cancelled.

The defaults of the `_with_view` hooks are written without `async`, so that an
implementation that does not override them gets one small function instead of
a state machine. The test binary of the application is close to a linker limit
on macOS that these state machines exceeded.

**Delegation.** The framework's side of it is described under "Delegated
agents". What it adds here:

- `TrackedDispatch::boundary()` and `argument(pointer, text)` give a tool the
  boundary of the conversation and a part that names the committed reply
  that asked for the running call. A call that a person changed before it ran
  is not a call of that reply: the agent gives the tool's outcome no parts
  that name the call's arguments, and the stored todo list no origins.
- `TrackedDispatch::invocation()` carries the actual post-chain terminal.
  `argument_source_for(invocation, part)` and `child_boundary_for(invocation,
  system)` require it to match the private live operation. Legacy caller-part
  and child factories refuse in the application. The child builder consumes the
  exact parent witness before binding its own actual agent owner.
- `TrackedModelRequest::agent_owner()` and `message_inputs()` are framework-issued
  actual owners/events. The application resolves registered private lanes from
  them; `conversation()` is only compatibility metadata and grants no authority.
  A source-derived summarizer request is separately marked `is_summary()`.
- `PartFrom::Source` and `OutcomePart::Source` name the whole text of
  something the host already holds a source for. `OutcomePart::Argument` and
  `OutcomePart::Read` name a string of the call's own arguments and text the
  tool read from configuration.
- `Agent::run_task_with_view` runs one delegated task on a selected agent and
  retains its actual return, partial state, provider data and tool facts before
  later formatting/source work, beside the ordinary result and its final source.
  The activity guard precedes seeding and stays intact; task setup never clears
  accepted input or cancellation plumbing.

The three new boundary methods that take the place of an `async fn` are
written as plain functions that return one shared ready future, like the
other new defaults.

Still refused on a selected agent because no source-preserving path exists: the
untyped turn, steering and resume entries, reopening a thread, transferring a
conversation, a boundary policy, a completion check, system text set on the
agent, untracked system additions and an inbox. Typed Chat idle compaction and
tracked additions have their separate paths described below.

Durable reconciliation of calls whose completion was not recorded keeps every
slot beside its message.

## Conversation boundary and Chat provider

The application has the other side of that boundary for one privacy
conversation, and one provider that uses it.

**Sources.** `PrivacySession::boundary(system)` returns the boundary for the
root agent of a conversation, once. A boundary for a delegated agent is made
only by a boundary of the same conversation. A source it hands to the
framework is one of these private objects:

| Source | Minted when | Sent as |
| --- | --- | --- |
| Admitted node | raw text was captured and admitted: typed input, system text | the node's view |
| Committed model node | this conversation's provider received a reply and the agent committed it | the recorded reply, unchanged |
| Tool definition | a tool's description and schema text were captured and admitted | the description's view; the schema as recorded |
| Tool result | the agent recorded a result; its content was captured and admitted | the content's view |
| The agent's own message | the agent appended it at a named event; its text was captured and admitted | the text's view |
| Argument text | a tool handed a string of the running call's arguments to a delegated agent; the string was checked against the committed reply that asked for the call | the text, as the model wrote it |
| Message with parts | the agent appended a notification or an inbox message that came with parts; each part was checked or admitted | host parts as their views, model parts as the model wrote them |

A committed model node is model-origin text. It is not an admitted node and
gives no right to restore anything. It keeps the reply as it was received, the
reply as it was committed, and the recorded changes between them. What was
committed must be what was received with those changes applied in order, or
that reply with its calls removed because the round ended without running
them. Anything else stops the session.

A conversation has one agent. Its boundary is handed out once.

**System additions.** For each section a middleware adds, the conversation
holds its parts. Fixed and read text is captured and admitted, once per owned
piece, and sent as its view. Text a part says a call wrote is checked against
the committed reply: the string at that place of that call's arguments, or its
JSON encoding, must be exactly the part's text. It is model-origin text and is
sent as the model wrote it; a placeholder in it is neither replaced by the
original nor masked again. A part that fails the check stops the session.

**Results in parts.** When a tool said what its own outcome is made of, the
result is held piece by piece the same way, followed by what middleware
appended. Otherwise the whole content is captured and admitted as raw text.
This matters for a tool that repeats its arguments: the todo tool's outcome
repeats the list, and a todo in which the model wrote a placeholder would end
the conversation if the outcome were admitted whole.

**Composition.** `PrivacySession::model_view` turns a tracked request into the
request a model may see. Each recorded text is replaced by the view of its own
source. Roles, order, names, call ids, the model's own text and the arguments
of its tool calls are the recorded ones. A source is used only when it is this
session's own object and still fits the item beside it: the captured text
equals the recorded text, a reply equals the reply that was committed, a result
belongs to that call. Every retained item also requires the same actual commit
witness and complete immutable role/id/status/text. Anything else stops the
session before a request exists.
That includes a source from another conversation, a draft in the system
channel, a recorded message that was changed after its source was made, and a
tool schema in which admission found a value, because a schema cannot be
changed without changing what the tool accepts.

**Tool results.** The recorded content is captured as raw text and admitted. If
it cannot be admitted, the session stops; the result stays recorded exactly as
returned, and nothing is put in its place. The capture is labelled as the
tool's own content only when the last run of the tool returned exactly the
recorded content. Otherwise the chain wrote it: a denial on the way, a cut, or
the text of a failure. Beside the admitted content the conversation keeps an
`ActualToolResult`: the call id and tool, every run of the tool with its exact
call and own outcome, the chain's outcome, and the checked operation.

**The agent's own messages.** The fixed results and the closing note are
captured as host text. A recorded denial is captured as a person's reply,
because it carries their words. A notification or an inbox message that came
with parts is held part by part: the framing and the sender's name are host
text and are admitted, and what a part says is the text of a committed reply
or of a call's arguments is checked against that reply and kept as model
text. A notification that came without parts has no source of its own and
stops the session.

**Checked operations.** Before a call runs, the boundary requires that the last
committed reply asked for exactly that call, or that a checked decision allowed
exactly it. Anything else stops the session before the call runs. For the call
it lets through it keeps an `ApprovedOperation`: the exact call, the committed
reply that asked for it, whether a person replaced the arguments, and the
fields of the arguments. A component at the tool can read it through
`ConversationBoundary::operation_in_flight`.

**Decisions.** `PrivacyView::request_decision` reads only the private producer
registered by the actual builder, against the framework's actual parked-call
witness. The producer retains the exact human choice and raw edit before parsing
or admission. A caller action/JSON factory cannot issue a decision. Every edit
is human-origin even if its JSON equals the model arguments, and raw issued
aliases refuse before effects. The actual Always update follows acceptance.

**Fields.** A `SourceField` is one occurrence, in model-origin text, of a
placeholder that a commit of this conversation issued. It gives the place (the
text of a committed reply, or a string value of a call's arguments by JSON
pointer), the byte range, and access to the record of the commit that issued
the placeholder. A committed model node lists the fields of its reply text; a
checked operation lists the fields of the arguments the model wrote. A spelling
that no commit of this conversation issued is not a field. Arguments a person
typed are raw text and have no fields. Overlapping occurrences give no fields
at all. A field states where a known placeholder was written. It is not a
permission to put the original there: that decision, with its own syntax,
destination and approval checks, belongs to the restoration work.

## Providers for the four wires

Two providers send the view. Both are the existing clients and encoders, bound
to the conversation's send guard, so the final bytes of every attempt still
pass the guard under the existing transport policy. Both refuse the ordinary
`complete` call, so recorded text cannot be sent through them. Both keep the
real usage.

**Receipts.** The providers live inside the conversation module and are the
only producers of a reply's receipt. The functions that make one are private
to that module and are called only there, at the moment a client returns a
reply, before the agent reviews or commits it. No other code can make a
receipt from a message it holds. Every receipt, on all four wires, carries the
position of the reply and the SHA-256 of the exact request bytes that were
sent, taken where those bytes are sealed for sending.

The combined correction verifies a framework-issued actual retained commit
and the full immutable message against the private transport receipt, with each
tracked review edit kept. Caller messages, equal hashes and mutable/checkpoint
state cannot issue that commit. The providers retain actual reply/usage data
before later receipt checks; that fact record alone grants no source authority.
The private human producer and actual parked-call witness replace the public
human-decision factory. Inline gate and question adapters remain unfinished.

**Chat.** `ViewChatProvider` composes the view, encodes it with the functions
the ordinary provider uses and sends it with the existing retry policy. It
refuses a client that has a native transport, and it refuses a conversation
in which a reply was received by a native transport: those items are not sent
on another wire.

**Responses, Claude Messages and Gemini.** `ViewNativeProvider` uses the
existing native client: endpoint, headers, protocol version, the 4 MiB request
and event bounds, the 16 MiB response bound, stream parsing, cancellation and
usage are that client's, unchanged. There is no retry, redirect or fallback to
another wire, as before. What differs:

- The request is the view, and the provider items sent back for an earlier
  reply come from this conversation's own receipts, not from the client's
  history cache. That cache is neither read nor written, the history
  middleware is not installed, and nothing about provider items is written to
  the checkpointed state. A checkpoint cannot produce a receipt.
- A native receipt also holds the client's identity (endpoint, model,
  protocol and credential, as one digest) and the provider's items exactly as
  received, signed and opaque ones included. The items are validated against the reply with the native
  transport's own check and size bound before the receipt exists.
- Items are sent back only with the reply they were received with, by a
  client of the same identity, and only if that reply was committed as
  received. Then they are sent back exactly; nothing is stripped or
  reformatted. They stay with the reply's node, together with the position
  and the digest of the request the reply answered, and go back with the
  reply wherever it stands: a compaction that moves a kept reply does not
  take its items away. A reply changed from what its native receipt holds,
  including removal of its original calls, refuses later normal replay. Its
  original data stays retained; no portable fallback silently replaces items. A reply with
  no receipt, or with another client's, stops the session before a request
  exists. A native reply that a review changed is refused with the native
  transport's own message, as on the ordinary path.
- A request the agent composed for a summarizer is refused by every provider
  except the summarizer's, and the summarizer's provider refuses every other
  request; see "Compaction".
- At most 256 replies with provider items are kept, the native history's bound.

**What a client adds by itself.** The model id, the fixed fields of its wire and
the configured generation settings are captured once as host configuration:
for a native wire, the encoding of a request with no system text, message or
tool, made by the same encoder; for Chat, the model id and the configured
settings. That capture is admitted like any other raw source. A setting that
holds a supported value cannot be replaced without changing what it means, so
such a client is refused and the conversation that was offered ends. The
capture is checked against the client again before every request. Request
headers, including credentials and configured headers, are outside the view
and the guard, as they always were.

**Scope on native wires.** What is masked is raw-origin text: typed input, tool
results, system text, tool descriptions. What is checked last is the existing
guard: the final bytes, provider items included, must not contain an original
this conversation already holds. Encrypted reasoning, signed thinking and
thought signatures cannot be inspected, and nothing here claims to know what
is inside them. A user policy that requires inspectable provider items
therefore refuses a native wire at build.

## Session builder selection

The shared session builder accepts one private selection,
`PrivacyViewSelection::from_user_policy(policy, session)`: the user's captured
policy and the live conversation that was opened under it. The policy comes in
through the `CapturedUserPolicy` trait, which the application's policy owner
implements for the policy it captured from the user's own configuration before
any project file was merged. It answers two questions: whether the policy
enables privacy for the conversation's workspace, and whether everything a
provider is sent must be inspectable. A flag, a label or a digest is not a
policy. Nothing in this branch implements the trait outside tests, and no
frontend, configuration key or command makes a selection.

Making the selection is refused when the policy does not enable privacy for
the workspace or the conversation has ended. The builder's own result has no
room for a reason, so the selection carries a report: when the builder refuses,
`selection.report().refusal()` gives the host a typed `BuildRefusal`, including
the names of the parts that have no source path.

The selection carries no permission to run with fewer parts than the user
configured. Before configuration, stores or providers are touched, the builder
refuses:

- a caller's thread id, a separately selected guard, the prepared headless
  runtime and an owner that has already ended;
- a request for a script capture, whose source adapter is still unavailable.

Declared verification, context compilation and failure controls are no longer
refused at this point. Their controllers are made from the user's settings as
for any session and installed behind the conversation's adapters; if one of
them cannot be installed that way, the build is refused then, with the same
typed reason. None is ever left out.

Then it:

- uses the conversation's reserved thread id for the agent and the checkpoint
  store, and the conversation's own guard for every model client it makes;
- builds the provider for the configured wire, and refuses a native wire when
  the policy requires inspectable provider items;
- takes the sections of the system prompt from their producer while they are
  still apart, admits each as its own capture, and gives the agent no system
  text of its own. A section that holds text of several origins is taken
  piece by piece: the global instruction file, the project instruction
  file, each discovered skill, each extension's instructions and the gadget
  authoring skill are each their own capture, made where the text is read
  and before the section is joined, with the application's own headings
  between them as fixed text. What a section's trimming removes from the
  edge of a file is not part of the prompt and is not captured. Joined, the
  sections must be exactly the text an ordinary session is given, or the
  build is refused;
- requires a statement from every middleware it would install. If one has
  none, the build is refused and the frontend is told which.

Today the statement exists for the approval gate, the result size limit, the
protected call repair, the todo list, memory, the todo reminders, compaction,
the task workflow notes, workspace checkpoints, the delegated agents and the
middlewares of the verification, context and failure controllers. With
the ordinary configuration that is every installed part, so a selection made
from a policy builds the session with all of them: the same tools, the same
system text and the same delegated agents as an ordinary session. The builder
has no way to leave a part out. The earlier test-only selection that left
parts out and named them is gone.

The model-visible gadget runtime has a source path too; see "Gadget runtime".
A script capture has none. A frontend asks for it explicitly, and a selection
that does is refused before anything is built. If a middleware without a
statement were installed
some other way, for the root agent or for a delegated agent, the build would
be refused and would name it.

The policy itself is still a trait with no implementation outside tests in
this branch, so nothing can make a selection here.

The session keeps the view. `Session::set_thread_id` on such a session closes
the privacy owner and changes nothing else; reopening and transfer are refused
as they are for a guard-bound session. A new conversation is a new owner, a new
reserved id, a new scope and a new session.

**Typed entries.** `PrivacyView::compact` runs the manual compaction of an idle
conversation through the tracked path. `PrivacyView::turn`, `steer` and
`request_decision` are typed entry points. Terminal drafts are captured by the
entry; decisions come only from the registered private human-input owner. Each captures the text as a draft and admits it before
it is accepted, so a draft that cannot be admitted is turned away unchanged and
the conversation continues. A reason typed with a denial is checked the same
way. Nothing calls these entries yet.

**In the fixtures.** A fixture session is built from a fixture policy through
the same selection a user's policy would make. Its system text is the ordinary
session's, byte for byte, the delegated-agent section included. Reading the
checkpoint back with an ordinary session returns the real record: nothing in
it is redacted or encrypted by this work.

## Compaction

The supported Chat path uses the ordinary trigger, thresholds, complete split,
kept tail, user-role summary framing and usage-anchor reset. Every original is
accounted for exactly once. Summarized messages retain their own immutable
source/event witnesses; the host instruction has a request-input witness and
no conversation-retention claim. The summarizer shares the conversation and
guard, with its own usage counter.

The actual reply and usage are retained before framing/source checks. The
opening and closing host text stay apart from the summarizer's model text;
issued aliases in that model text are never re-masked. A distinct working-state
stage becomes a retained-summary witness only at the actual canonical commit.
Failure or cancellation of an uncommitted stage leaves original canonical
messages intact while retaining actual received/staged facts. A save failure
keeps the real memory result and remains a failure.

`Agent::compact_idle_with_view`, reached through `PrivacyView::compact`, keeps
exclusive activity, the original durable-save conditions, pending-approval
refusal and before/after saves. The untyped entry stays refused.

A summarizer receives a separate explicit source-derived request containing
committed text/calls and its host instruction. It retains every original
native receipt/item in the private fact/source history. An unchanged kept reply
replays its original native items after position movement, using the original
client identity and request context. A changed native reply refuses later
normal replay. This explicit projection does not inspect opaque content or
prove acceptance by a real provider; policies requiring inspectability still
refuse native wires. Only loopback/native fixture proof exists here.

## Delegated agents

Delegation on a selected agent does what the ordinary delegation does: the
same `task` and `send_message` tools with the same definitions, the same
registry entries, ids, statuses and progress labels, the same limit on
running agents, the same foreground and background runs, the same
notification texts, the same file merge and the same recorded conversation.
What differs is that no text goes from one agent to another as recorded
text. Where the conversation has no source for a piece, the task is not
started; nothing falls back to the ordinary path.

- **A lane per agent.** Every agent of a conversation has a boundary of its
  own: the root agent's, handed out once by the session, and one per
  delegated task, made by the delegating agent's boundary
  (`child_boundary_for`, with the actual terminal invocation). All of them are the same session: one vault, one set
  of placeholders, one guard. Each has a lane. A reply is received for the
  lane of the request it answered and can be committed only by that lane's
  boundary. A request carries, as its own replies and results, only those of
  the lane it was composed for. A delegated agent's request never carries a
  reply of the root agent, or its provider items, and the other way round.
- **The delegated agent's system text** is its definition's prompt, admitted
  as host text and kept apart from the root agent's. The agent has no system
  text of its own.
- **The task description** is text the model wrote in the `task` call. The
  tool asks the boundary for its source while the call runs
  (`argument_source_for`, with that invocation): the boundary checks it against the committed reply
  that asked for the call, and it becomes the delegated agent's input as the
  model wrote it. If a person replaced the arguments at an approval, the
  description is that person's raw text and is admitted like any other. A
  call without a description starts the task with an empty input, as always.
- **The provider.** The session builder gives delegated agents a provider of
  the same conversation on the configured wire: the same owner and guard,
  its own usage counter. The progress wrapper around it and around each tool
  passes the view path through.
- **What a task ends with.** A foreground task's result is the text of the
  message the delegated agent ended with. The tool states that with the
  source its boundary keeps beside that message, and the root agent's
  boundary checks it: a committed reply of a lane it delegated to, with
  exactly that text. It is then sent as the model wrote it. A task that ends
  with its agent's own closing note comes back with that note's source. The
  tool's other outcomes are host text and are admitted as results: a
  refusal, the limit, a failure, a suspension. A name the call wrote that
  the tool repeats, for an unknown agent type or for the recipient of a
  message, stays text of the call.
- **Background tasks** post their completion with parts: the reply's source
  for a finished task, the host's own text for a failure, and for a
  suspended task the gate's summary, which is the tool's name and the JSON of
  the parked call's arguments as text of the reply that asked for it. The
  root agent records the notification inside its own framing, as parts.
- **Messages.** `send_message` resolves the message like a description and
  delivers it, as a part, to the inbox of a running delegated agent of this
  conversation. That agent records it where an ordinary one reads its inbox,
  before a round's request. A recipient without such an inbox gets nothing,
  and the tool says so as it always did.
- **A delegated agent's parts.** Every middleware a delegated agent would get
  must have made its own statement: the middleware the host installs in each
  delegated agent, the ones its profile adds, and the policy gate for its
  listed tools, which has one. If one has none, the delegation middleware
  makes no statement either and the session is not built.
- **Bounds.** A message with parts has at most 64 parts and 1 MiB. A
  conversation makes at most 4096 lanes. The inbox and the notification
  notification subscribers remain the existing unbounded channels. The selected
  inbox uses its bounded 256-message queue and refuses a full delivery; none of
  those counts is source, approval or completion authority.
  A background task runs as two spawned tasks of one erased type, as the
  ordinary one runs as two.

What a delegated agent's tools ask a person inline, through the existing
gate around each delegated tool, is decided as before and recorded as that
tool's result. There is no decision witness for it yet.

## Gadget runtime

The model-visible gadget runtime is selected as it always is: the user's
`gadgets.enabled`, the terminal's own opt-in, source and capability approval,
build approval and first-load approval are unchanged, and nothing is
described, compiled or run before approval. A privacy conversation changes
none of that. What it adds is where the text comes from.

- **The catalog update** is still a message the catalog middleware appends
  before a round, only when the catalog changed or a compaction removed the
  last update, for the root agent and for every delegated agent. On a
  selected agent it is appended as parts: the application's framing and
  guidance as fixed text, and the fingerprint and the catalog JSON as data
  the host read from its gadget runtime. Those two are captured and admitted
  as raw host text. Nothing in the update is model text, and the boundary
  refuses a host note that says otherwise. The fingerprint is kept under the
  middleware's own key as before.
- **The gadget tools** are ordinary tools. A `gadget_list` or `gadget_call`
  result is recorded as the tool returned it and admitted as a tool result,
  with what actually ran. What a guest prints is raw text whatever it says:
  it is neither an approval nor a source.
- **Declared prompts, commands and agents** of a gadget manifest come in
  through the same producers as before: instructions as their own piece of
  the system prompt, agents as delegated agents with lanes of their own.
- **The gadget authoring skill** follows the last section of the system
  prompt directly, as the ordinary text has it, and is a piece of its own.

The private headless guest host, callable headless gadgets, public exec and
script capture are untouched and stay off.

## Normal parts

How each normal component runs on a selected agent today.

| Component | Path |
| --- | --- |
| System prompt | sections from the producer: fixed text, the profile's text, instruction files, discovered skills, extensions and the frontend's addition. A section of one origin is admitted on its own. The instruction section is the global file, the project file and the application's headings as separate pieces; the skill and extension sections are one piece per skill or extension |
| `patch_tool_calls` | tracked review; each repair recorded |
| Todo list | the section is fixed prompt, heading, status labels and line ends from the application, and each todo's text from the call that stored it. The tool states which arguments it stored and what its outcome repeats |
| Memory | the turn-start load is unchanged. The section is the application's tags, paths and guidance, with each file's content as one read part per load |
| Todo reminders | announces its round counters in the custom state and the fixed reminder it appends to a tool outcome |
| Task workflow notes | the two fixed notes as parts; its observation of outcomes stays in its own memory |
| Workspace checkpoints | reads recorded messages and writes its own store; changes nothing the agent records, which is checked |
| Approval gate, result size limit | leave the record alone, which is checked |
| Host tools | a result is the tool's own raw text. Three outcomes repeat a string of the call's arguments and say so piece by piece: `write_file` and `edit_file` the path, with any language diagnostics as host text after it, and `glob` the pattern it found nothing for. The piece is bound to the reply that asked for the call only if the call ran as that reply wrote it. The gate around a delegated agent's tools passes the statement through |
| Compaction | tracked: same trigger, split and record as the ordinary compaction; see "Compaction" |
| Delegated agents, notifications, inbox | tracked: a lane per agent, the description and messages as text of the call, results and notifications with the source of what the agent ended with; see "Delegated agents" |
| Verification, context and failure controls | the original controllers behind the conversation's adapters; see "Controller adapters". The context section is added as parts, the failure advisory and the verification note as fixed text, and each controller writes only its own keys of the custom state |
| Gadget runtime and catalog | tracked: the update as a host note of parts; see "Gadget runtime" |
| Decisions inside the approval gate, `ask_user` answers | recorded as results; no decision or answer witness yet |

## Limits that will remain

- A raw text that contains an issued placeholder is refused, including a
  telephone number that equals one.
- Provider items that are signed or opaque are kept exactly as received and
  sent back exactly. If a policy needs to inspect one that cannot be
  inspected, that wire is refused; nothing is stripped or rewritten.
- A model node is not an admitted node and gives no right to restore anything.
- A lane resolves only an actual registered `ActualAgentOwner`. Its parent is
  the genuine terminal/child-construction witness, not a registry name, label,
  hash or ordinal. Every message/source also matches its actual immutable commit.
- `ActualTaskReturn` retains real ending/partial state and child provider/tool
  facts. Registry statuses, `TrackedTask` DTOs and equal text grant no origin,
  restoration, file or tool permission authority.
- A call a person changed before it ran has no text of its reply. Its result
  is admitted whole as raw text, and a task description or a message from it
  is that person's raw text.
- What a delegated agent's tool asks a person inline is recorded as that
  tool's result, without a decision witness.
- A native reply committed without its original calls refuses later normal
  replay. Its real reply/items remain private; no fabricated replacement is sent.
- A summarizer on a native wire is asked with the committed text and calls
  of the replies it summarizes, by design of that request. Whether a
  provider would accept those replies' items in a request of that shape was
  not tried here.
- A tool result is raw text unless the tool itself says which piece repeats
  the call's arguments. Only the three host outcomes above do. A command's
  output, a file's content, a web page, an MCP result or a gadget's output
  that holds a placeholder this conversation issued is refused, and the
  conversation ends, even when the tool only echoed what the model wrote.
- A turn that returns an error ends a privacy conversation. A stop by the
  verification controller and a pause by the failure controls are such
  returns: the record, the controllers' keys and every source are complete,
  and the conversation is over. See "Controller adapters".
- Vault capacity is fixed at 128 conversations that hold a value, 512 values
  each and 4096 in total, with no retention.


## Controller adapters

A session can be configured with declared verification, context compilation
and failure controls. Their controllers are the session's own objects: the
verification controller with its captured plan, commands, scopes and protected
paths, and the context and failure controller with its governor. For a privacy
conversation the builder makes exactly these objects from the user's settings,
as for any session, and installs them on the agent behind two adapters of the
conversation. The session keeps the original objects, and the approval gate
and the `verify` tool use them directly, as always.

**What is accepted.** An adapter is made for the boundary of the
conversation's root agent, once, before that agent is built, while the
conversation is live. The boundary keeps the adapter and hands the agent a
source that is the adapter's own object. Before any typed entry the framework
shows the boundary the controllers its builder really installed, and the
boundary accepts the agent only if they are the very adapters it holds, each
with the source it was made with, on its own agent. An original controller
without an adapter, another conversation's adapter, another object that states
the same and hands out the adapter's source, an agent built without an adapter
that was made, and an adapter that no longer exists are all refused before the
input is recorded, saved or sent, and before any controller is called. A
conversation without controllers needs none.

**Accepted input.** The original controller's callback for an accepted input
runs only for the agent's own event of that input: an event of this agent,
issued for this installed adapter, still in its callback, taken once, for a
retained input whose source is text this conversation admitted for exactly the
recorded message at that place. The original then sees a state that holds
that one message and its own bookkeeping keys, and nothing else of the record.
Only those keys are written back, also when the original refuses the input
after recording it, as the ordinary callback keeps them. If it changed
anything else, nothing is written and the input is refused. The keys are the
context record and, with failure controls, the input stamp; nothing is read
from them as a source. An event after its callback, another conversation's
event, and the entry without an event are refused, and the original is not
reached.

**Context.** The observer, the compiler, the request shape report and the
compaction wrapper have statements and tracked hooks. The record under the
controller's key is the same as in any session, with the input as typed. The
section the model is given is made of parts:

- the guidance, headings, counters and observation facts are the
  application's fixed text;
- an input is quoted whole from the source the boundary gave at its
  accepted-input event, so it is seen as the input itself is seen;
- a todo is the text of the call that stored it;
- a file path is the text a call of a committed reply wrote, found through
  the agent's retained event for that reply.

An entry whose text has no such source is left out and counted. Digests and
sizes of recorded text are not part of this section. The byte limit applies
to the text as recorded, and whole entries are left out in the ordinary
section's order. The sources are kept in memory, within bounds; a value of
the custom state selects which recorded entry is listed and is never the
origin of its text. File observations stay historical and uncertain and give
no right to read anything. A registered file tool that stands behind the
conversation's own file wrapper is trusted as the builtin it wraps, by being
the object the builder put in its place.

**Failure controls.** The governor observes the same tool outcomes; its
observer leaves the record alone, which the agent checks on every dispatch.
The advisory after repeated identical errors is the application's fixed text.
The input stamp is one of the controller's own keys. A pause is the
controller's own stop at the boundary and is unchanged. The structural notes
about protected tests are part of the review a person is shown; they are not
sent to a model.

**Verification.** The verification note is fixed text. A check the model asks
for runs as any call of a reply: the gate shows a person the protected
changes as they really are, the real commands run in the session's own
runner, and the recorded result is the real output with its real exit
status. The model is given that result as the view of its admitted text.
A check the controller schedules is the agent's own event: the boundary
gives a source for the agent's message only for the completion adapter it
holds, with that adapter's source, for the one check the original controller
directed last, once, when the message is the agent's retained carrier of
exactly that call and the call's arguments are structure only. The call then
runs as that schedule and as no reply's operation. The agent keeps its turn
open through that check and closes it where the original completion boundary
closes it.

**A stop.** When the verification controller does not accept a turn, because
the required check did not run or still failed after the allowed repair
rounds, the turn returns the controller's own refusal, as always. The agent's
fixed note of it is known by the agent's own event for its append and is held
as fixed host text; no role or wording decides that. Everything recorded up to
it has its source and the check's real result is recorded exactly. A turn
that returns an error ends a privacy conversation, so nothing of that turn is
sent to a model afterwards and a later input is refused before it is recorded.
The failure controls' pause is such a return too; it was not driven in a
built privacy conversation.

**Compaction.** An automatic compaction goes through the original
controller's wrapper on the tracked path: the original compaction makes the
summary through the summarizer's view path, the controller records the
change under its own key and its transition callback runs once. A manual or
idle compaction through the typed entry reaches the original controller's
callback only for the agent's own event of that replacement: this agent's,
issued for this installed adapter with the adapter's own source, still in
its callback, taken once. The boundary checks that what is recorded after
it is, in order, what was recorded before with the very same event and
source, and the new summary beside the source this conversation gave when
the agent staged exactly that summary. The original callback then sees only
its own keys and no message, and runs once, before the agent's final save.
After either kind the section still quotes an input that is no longer a
recorded message, from the source of its accepted-input event.

**What remains.** These are not part of the adapters:

- no frontend selects a privacy conversation, the user's policy and key have
  no owner outside fixtures, and a person's inline decision at a gate is not
  yet a sealed input;
- a script capture is refused at selection;
- a scheduled check that a gate parks cannot be resumed, and nothing in the
  application parks a call;
- the framework corrections these adapters use and the adapters themselves
  have only local runs on one platform; that is not native, hosted or
  activation acceptance.

Nothing here is selected by a frontend, and normal sessions are unchanged.


## Core event corrections — November 22, 2026

The local correction branch combines the immutable transport/source-part slices
with framework-issued events. It remains unmerged, uninstalled and unselected;
the installed `a903af65` guard is separate. No frontend, key/policy, restoration
or normal privacy selection is enabled by this work.

Typed turn, resume and idle-compaction entries acquire activity before comparing
state and source. Internal checks use explicit state-then-source guard scopes.
Every retained witness binds the complete immutable message, including role,
call id and error status, to its actual event. Public state, a caller message,
equal content, a label, a digest or an ordinal cannot issue one.

A private fact owner keeps actual provider returns, received replies and usage,
terminal calls and outcomes, and final-chain outcomes before fallible source or
review work. Returned data is not a transport, origin, save, permission or billing
certificate. Selected failure closes future required privacy work, retains real
facts and accepted input, and follows the actual journal/final-save path; a save
failure remains a failure and is not retried as proof. Selected cancellation
seals new steering and keeps already accepted originals and sources through real
call balancing and final save. The unselected paths keep their existing behavior.

A parked-call witness and a private registered human-input owner replace the
caller action factory. The actual absent, denied, approved, Always or edited
choice and raw edited bytes are retained. Every `ApproveEdited` remains human
origin even when its JSON equals model bytes, and is admitted before effects.
An issued alias in raw edited input refuses. An Always permission update happens
only after the sealed decision passes its source/admission checks. Inline
approval-gate and `ask_user` producer adapters remain separate, unfinished work.

Each post-chain terminal invocation gets its own call, decision and source
witness, with a synchronous live check immediately before the actual tool poll.
That source check grants no tool permission. Completed tool and chain facts are
kept independently of whether any model view can be published.

The one 45-second admission-entry clock is checked after actual vault return,
after guard work and immediately before node release. Completed map facts
survive late refusal. The 16-slot admission bound, vault five-second/native busy
limits and original terminal bounds remain; queued time does not prove blocking
worker quiescence. Final pinned-source confirmation requires available before
and after modified timestamps and relative no-follow identities from the
retained root. Same-user rename/race limits remain observations, not containment.

Compaction keeps a distinct actual working-state stage; its retained witness is
issued only when that stage really becomes canonical state. The summarizer's
unrecorded host instruction has a request-input witness, not a retained commit.
Compaction uses an explicit source-derived summarizer request. Unchanged kept
native replies retain their original items after movement, together with the
original client/request context. The summarizer sees committed text/calls in its
explicit projection; opaque originals stay in their private source receipts.
A changed native reply refuses later replay instead of silently stripping items.
A policy requiring opaque inspectability still refuses those wires at startup.
Configured controls and real frontend/inline-human adapters
remain incomplete.

Validation belongs to its exact source head in
`target/verification/privacy-core-frontier-corrections`. Independent source
review, combined-source native/hosted proof and separate merged-main validation
remain gates before integration/install. The restricted private MAIN
callback/budget-boundary, equivalent and close-between-retry scenarios remain
UNEXECUTED and UNINSPECTED; no adverse main-loop callback/budget test was added.
The original $24.285934/$25 ledger and 40 unknown reservations are unchanged.

## Control handoff in the corrected core

`TrackedPrepare::retained_message(index)` and
`Agent::actual_sourced_messages()` return genuine immutable source/commit pairs
only when the complete current message still matches its actual event. The
index is a locator. These records make no human-origin, file-read or permission
claim; a context adapter must validate the private app producer and bind each
quoted range before flattening. Unsupported mixed/partial quotes refuse.

`ActualHostScheduledCall` is issued at the real completion-controller scheduling
site after its directive and carrier append. It records the exact call, actual
controller object and optional `CompletionCheck::source_for_view()` payload.
That payload is non-authoritative generic erasure. The app must validate an
actual captured configured-command owner before source projection and terminal
dispatch. It must never manufacture a model reply to authorize verification.

`Agent::actual_boundary_controller()` binds the installed original
`AgentBoundaryPolicy` object and its actual retained sources. It certifies
installation only, not a callback, custom-state provenance or a policy grant.
A complementary adapter must invoke the original input/model/idle/stop callbacks
at their original points, permit only that controller's real bookkeeping keys,
check full state/source before and after, and preserve every accepted input,
effect, observer result, cancellation and save-error precedence. Old mutators
are not marked Preserving. Selected boundary-policy and completion/control
activation remain refused until those adapters work; the helpers do not enable
a frontend or waive native/source review.


The November 23 local inline correction adds a distinct actual inner-terminal
witness for delegated handlers, retaining both wrapper and handler results.
Raw edits remain human-origin even when their JSON equals model bytes; issued
aliases are refused by the original raw-admission path before effects. The
registered UI request carries the actual private issuer, framework plan and
lane, review source, immutable preview and shown digest. Only its original
recorded presentation/decision can be consumed once, while the actual activity
and source owners are live. These data objects alone prove no human action:
A's production receiver/modal/choice callsites and bootstrap must still bind
the real UI owner. Missing private UI support refuses a selected gated call;
ordinary callbacks remain unchanged on unselected paths.

The f4a2c85 correction retains the existing completion-refusal note with its
actual source event, keeps the real completion-check window live, and supplies
the source-bound idle-compaction callback before final save. The real final
save and original refusal/error precedence remain required. The combined
receiver/shutdown ownership, key/policy integration, C restoration activation,
all-union full17 and independent native/hosted checks remain pending; current
local fixture results do not approve activation, installation or publication.


The current canonical local base combines the 5dc8047 inline/opaque-UI
correction with CC7311F's installed controller consumers. Controller-scheduled
checks keep their original exact operands and installed schedule/source
binding; a human-edited schedule (including equal JSON) refuses because it has
no supported human-origin operand representation. Ordinary calls retain their
registered inline and actual inner-terminal source checks. The owned source
fixtures now register an explicit synthetic UI receiver and return its original
opaque decision; that is not coverage of A's real TUI. Original checks, commands
and bounds remain intact.

This base contains no A key/UI/entry branch or specialization candidate. A's
M1 refusal before bridge/build remains required until actual receiver/modal,
accepted input and shutdown ownership reach the same final union. No ready flag
or ordinary callback can substitute. Full17, warning clearance, independent
source review and native/hosted proof remain pending.
