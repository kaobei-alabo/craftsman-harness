# Private final-send guard

Status, November 22, 2026: a private, default-absent refusal seam exists at the
two places where a model request actually leaves the app. Nothing selects it:
there is no configuration key, command, frontend option, key source, detector
run, masking, restoration or hook registration. Normal sessions behave exactly
as before, and privacy remains inactive.

This is one slice of the work described in [privacy hooks](privacy-hooks.md).
It builds on the [private vault](privacy-vault.md). It is a separate candidate
and is not part of PR #32.

## What it does

When host code explicitly binds a guard to a session, every model client of
that session hands the network only request bytes the guard has sealed, and
asks the guard again before each send attempt. The guard refuses an attempt
whose final bytes contain an original value that real vault mapping records
already hold.

A refusal is about one attempt or one operation. The refused attempt is not
handed to the HTTP client. Anything that happened before it stays as it is: an
earlier attempt of the same request, earlier requests in the same turn, tool
effects, and usage already recorded.

## What it is not

- It is not a detector. It finds only originals that are already in admitted
  mapping records. Personal data the vault has never seen passes through.
- It does not mask, restore or change any request, reply, tool call or file.
- It does not prove that privacy integration is complete or safe to enable.
  Key ownership, trusted policy, the hook runner, the masking adapter, display
  and file restoration, retention and activation are separate, later slices.
- A refusal is not evidence that nothing was sent earlier, that a provider has
  stopped processing or billing, or that a socket or the operating system is
  idle.

## Selection and provenance

`SendGuard::from_vault` is the only constructor. It takes an actual live
`MappingSession` and `ResolutionRecords` that the same session produced. Further
records are added with `admit`. Originals are read through the vault's existing
checked host-field view. Each is checked in place and copied into a zeroized
store inside the guard only if it has a supported size and shape.

Caller strings, hashes, status flags and records of another session cannot mint
or feed a guard. Records of another session are refused, and so is a seal made
by another guard, even one built over the same session and records.

A failed admission closes the guard. An original the guard cannot represent is
therefore never dropped silently while requests keep flowing.

`SessionExtras.send_guard` carries the selection into the shared session
builder. It is absent by default and no frontend sets it.

## Where it checks

| Send site | Check |
| --- | --- |
| Chat Completions, `LlmClient::stream_body` | The body is sealed once. The guard decides before every attempt, including the existing single retry, and again after the bytes are copied into the request. |
| Responses, Claude Messages and Gemini, `NativeClient::complete` | The encoded body is sealed and decided immediately before the one send. |

A bound client refuses an unsealed body and a body sealed by another guard. An
unbound client never accepts a sealed body.

The main, delegated and compaction clients of a session all come from one
closure in the builder. When a guard is selected, that closure binds the same
guard to each of them. Each client is moved into its provider at once and is
not mutated again. The native transport is a crate-visible field of the client;
replacing a bound client or its transport later would need the same binding and
a new review.

Provider-owned native history is not altered. Signed and opaque items are sent
exactly as recorded.

## Supported representations

The guard refuses when an admitted original appears in any of these forms:

1. **In the final bytes.** A contiguous run anywhere in the encoded body:
   string values, object keys, numbers and tool definitions.
2. **In the decoded structure.** Inside any decoded JSON string of the body,
   under any standard JSON escaping. Strings that are themselves complete JSON
   documents are decoded too, down to four documents below the body. This
   covers tool-call arguments that the Chat and Responses wires carry as a JSON
   string, including escapes such as `\u0040` for `@` that a provider may have
   written.
3. **In the request's own text nodes** (native transports). The system prompt
   and each message's text are checked before encoding. This catches a visible
   answer that a provider's replayed item splits across several parts.

