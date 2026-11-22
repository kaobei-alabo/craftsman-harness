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
| Framework boundary: sources, typed turn, steering and approval entries, dispatch check, the agent's own messages, middleware statements, tracked review of a reply | implemented, private; see below |
| Conversation boundary: sources, composition, result records, checked operations and fields | implemented, private; see below |
| Providers for all four wires (Chat, Responses, Claude Messages, Gemini) through the existing clients and guard, with receipts | implemented, private; see below |
| Protected repair of tool calls (`patch_tool_calls`) as a recorded change of the received reply | implemented, private; see below |
| Session builder selection and typed session entries | implemented, private, selected by nothing. A session is built only when every part has a source path, which today is never the case outside test fixtures; see below |
| Parts kept apart inside the system text; memory, todos, context notes, reminders | not implemented; the middleware that add them have no source path, so the build is refused |
| Compaction, delegated agents, notifications and inbox with sources | not implemented; refused |
| Receipts for approval decisions taken inside the approval gate; `ask_user` answers as drafts | not implemented |
| Terminal ingress, `/clear`, display of replies | not implemented |
| Verification, context and failure controls, local gadgets | not implemented; a selection that asks for one is refused |
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

**Replies and results.** A reply is committed, journaled and saved first, then
reported to the boundary with the provider's receipt. A tool result is
journaled and committed first, and saved there when the agent saves after each
result, then reported with what produced it: every run of the tool itself with
the exact call and its own outcome and metadata, before any middleware saw it,
and what the dispatch chain finally returned. If the boundary returns no
source, the real reply or result stays recorded with its usage and the turn
stops.

**Before a call runs.** The boundary is asked immediately before every call:
before the dispatch chain for an ordinary call, and before the tool itself for
a call a person approved. It is shown exactly what will run. A refusal means
nothing ran; the reply that asked for the call stays recorded and no result is
made up for it.

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

Still refused on a selected agent because no source-preserving path exists: the
untyped turn, steering and resume entries, reopening a thread, transferring a
conversation, idle compaction, a boundary policy, a completion check, system
text set on the agent, per-round system additions and an inbox.

Durable reconciliation of calls whose completion was not recorded keeps every
slot beside its message.

## Conversation boundary and Chat provider

The application has the other side of that boundary for one privacy
conversation, and one provider that uses it.

**Sources.** `PrivacySession::boundary(system)` returns the boundary for the one
agent of a conversation. A source it hands to the framework is one of these
private objects:

| Source | Minted when | Sent as |
| --- | --- | --- |
| Admitted node | raw text was captured and admitted: typed input, system text | the node's view |
| Committed model node | this conversation's provider received a reply and the agent committed it | the recorded reply, unchanged |
| Tool definition | a tool's description and schema text were captured and admitted | the description's view; the schema as recorded |
| Tool result | the agent recorded a result; its content was captured and admitted | the content's view |
| The agent's own message | the agent appended it at a named event; its text was captured and admitted | the text's view |

A committed model node is model-origin text. It is not an admitted node and
gives no right to restore anything. It keeps the reply as it was received, the
reply as it was committed, and the recorded changes between them. What was
committed must be what was received with those changes applied in order, or
that reply with its calls removed because the round ended without running
them. Anything else stops the session.

A conversation has one agent. Its boundary is handed out once.

**Composition.** `PrivacySession::model_view` turns a tracked request into the
request a model may see. Each recorded text is replaced by the view of its own
source. Roles, order, names, call ids, the model's own text and the arguments
of its tool calls are the recorded ones. A source is used only when it is this
session's own object and still fits the item beside it: the captured text
equals the recorded text, a reply equals the reply that was committed, a result
belongs to that call. Anything else stops the session before a request exists.
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
because it carries their words. A notification from another agent has no
source of its own and stops the session.

**Checked operations.** Before a call runs, the boundary requires that the last
committed reply asked for exactly that call, or that a checked decision allowed
exactly it. Anything else stops the session before the call runs. For the call
it lets through it keeps an `ApprovedOperation`: the exact call, the committed
reply that asked for it, whether a person replaced the arguments, and the
fields of the arguments. A component at the tool can read it through
`ConversationBoundary::operation_in_flight`.

**Decisions.** `PrivacySession::human_decision(call, action)` is called by the
interface that showed the call to a person. A decision of another conversation,
for another call or for another action is turned away; the call stays parked
and the conversation continues.

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
`complete` call, so recorded text cannot be sent through them. Both make their
receipt from the reply as received, before the agent reviews or commits it,
and keep the real usage.

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
- A receipt is made at the moment a reply is received. It holds the client's
  identity (endpoint, model, protocol and credential, as one digest), the
  position of the reply, the SHA-256 of the exact request bytes that were sent,
  and the provider's items exactly as received, signed and opaque ones
  included. The items are validated against the reply with the native
  transport's own check and size bound before the receipt exists.
- Items are sent back only for the reply they were received with, at its
  position, by a client of the same identity, and only if that reply was
  committed as received. Then they are sent back exactly; nothing is stripped
  or reformatted. A reply with no receipt, or with another client's, stops the
  session before a request exists. A native reply that a review changed is
  refused with the native transport's own message, as on the ordinary path.
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
- captures the complete system text it built as one host capture, admits it
  and gives the agent no system text of its own;
- requires a statement from every middleware it would install. If one has
  none, the build is refused and the frontend is told which.

Today the statement exists for the approval gate, the result size limit and
the protected call repair. Todos, memory, compaction and system reminders are
installed in every ordinary session and have none. **So no privacy session can
be built outside tests yet**: a selection made from a real policy is refused
with the list of those parts. Test fixtures alone use a fixture policy and a
second constructor, both compiled only into test builds, that leave those
parts out and name them in one line. Neither is a user policy and neither
gives a production path.

The session keeps the view. `Session::set_thread_id` on such a session closes
the privacy owner and changes nothing else; reopening and transfer are refused
as they are for a guard-bound session. A new conversation is a new owner, a new
reserved id, a new scope and a new session.

**Typed entries.** `PrivacyView::turn`, `steer` and `decide` are the producers
of what a person types. Each captures the text as a draft and admits it before
it is accepted, so a draft that cannot be admitted is turned away unchanged and
the conversation continues. A reason typed with a denial is checked the same
way. Nothing calls these entries yet.

**In the fixtures.** A fixture session has no todo list, memory text, system
reminders, compaction, workspace checkpoints, delegated agents or task workflow
notes. The system text is one capture, not parts. Reading the checkpoint back
with an ordinary session returns the real record: nothing in it is redacted or
encrypted by this work.

## Limits that will remain

- A raw text that contains an issued placeholder is refused, including a
  telephone number that equals one.
- Provider items that are signed or opaque are kept exactly as received and
  sent back exactly. If a policy needs to inspect one that cannot be
  inspected, that wire is refused; nothing is stripped or rewritten.
- A model node is not an admitted node and gives no right to restore anything.
- Vault capacity is fixed at 128 conversations that hold a value, 512 values
  each and 4096 in total, with no retention.
