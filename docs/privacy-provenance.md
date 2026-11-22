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
| Framework boundary for sources and views | first slice implemented, private; see below |
| Conversation boundary: sources for input, replies, tool results, system text and tool definitions | implemented, private; see below |
| Chat provider that sends the view through the existing client and guard | implemented, private; see below |
| Native wires (Responses, Claude Messages, Gemini) with receipts | not implemented; such a client is refused |
| Producers that keep parts: memory, todos, context, tasks, compaction, human replies | not implemented |
| Resumed, edited and denied approvals through the same checks | not implemented |
| Session builder use of the reserved conversation | not implemented |
| Restoration into replies and files | not implemented |

## Framework boundary, first slice

The framework crate has an optional boundary, `ModelViewBoundary`, selected
per agent with `AgentBuilder::model_view_boundary`. The default is none, and an
agent without it runs exactly its existing paths. The crate gained no
dependency and knows nothing about what a source is: a `SourceRef` is opaque
evidence that the selecting host supplies and checks itself.

With a boundary selected:

- A turn is accepted only through `Agent::run_turn_with_view(input, source)`.
  The input is recorded exactly as an ordinary turn records it; the source is
  kept beside it in memory and is never read from a checkpoint.
- Each request is a `TrackedModelRequest`: the recorded messages and tool
  definitions with one source per message and per definition, and the sources
  of the system channel from the boundary. The provider receives it through
  `ModelProvider::complete_with_view`. A provider that does not implement that
  method refuses; the recorded text is not sent in its place.
- A reply is committed, journaled and saved first, then reported to the
  boundary with the provider's receipt, and its source is recorded.
- A tool result is journaled and committed first, and saved there when the
  agent saves after each result, then reported to the boundary.
- If the boundary returns no source, the real reply or result stays recorded
  with its usage, the turn stops, and no later request can be composed for
  that conversation.

Refused on a selected agent, before any effect: the ordinary turn entry,
steering, resuming an approval, reopening a thread, transferring a
conversation and idle compaction. Refused because they have no
source-preserving path yet: any middleware, a boundary policy, a completion
check, system text set on the agent, per-round system additions, and any
recorded message that arrived without a source, such as a notification from
another agent or the results appended when a turn is cancelled.

## Conversation boundary and Chat provider

The application now has the other side of that boundary for one privacy
conversation, and one provider that uses it. Both are private: no session
builder, frontend or configuration constructs either.

**Sources.** `PrivacySession::boundary(system)` returns the boundary for the one
agent of a conversation. A source it hands to the framework is one of three
private objects:

| Source | Minted when | Sent as |
| --- | --- | --- |
| Admitted node | raw text was captured and admitted: typed input, a tool result, system text, a tool description | the node's view |
| Committed model node | this conversation's provider received a reply and the agent committed exactly that reply | the recorded reply, unchanged |
| Tool definition | a tool's description and schema text were captured and admitted | the description's view; the schema as recorded |

A committed model node is model-origin text. It is not an admitted node and
gives no right to restore anything.

**Composition.** `PrivacySession::model_view` turns a tracked request into the
request a model may see. Each recorded text is replaced by the view of its own
source. Roles, order, names, call ids, the model's own text and the arguments
of its tool calls are the recorded ones. A source is used only when it is this
session's own object and still fits the item beside it: the captured text
equals the recorded text, a reply equals the reply that was received. Anything
else stops the session before a request exists. That includes a source from
another conversation, a draft in the system channel, a recorded message that
was changed after its source was made, and a tool schema in which admission
found a value, because a schema cannot be changed without changing what the
tool accepts.

**Tool results.** The agent commits the returned result first. The boundary
then captures that content as raw text and admits it. If it cannot be admitted,
the session stops; the result stays recorded exactly as returned, and nothing
is put in its place.

**Chat provider.** `ViewChatProvider` is the existing Chat client and request
encoding. It composes the view, encodes it with the functions the ordinary
provider uses, and sends it through a client bound to the conversation's send
guard, so the final bytes of every attempt still pass the guard under the
existing retry policy. It makes the receipt from the reply as received, before
the agent reviews or commits it, and keeps the real usage. It refuses the
ordinary `complete` call, so recorded text cannot be sent through it, and it
refuses a client that speaks a native wire.

Still missing for a real session: the session builder does not select any of
this, and the selected agent still refuses every middleware and the other
entries listed above.

## Limits that will remain

- A raw text that contains an issued placeholder is refused, including a
  telephone number that equals one.
- Provider items that are signed or opaque are kept byte-exact. If a policy
  needs to inspect one that cannot be inspected, that representation is
  refused; it is not stripped or rewritten.
- A model node is not an admitted node and gives no right to restore anything.
- Vault capacity is fixed at 128 conversations that hold a value, 512 values
  each and 4096 in total, with no retention.