Admitted originals must be 4 to 1024 bytes and contain no double quote,
backslash or control character. A JSON string encoder never has to escape such
a value. An encoder that escapes only what it must (the app's own encoder does)
therefore leaves it literal at every nesting depth, which form 1 sees. Form 2
adds encoders that escape more than they must. An original outside this shape
is refused at admission rather than checked incompletely. Supported email and
telephone formats always fit; a declared identifier containing a quote or
backslash does not.

The guard refuses, rather than passes, a body it cannot check:

| Condition | Refusal |
| --- | --- |
| Body is not exactly one JSON value, or is not UTF-8 | unsupported |
| Body larger than 4 MiB | bounds |
| Request text nodes supplied with the body larger than 4 MiB, counting one byte per node | bounds |
| More than 96 open JSON containers, summed across embedded documents | bounds |
| A fifth embedded JSON document below the body | bounds |
| More than 128 MiB of counted work for one verification (see below) | bounds |
| More than 512 originals, or 256 KiB of originals in total | bounds, and the guard closes |

## What the work ceiling counts

The 128 MiB ceiling is a count of specific operations. Each unit is charged
before the work it stands for, and a refused charge ends the verification.

| Charged | Units |
| --- | --- |
| Literal pass over the body, each supplied text node, and each decoded string or key | one per byte, for the two-byte filter probe at that position |
| Each four-byte prefix comparison in the candidate search: every binary-search step, and every equality test that accepts a candidate or ends the run | four |
| Comparing one candidate original with the text | the original's length |
| UTF-8 validation of the body | one per byte |
| Each JSON parse, of the body and of each embedded candidate document | one per input byte |
| Skipping the leading whitespace of a decoded string | one per byte examined |
| Each supplied text node | one |

It is not a measure of CPU time. It does not count constant bookkeeping per
charged step, allocator work, the single copy of the supplied text nodes into a
seal, or liveness probes; those are bounded by the 4 MiB ceilings, not by this
count.

Admission is separate and is never charged to it. Admission is bounded by the
originals ceilings alone: at most 512 records are read; each value is checked
in place, and its length is reserved against the 256 KiB total, before it is
copied, so at most 256 KiB of new values is copied; the existing set, itself
at most 256 KiB, is copied once to build the union; and at most 1024 values
are sorted.

All of these ceilings are conservative refusal policies. They can refuse a
real request, including one that an earlier decision allowed: a later
verification of a seal checks its retained text as one joined string, which
can cost slightly more counted work than the first verification did.

## What is not detected

- A value changed by anything other than JSON string escaping: percent or
  base64 encoding, HTML entities, case or Unicode normalization, inserted
  spaces or zero-width characters, truncation.
- A value split across separate strings, except the native visible-text case in
  form 3. In particular, text a provider splits across separate thought or
  reasoning parts is not rejoined.
- Anything inside a provider's encrypted or signed payload.
- A JSON fragment with extra escaping that is not a complete document.

Form 1 can also refuse a body in which adjacent structural bytes happen to
spell an original, for example two neighbouring numbers. That is a deliberate
false refusal, never a false pass.

## Decisions, admissions and their limits

A seal keeps the exact bytes, the request text nodes it was verified with, and
which admitted set it was verified against. A later decision uses the seal's
own retained text; a caller cannot supply different text for it.

Every decision — sealing, the check before an attempt, and the builder's
liveness check — reads the guard's admitted set, its generation and its closed
flag under one lock. Admission and close take the same lock exclusively. So:

- a decision is made against exactly one admitted set;
- an admission or a close that arrives during a decision waits, and takes
  effect for decisions made after it returns;
- if the admitted set has changed since a seal was verified, the next decision
  verifies it again against the current set and passes only if it is still
  clean;
- an admission decides its next generation before it commits anything, so a
  refused admission leaves the set and its generation unchanged, and closes the
  guard.

No operation awaits while holding the lock.

A decision covers the instant it is made, not what follows. Between a decision
and the bytes reaching the network there is a window the guard does not close.
An admission or a close after the decision does not retract an attempt that
was already allowed and cannot stop network work that has already started. The
vault session's own liveness is sampled at the decision, not locked.

An ordinary refusal does not close the guard; later clean attempts are still
allowed.

## Refusals

All refusals are fixed text. Each names the refused operation or attempt and
nothing else. They contain no body, value, alias, path, credential or provider
text, and the guard and seal types print only a fixed placeholder. Nothing is
logged.

| Refusal | Meaning |
| --- | --- |
| closed | The guard, its mapping session or its vault is closed |
| known original | This attempt's request contains an admitted original |
| bounds | A size, depth or work ceiling was exceeded |
| unsupported | The body or an original has a shape the guard cannot check |
| unsealed | A bound client was handed a body the guard did not seal |
| foreign | The seal or records belong to another guard or session |

A refused attempt returns no response and adds no usage of its own. It does
not undo, hide or reset earlier attempts, earlier requests of the same turn,
tool effects or usage already recorded, and it says nothing about what a
provider has received or billed.

## Fresh conversations only

Reopen and model transfer are not implemented for a bound conversation. They
are refused before any state, hook or checkpoint is touched:

- the builder refuses a selection together with an explicit thread id, a closed
  guard, or the private prepared (headless) construction, before configuration,
  stores, providers or discovery;
- `Session::load_thread` refuses on a bound session;
- `Session::inherit_conversation` refuses when either session is bound.

Checkpoint bytes, native history and `threads.db` are unchanged.

`Session::set_thread_id` is a gap. It returns nothing, so it cannot refuse, and
its behaviour is left as it was: on a bound session it still rebinds the agent,
checkpoint, verification and control state. The clients stay guarded, so this
is not a way to send unchecked bytes, but it is not a supported continuation.
A bound conversation must not be switched to another thread until a
continuation route is designed and reviewed.

## Unchanged without a selection

With no guard selected, the Chat and native clients run their original code
paths: the same serialization, headers, retry, response handling, usage
accounting, cancellation, native history and profile handling. The guard is not
middleware, so a harness profile cannot exclude it once selected.

## Evidence

Local evidence on macOS, in core feature mode and gadget feature mode. The
first candidate's records are in `target/verification/privacy-send-fable52/`;
the corrections' are in `target/verification/privacy-send-fable52-correction/`
and `target/verification/privacy-send-fable52-copy-bound/`.

- 17 ordinary cases use an actual vault with host-supplied detector reports and
  loopback HTTP. They cover exact sealing, each supported form, each refusal
  and its fixed wording, foreign records and seals, re-verification after a
  sequential admission, the seal's retained request text, an exact count of the
  work a verification charges, an unchanged set after a refused admission,
  closed session, vault and guard, byte and header equality with an unbound
  client on all four wires, no request reaching the loopback server for a
  refused first attempt, the existing Chat retry, and a replayed answer split
  across provider parts on the three native wires.
- 7 cases use the actual shared builder. Five run in isolated child processes
  against loopback HTTP: one per wire, and one for the compaction and delegated
  clients. One checks the marker discipline of that fixture. One checks that
  the private prepared construction refuses a selection before any effect.

Nine uncommitted single-fault controls ran against the first candidate; each
removed one guard behaviour and made at least one case fail. They were not
repeated for the correction.

These are synthetic, sequential tests. No detector ran, no model service was
contacted and no paid call was made. Linux and Windows have not run these
cases, and the voice feature build was not exercised. Full local validation,
all four hosted contexts in both native modes, and independent review remain
delivery gates.

## Known gaps

- The locking and ordering described under "Decisions, admissions and their
  limits" are shown by the source only. No test admits originals, closes a
  guard or runs anything else concurrently with a decision or between two
  attempts of one request.
- That a refused charge stops the candidate search is shown by the source; the
  tests pin the counts, not a refusal at a chosen count.
- That an admission reserves each value's length against the total before it
  copies the value is shown by the source order; no test targets that ceiling.
- `Session::set_thread_id` on a bound session, as described above.
- Verification is synchronous work on the calling task, and an admission or a
  close waits for verifications in flight.
- Each Chat attempt copies the sealed body, as the existing checked path does,
  and a native seal keeps one copy of the request text.
