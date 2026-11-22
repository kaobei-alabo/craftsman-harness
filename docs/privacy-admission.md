# Private detector contract and admission core

Status, November 22, 2026: a private core exists that can ask one approved,
source-pinned detector for spans, check them, commit them in an open vault,
admit them to the send guard and only then publish an admitted node with a
model view. Nothing selects it. There is no configuration key, command,
frontend option, key source, provider, tool, approval, display or file path
that uses it, and normal sessions behave exactly as before. Privacy remains
inactive.

This is one slice of the work described in [privacy hooks](privacy-hooks.md).
It builds on the [private vault](privacy-vault.md) and the
[final-send guard](privacy-send.md). The way admitted nodes are meant to reach
a model request is described in [privacy provenance](privacy-provenance.md).
It is local, unmerged and not installed.

## What exists and what does not

| Piece | State |
| --- | --- |
| Detector request and reply checks, with a host floor (`privacy_hook`) | implemented, private |
| Source-pinned detector call on the gadget engine | implemented, private |
| Admission owner for one conversation (`privacy_session`) | implemented, private |
| Session builder use of the reserved thread id | implemented, private; selected by nothing, and buildable only in test fixtures until every part has a source path |
| Producers that create raw captures | typed session entries, tool results, the agent's own messages, system and tool text: implemented, private. Files, memory and configuration parts: not implemented |
| Conversation boundary, model-origin nodes and request composition for all four wires | implemented, private; see provenance |
| Mixed text from the system prompt, todos, memory, reminders and task workflow notes, as parts | implemented, private; see provenance |
| Mixed text from compaction, delegated agents and context controls | not implemented |
| Display and file restoration | not implemented |
| Key envelope, unlock and captured user policy | a private backend exists on a separate local branch and is not part of this one. The builder selection takes the user's captured policy through the `CapturedUserPolicy` trait. Nothing in this branch implements that trait outside tests, so no selection can be made here |
| Secret input, frontend selector, activation | not implemented |

## Detector contract

The host sends one document to one detector function and reads one document
back. The contract is the one the source-built detector fixture already uses.

**Request.** Version 1, a request binding, and up to 64 text nodes, each with an
id, its text and its own binding. A binding is the SHA-256 of a fixed label,
the id and the text; the request binding covers every node binding in order.
One text is at most 256 KiB and each node at most 64 KiB. Text is split only
after a line feed, so no supported value is cut. A line longer than 64 KiB is
refused instead of split.

**Reply.** Version 1, the same request binding and up to 128 spans. Each span
names a node, repeats that node's binding, and gives a byte range and a
category. The reply is at most 64 KiB.

**Checks.** The host refuses the whole reply when any of these holds:

- it is larger than 64 KiB, is not exactly the expected document, or has an
  extra field;
- its version or request binding differs;
- it has more than 128 spans;
- a span names an unknown node or a wrong node binding;
- a range is empty, reversed, out of bounds or splits a UTF-8 scalar;
- spans are out of order or overlap;
- a category is not `email` or `phone`;
- the span's text is not one complete value of that category under the vault's
  own grammar;
- a match of the host floor lies outside every reported span.

A reply is data. A guest's success text, logs, timings and diagnostics are not
read as authority, and no category other than email and telephone can create a
mapping.

**Host floor.** The host has its own fixed rules for email addresses and for
telephone numbers in a few layouts. They are the reviewed rules of the detector
fixture. The host uses them only to refuse a reply that omits one of their
matches. They are not a second detector and not a coverage claim: names,
addresses, other identifiers, encoded values and secrets are outside them.

## Source-pinned call

A host selects a detector by gadget id, function name and the SHA-256 of its
component and manifest files. That selection is not an approval. The call runs
on the existing gadget engine and refuses unless all of these hold, in this
order, before any compile, `describe` or guest call:

1. the selection is well formed;
2. no project gadget has the same id;
3. the user configuration root is absolute and it and each of its ancestors is
   a real directory, not a link;
4. `gadgets` and the gadget's own directory are each opened by handle from that
   root without following a link;
5. both files are read through that one directory handle, and each is a regular
   file with a single link, within its size bound, and unchanged in identity,
   size and modification time across the read;
6. after the reads, the root and the directory reached by path are still the
   objects held by handle;
