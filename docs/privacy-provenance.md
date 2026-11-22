# Privacy provenance: source receipts and a separate model view

Status, November 22, 2026: this page records the contract the private privacy
work follows for getting text to a model. Most of it is not built. Nothing
selects any of it, normal sessions are unchanged and privacy remains inactive.

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
| Delegated agents with `task`, notifications and inbox | not implemented; the middleware is installed in every ordinary session and has no source path, so the build is refused |
| Verification, context and failure controls, gadget catalog | not implemented; a selection that asks for one is refused |
| Sealed parked-call human decisions | private registered input producer; actual decision and raw edit retained, no caller action factory |
| Decisions inside the approval gate; `ask_user` answers as drafts | producer adapters not implemented |
| Terminal ingress, `/clear`, display of replies | not implemented |
| Reopening, transferring or rebinding a privacy conversation | refused |
| Restoration into replies, files and tool arguments | not implemented; separate work |
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
was posted without parts still has no source. Every other receiver of the
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
- a request for verification, context controls, failure controls, local
  gadgets or script capture.

Then it:

- uses the conversation's reserved thread id for the agent and the checkpoint
  store, and the conversation's own guard for every model client it makes;
- builds the provider for the configured wire, and refuses a native wire when
  the policy requires inspectable provider items;
- takes the sections of the system prompt from their producer while they are
  still apart, admits each as its own capture, and gives the agent no system
  text of its own. Joined, the sections must be exactly the text an ordinary
  session is given, or the build is refused;
- requires a statement from every middleware it would install. If one has
  none, the build is refused and the frontend is told which.

Today the statement exists for the approval gate, the result size limit, the
protected call repair, the todo list, memory, the todo reminders, compaction,
the task workflow notes, workspace checkpoints and the delegated agents. With
the ordinary configuration that is every installed part, so a selection made
from a policy builds the session with all of them: the same tools, the same
system text and the same delegated agents as an ordinary session. The builder
has no way to leave a part out. The earlier test-only selection that left
parts out and named them is gone.

The model-visible gadget runtime and its catalog still have no source path.
A frontend asks for them explicitly, and a selection that does is refused
before anything is built, like one that asks for verification, context or
failure controls. If a middleware without a statement were installed some
other way, for the root agent or for a delegated agent, the build would be
refused and would name it.

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

## Normal parts

How each normal component runs on a selected agent today.

| Component | Path |
| --- | --- |
| System prompt | sections from the producer: fixed text, the profile's text, instruction files, discovered skills, extensions and the frontend's addition, each admitted on its own. The global and project instruction files are one section, as the producer joins them |
| `patch_tool_calls` | tracked review; each repair recorded |
| Todo list | the section is fixed prompt, heading, status labels and line ends from the application, and each todo's text from the call that stored it. The tool states which arguments it stored and what its outcome repeats |
| Memory | the turn-start load is unchanged. The section is the application's tags, paths and guidance, with each file's content as one read part per load |
| Todo reminders | announces its round counters in the custom state and the fixed reminder it appends to a tool outcome |
| Task workflow notes | the two fixed notes as parts; its observation of outcomes stays in its own memory |
| Workspace checkpoints | reads recorded messages and writes its own store; changes nothing the agent records, which is checked |
| Approval gate, result size limit | leave the record alone, which is checked |
| Compaction | tracked: same trigger, split and record as the ordinary compaction; see "Compaction" |
| Delegated agents, notifications, inbox | tracked: a lane per agent, the description and messages as text of the call, results and notifications with the source of what the agent ended with; see "Delegated agents" |
| Verification, context and failure controls | **no path**; a selection that asks for one is refused |
| Gadget runtime and catalog | **no path**; a selection that asks for it is refused. The catalog records its updates as messages of its own and keeps a key in the custom state; it needs a tracked way to append a host message |
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
- Vault capacity is fixed at 128 conversations that hold a value, 512 values
  each and 4096 in total, with no retention.


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
Configured controls, runtime/catalog and real frontend/inline-human adapters
remain incomplete.

Validation belongs to its exact source head in
`target/verification/privacy-core-frontier-corrections`. Independent source
review, combined-source native/hosted proof and separate merged-main validation
remain gates before integration/install. The restricted private MAIN
callback/budget-boundary, equivalent and close-between-retry scenarios remain
UNEXECUTED and UNINSPECTED; no adverse main-loop callback/budget test was added.
The original $24.285934/$25 ledger and 40 unknown reservations are unchanged.
