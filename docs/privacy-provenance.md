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
| Producers that keep parts: system prompt, tools, memory, todos, context, tasks, compaction, human replies | not implemented |
| Provider and native request composition from views, with receipts | not implemented |
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

No application provider implements the view path and no session selects the
boundary. The admission core does not implement the boundary yet.

## Limits that will remain

- A raw text that contains an issued placeholder is refused, including a
  telephone number that equals one.
- Provider items that are signed or opaque are kept byte-exact. If a policy
  needs to inspect one that cannot be inspected, that representation is
  refused; it is not stripped or rewritten.
- A model node is not an admitted node and gives no right to restore anything.
- Vault capacity is fixed at 128 conversations that hold a value, 512 values
  each and 4096 in total, with no retention.