7. the bytes just read hash to the selected component and manifest values;
8. the manifest declares no read, write or HTTP capability;
9. a stored grant exists for these exact component bytes at this source.

Then the same bytes that were hashed are compiled, described and run with no
capability. The source is read and checked again on every call, so a rebuilt or
replaced detector is refused even after an earlier call compiled it. No path
is resolved through a link on the way to the files: a `gadgets` entry or a
gadget directory that is a link is refused even when it leads to an approved
source. The ancestor checks in steps 3 and 6 are observations before and after
the read, not a guarantee against another process of the same user renaming a
directory at that moment.

The call returns the guest's reply byte for byte. It is not parsed and encoded
again first, so the size bound and the strict checks above apply to exactly
what the guest wrote, including a repeated member or padding.

Ordinary gadget approval is unchanged. Three things that satisfy it do not
satisfy this path: a grant of an earlier build at the same path, an allow-list
entry, and workspace trust. This path never prompts. A gadget, a project, a
model or a reply cannot make a detector selected or approved.

The pinned detector owns its own gadget runtime with the engine's default call
bounds (30 seconds, 256 MiB) and no persistent native-code cache.

The selection object is built by host code. Until the trusted user policy
exists, nothing builds one outside tests.

## Captures and nodes

Three things are kept apart.

- **A raw capture** is one actual source: what a producer really took in, with
  the kind of producer it was. It is immutable and carries no authority. A
  terminal draft is the only kind that has not been accepted yet; an accepted
  answer, a returned tool result and host text are accepted facts.
- **An admitted node** exists only as the result of a successful admission of
  one particular capture. It keeps that capture, the vault's own committed
  result and the model view. There is no way to build one from a string, a
  digest, a label, a flag or a position.
- **The canonical record** of what was typed, returned or received stays with
  its existing owner and is not changed by admission. The model view is a
  separate projection.

Equal bytes do not transfer authority. The view of an existing node can be used
again; the same bytes arriving as a new capture are raw and are admitted or
refused on their own. That includes a telephone number that happens to equal an
issued telephone placeholder.

## Conversation binding

`FreshConversationBinding::reserve(workspace)` takes the fresh thread id from
the application's own allocator and the SHA-256 of the canonical workspace
path. It is reserved before any privacy or client object exists, cannot be
built from a caller's string and cannot be cloned. The privacy session opens
its vault scope from it and exposes the same thread id for the session builder
of that conversation. Main, delegated and compaction lanes share the one scope.

The shared session builder uses that id for the agent and the checkpoint store
when it is given the private selection for the conversation, and a session
built that way closes the privacy owner instead of rebinding its thread. See
[privacy provenance](privacy-provenance.md). Nothing makes that selection.

A new conversation is a new scope and counts against the vault's fixed capacity
of 128 scopes that hold a value, 512 values per scope and 4096 values in total.
Nothing is pruned, reused or raised.

## Admission

One admission of one capture runs alone, in this order:

1. Take one of 16 places in the owner, or be turned away at once. One clock of
   45 seconds starts at this entry; waiting spends it and nothing restarts it.
2. Refuse text over 256 KiB, and text containing a placeholder already issued
   in the conversation. Both are decided before the detector runs.
3. Ask the detector and apply the reply checks above.
4. Refuse, still before any vault effect: a value reported under two
   categories, a reported or already known value that occurs somewhere
   unreported, a view that could exceed 256 KiB, and a scope that would exceed
   512 values.
5. Run the vault's own mask and commit, also when nothing was reported.
6. Admit the returned records to the send guard.
7. Run the guard's own check over the view.
8. Publish the node.

The owner's lock is held only around this work. It is never held across a
model request, a tool or a child task.

**Outcomes besides success:**

| Outcome | When | Session |
| --- | --- | --- |
| Refused | steps 1, 2 and 4, for a draft only | continues; the draft is unchanged |
| Stopped | the same conditions for an accepted fact | ended; the fact is unchanged |
| Stopped | any detector failure, any vault or guard refusal, the clock running out after the detector started | ended |

Every detector failure is a stop: no result, an unapproved or changed source,
a refused reply, an omission caught by the floor. A vault refusal is never
relabelled as a content refusal. A stopped session closes its guard and its
scope: every later admission, read of a view and send through that guard
refuses. Stopping does not undo an earlier send or effect and changes no
recorded input or result. No outcome returns raw text as a view.

