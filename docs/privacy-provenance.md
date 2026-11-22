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
| Framework boundary: sources, typed turn, steering and approval entries, dispatch check, the agent's own messages, middleware statements | implemented, private; see below |
| Conversation boundary: sources, composition, result records, checked operations and fields | implemented, private; see below |
| Chat provider that sends the view through the existing client and guard | implemented, private; see below |
| Session builder selection with a limited supported set, and typed session entries | implemented, private, selected by nothing; see below |
| Native wires (Responses, Claude Messages, Gemini) with receipts | not implemented; such a client is refused |
| Parts kept apart inside the system text; memory, todos, context notes, reminders | not implemented; the middleware that add them are left out and named |
| Compaction, delegated agents, notifications and inbox with sources | not implemented; left out, or refused when they occur |
| Verification, context and failure controls, local gadgets | not implemented; a selection that asks for one is refused |
| Reopening, transferring or rebinding a privacy conversation | refused |
| Restoration into replies, files and tool arguments | not implemented; separate work |
| Key envelope, unlock, user policy, activation | not implemented; separate work |

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

**Middleware.** A middleware runs on a selected agent only if its author stated,
through `AgentMiddleware::view_support`, that its hooks never add, remove or
rewrite a recorded message, never add system text and do not need the request
text. The agent compares the recorded messages before and after every hook
phase and stops the turn if one changed. `around_model` is never called on a
selected agent; `around_model_with_view` is, with a request that can be read
and passed on but not changed. A middleware without the statement makes the
typed entries refuse before any effect.

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
gives no right to restore anything. A reply whose calls the agent removed
because the round ended without running them is committed in that form.

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

**Chat provider.** `ViewChatProvider` is the existing Chat client and request
encoding. It composes the view, encodes it with the functions the ordinary
provider uses, and sends it through a client bound to the conversation's send
guard, so the final bytes of every attempt still pass the guard under the
existing retry policy. It makes the receipt from the reply as received, before
the agent reviews or commits it, keeps the real usage and gives the frontend
the same round and status notifications. It refuses the ordinary `complete`
call, so recorded text cannot be sent through it, and it refuses a client that
speaks a native wire.

## Session builder selection

The shared session builder accepts one private selection,
`PrivacyViewSelection`, which only host code can make from a live
`PrivacySession`. It states that the host's policy permits that conversation
to run with the limited set of parts that have a source-preserving path. No
frontend, configuration key or command makes one, so no session is built this
way outside tests.

With the selection, and before configuration, stores or providers are touched:

- a caller's thread id, a separately selected guard, the prepared headless
  runtime and an owner that has already ended are refused;
- a request for verification, context controls, failure controls, local
  gadgets or script capture is refused, because those must not simply be left
  out.

Then the builder:

- uses the conversation's reserved thread id for the agent and the checkpoint
  store, and the conversation's own guard for every model client it makes;
- builds the provider as a `ViewChatProvider`, and refuses a configured native
  wire;
- captures the complete system text it built as one host capture, admits it
  and gives the agent no system text of its own;
- keeps the middleware whose authors made the statement above, which today are
  the approval gate and the result size limit, and names every other one to
  the frontend in one line. Nothing is left out silently.

The session keeps the view. `Session::set_thread_id` on such a session closes
the privacy owner and changes nothing else; reopening and transfer are refused
as they are for a guard-bound session. A new conversation is a new owner, a new
reserved id, a new scope and a new session.

**Typed entries.** `PrivacyView::turn`, `steer` and `decide` are the producers
of what a person types. Each captures the text as a draft and admits it before
it is accepted, so a draft that cannot be admitted is turned away unchanged and
the conversation continues. A reason typed with a denial is checked the same
way. Nothing calls these entries yet.

**What such a session does not have.** No todo list, memory text, system
reminders, compaction, repair of malformed tool calls, workspace checkpoints,
delegated agents or task workflow notes. A long conversation is not compacted
and will reach the provider's limit. The system text is one capture, not parts.
Reading the checkpoint back with an ordinary session returns the real record:
nothing in it is redacted or encrypted by this work.

## Limits that will remain

- A raw text that contains an issued placeholder is refused, including a
  telephone number that equals one.
- Provider items that are signed or opaque are kept byte-exact. If a policy
  needs to inspect one that cannot be inspected, that representation is
  refused; it is not stripped or rewritten.
- A model node is not an admitted node and gives no right to restore anything.
- Vault capacity is fixed at 128 conversations that hold a value, 512 values
  each and 4096 in total, with no retention.


## Core event correction branch — November 22, 2026

A separate local correction branch now issues immutable retained-message and
provider/terminal event witnesses at the actual framework event. They are
private-memory history, with no caller constructor or serialized checkpoint
certificate, and do not certify transport, saves, restoration or permission.
The typed activity guard precedes the state/source check. Selected cancellation
keeps already accepted inputs and sources through call balancing and final save.
The one admission-entry clock is checked again after real vault work and before
node release; the native vault bounds are unchanged and no worker quiescence is
claimed. Final pinned-source observations require available timestamps and
relative no-follow directory identities from the retained root.

This branch is still under implementation and has not had its full local run or
independent source review. Application consumption, actual human-input owners,
transport integration and full-set source adapters remain pending here. The
installed a903 guard is separate; no frontend, key/policy, restoration or normal
privacy selection is enabled. The restricted private MAIN callback/budget and
close-between-retry scenarios remain unexecuted and uninspected.
