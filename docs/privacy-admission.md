# Private detector contract and admission core

Status, November 22, 2026: a private core exists that can ask one approved,
source-pinned detector for spans, check them, save mappings in an open vault,
admit them to the send guard and only then release the admitted text. Nothing
selects it. There is no configuration key, command, frontend option, key
source, provider, tool, approval, display or file path that uses it, and normal
sessions behave exactly as before. Privacy remains inactive.

This is one slice of the work described in [privacy hooks](privacy-hooks.md).
It builds on the [private vault](privacy-vault.md) and the
[final-send guard](privacy-send.md). It is local, unmerged and not installed.

## What exists and what does not

| Piece | State |
| --- | --- |
| Detector request and reply checks (`privacy_hook`) | implemented, private |
| Source-pinned detector call on the gadget engine | implemented, private |
| Admission owner for one conversation (`privacy_session`) | implemented, private |
| Whole-scope authenticated record read in the vault | implemented, additive |
| Provenance for model-produced and mixed text | not implemented |
| Provider, tool, approval and system-prompt seams | not implemented |
| Display and file restoration | not implemented |
| Key envelope, unlock, user policy, activation | not implemented; separate work |

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
  own grammar.

A reply is data. A guest's success text, logs, timings and diagnostics are not
read as authority, and no category other than email and telephone can create a
mapping.

**Limit.** The host checks what a detector reports. It cannot see what an
approved detector leaves out. The only omission it catches is a value the
conversation's scope already knows (see admission below).

## Source-pinned call

A host selects a detector by gadget id, function name and the SHA-256 of its
component and manifest files. That selection is not an approval. The call runs
on the existing gadget engine and refuses unless all of these hold, in this
order, before any compile, `describe` or guest call:

1. the selection is well formed;
2. no project gadget has the same id;
3. the user-level gadget directory and both files are regular and unlinked;
4. the bytes just read hash to the selected component and manifest values;
5. the manifest declares no read, write or HTTP capability;
6. a stored grant exists for these exact component bytes at this source.

Then the same bytes that were hashed are compiled, described and run with no
capability. The source is read and checked again on every call, so a rebuilt or
replaced detector is refused even after an earlier call compiled it.

Ordinary gadget approval is unchanged. Three things that satisfy it do not
satisfy this path: a grant of an earlier build at the same path, an allow-list
entry, and workspace trust. This path never prompts. A gadget, a project, a
model or a reply cannot make a detector selected or approved.

The pinned detector owns its own gadget runtime with the engine's default call
bounds (30 seconds, 256 MiB) and no persistent native-code cache.

The selection object is built by host code. Until the trusted user policy
exists, nothing builds one outside tests; the policy will construct it from
the user's own configuration.

## Admission

One `PrivacySession` owns one conversation: a vault scope, the send guard
minted from that scope, and the selected detector. It is created from an
already open vault and opens no file itself.

**Scope.** Each conversation gets a fresh random identity and the SHA-256 of its
workspace path. Main, delegated and compaction lanes of that conversation share
the scope. A new conversation is a new scope and counts against the vault's
fixed capacity of 128 scopes, 512 values per scope and 4096 values in total.
Nothing is pruned, reused or raised; a full scope or vault is a refusal.

**One admission** of raw text runs alone, in this order:

1. Refuse text over 256 KiB, and text containing a placeholder already issued
   in this scope. Both are decided before the detector runs.
2. Ask the detector and apply the reply checks above.
3. Refuse, still before any vault effect: a value reported under two
   categories, a reported or already known value that occurs somewhere
   unreported, a result that could exceed 256 KiB, and a scope that would
   exceed 512 values.
4. Save the mappings in the vault.
5. Admit the saved records to the send guard.
6. Run the guard's own check over the text about to be released.
7. Release the admitted text.

Text with no reported value is released unchanged after steps 1 to 3 and 6.

**Three outcomes besides success**, which are not interchangeable:

| Outcome | Meaning | Session |
| --- | --- | --- |
| Refused | the text itself cannot be admitted; nothing was saved | continues |
| Detector | the approved detector gave no usable result for this text; nothing was saved | continues |
| Stopped | the detector source, the vault or the guard refused or failed | ended |

A stopped session closes its guard and its scope: every later admission, read
of admitted text and send through that guard refuses. A vault refusal after the
host's own checks is always a stop, never relabelled as a content refusal. No
outcome releases raw text.

**Placeholders in raw text.** Raw text that contains a placeholder issued in the
scope is ambiguous and is refused. It is not restored and masked again, and no
channel, tool name, flag, digest or spelling is treated as proof that text was
already admitted. Text the model produced, and text that mixes raw and admitted
parts, need provenance that does not exist yet.

**Admitted text** is an opaque record tied to the session that produced it. It
can be read only while that session is live.

## Interface for the other steps

```rust
// crate::privacy_hook
DetectorPin::supplied_by_host(gadget, function, component_sha256, manifest_sha256)
PinnedDetector::new(workspace, config_root, pin)
Detector::pinned(pinned_detector)

// crate::privacy_session
ConversationScope::fresh(workspace)
PrivacySession::open(&vault, scope, detector).await
session.admit_raw(text).await        -> Result<AdmittedText, AdmitError>
session.send_guard()                 -> Arc<SendGuard>
session.with_scope_records(|mapping_session, records| ...).await
session.check() / session.stop()

// crate::privacy_vault, additive
MappingSession::scope_records().await
Category::admits(value)
```

The pinned detector needs the `gadgets` feature. The contract and the admission
owner compile without it but cannot be constructed there outside tests.

## Evidence

- Contract: four cases over bindings, splitting, valid spans and every refusal.
- Admission: nine cases over an actual vault and guard with fixed host reply
  rules. They cover release only after save and guard admission, repeated
  values, issued placeholders, uncovered values, bounds, detector failures,
  vault and guard failures, separate conversations, serialized admissions and
  scope capacity.
- Pinned call: one marked native fixture builds the detector from source and
  runs it. It covers 18 source, approval, capability and shadowing refusals
  with zero compiles and zero descriptions, 18 malformed or failing guest
  results, and admission through the actual detector. Two more cases check
  that an unmarked start does nothing.

These are synthetic, local tests. No model service was contacted and no paid
call was made. Runs are recorded with their own head under
`target/verification/privacy-hook-admission-core/`.

## Known gaps

- Nothing reaches a model, a tool, a display or a file through this core.
- Detector coverage is the detector's. Names, addresses, other identifiers,
  encoded values and images are not detected.
- Declared identifiers are not wired; admission passes none.
- The conflicting-category refusal cannot occur with only email and telephone,
  because no text is both; it is exercised by no test.
- Preflight uses the vault's default ceilings. A vault opened with tighter
  limits stops the session where the host would otherwise refuse.
- Admissions are serialized by one in-process lock. Another user of the same
  vault at the same moment can still make the vault report busy, which stops
  the session.
- A detector call cannot be interrupted before its own time bound, and a
  started vault operation is not interrupted either.
- No Linux, Windows or hosted run covers this slice yet.