**Placeholders in raw text.** Raw text that contains a placeholder issued in the
conversation is ambiguous and is refused. It is not restored and masked again,
and no channel, tool name, flag, digest or spelling is treated as proof that
text was already admitted.

**Records.** The owner keeps the vault's returned records of each successful
commit, at most one holder per mapping. It does not read the whole scope from
the vault. A node gives access to the records of that node only.

## Interface

```rust
// crate::privacy_hook
DetectorPin::supplied_by_host(gadget, function, component_sha256, manifest_sha256)
PinnedDetector::new(workspace, config_root, pin)
Detector::pinned(pinned_detector)

// crate::privacy_session
FreshConversationBinding::reserve(workspace)      // .thread_id()
PrivacySession::open(&vault, binding, detector).await
CapturedRaw::from_producer(origin, text)          // at the real producer only
session.admit_raw(&raw).await                     -> Result<AdmittedNode, AdmitError>
node.model_view() / node.admits(&raw) / node.with_records(|scope, records| ...)
session.thread_id() / session.send_guard() / session.check() / session.stop()
session.source(&node)                             -> Result<SourceRef, SessionStop>
session.boundary(&system_nodes)                   -> Arc<ConversationBoundary>   // once per conversation
session.model_view(&tracked_request)              -> Result<ModelRequest, SessionStop>
session.admitted(&source) / session.committed_model(&source) / session.actual_result(&source)
session.human_decision(&call, &action)            -> Result<SourceRef, SessionStop>
boundary.operation_in_flight()                    -> Option<Arc<ApprovedOperation>>
operation.call() / .model() / .edited_by_human() / .fields()
committed_model.message() / .fields()
field.place() / .range() / .with_record(|scope, records, index| ...)
trait CapturedUserPolicy { enables_privacy_in(workspace_sha256); requires_inspectable_provider_items() }
PrivacyViewSelection::from_user_policy(policy, session)   -> Result<Self, BuildRefusal>   // SessionExtras.privacy_view
selection.report().refusal()                      -> Option<BuildRefusal>   // why the builder refused
session.workspace_sha256()
view.turn(&agent, typed) / view.steer(&agent, typed) / view.decide(&agent, &shown, action, deny_default)

// crate::privacy_session, the only producers of a reply's receipt
ViewChatProvider::new(chat_client, session, usage, sink).await
ViewNativeProvider::new(native_client, session, usage, sink).await
session.chat_view(&tracked) / session.native_view(&tracked, identity)
committed_model.received_message() / .edits() / .request_sha256() / .has_provider_items()

// crate::privacy_vault, additive
Category::admits(value)
```

The pinned detector needs the `gadgets` feature. The contract and the admission
owner compile without it but cannot be constructed there outside tests.

## Evidence

- Contract: five cases over bindings, splitting, valid spans, every refusal and
  the host floor.
- Admission: ten cases over an actual vault and guard with fixed host reply
  rules.
- Pinned call: one marked native fixture builds the detector from source and
  runs it through 18 source, approval, capability and shadowing refusals, 19
  malformed, omitting or failing guest results, and admission through the
  actual detector. Two more cases check that an unmarked start does nothing.

These are synthetic, local tests. No model service was contacted and no paid
call was made. Runs are recorded with their own head under
`target/verification/`.

## Known gaps

- Only tests drive a conversation through this core to a model request. No
  frontend builds a session with it, and nothing reaches a display or a file
  through it.
- The system prompt is admitted section by section. The global and project
  instruction files are one section together.
- `CapturedRaw::from_producer` can be called by any code in the binary. The
  origin it records describes the producer; it is not authority.
- Detection is the detector's plus the narrow floor. Everything else is not
  detected.
- Declared identifiers are not wired; admission passes none.
- The conflicting-category refusal cannot occur with only email and telephone
  and is exercised by no test.
- Preflight uses the vault's default ceilings.
- Another user of the same vault at the same moment can make the vault report
  busy, which stops the session.
- A started vault operation is not interrupted by the admission clock; the
  vault's own 5-second bound applies to it.
- No Linux, Windows or hosted run covers this slice yet.
