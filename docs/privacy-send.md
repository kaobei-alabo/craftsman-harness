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
that session sends only request bytes the guard has sealed. The guard refuses a
request whose final bytes contain an original value that real vault mapping
records already hold. A refused request is not sent, and the turn ends with a
fixed message.

## What it is not

- It is not a detector. It finds only originals that are already in admitted
  mapping records. Personal data the vault has never seen passes through.
- It does not mask, restore or change any request, reply, tool call or file.
- It does not prove that privacy integration is complete or safe to enable.
  Key ownership, trusted policy, the hook runner, the masking adapter, display
  and file restoration, retention and activation are separate, later slices.

## Selection and provenance

`SendGuard::from_vault` is the only constructor. It takes an actual live
`MappingSession` and `ResolutionRecords` that the same session produced. Further
records are added with `admit`. Originals are read through the vault's existing
checked host-field view and copied into a zeroized store inside the guard.

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
| Chat Completions, `LlmClient::stream_body` | The body is sealed once. The guard is consulted before every attempt, including the existing single retry, and again after the bytes are copied into the request. |
| Responses, Claude Messages and Gemini, `NativeClient::complete` | The encoded body is sealed and checked immediately before the send. |

A bound client refuses an unsealed body and a body sealed by another guard. An
unbound client never accepts a sealed body.

The main, delegated and compaction clients of a session all come from one
closure in the builder. When a guard is selected, that closure binds the same
guard to each of them.

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
| More than 96 open JSON containers, summed across embedded documents | bounds |
| A fifth embedded JSON document below the body | bounds |
| More than 128 MiB of parsed, scanned and compared bytes for one check | bounds |
| More than 512 originals, or 256 KiB of originals in total | bounds, and the guard closes |

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

## Seals and later admissions

A seal records which admitted set it was verified against. If more originals
are admitted before an attempt, the seal is verified again against the current
set. It passes only if the body is still clean. An ordinary refusal does not
close the guard; later clean requests are still sent.

## Refusals

All refusals are fixed text. They contain no body, value, alias, path,
credential or provider text, and the guard and seal types print only a fixed
placeholder. Nothing is logged.

| Refusal | Meaning |
| --- | --- |
| closed | The guard, its mapping session or its vault is closed |
| known original | The request contains an admitted original |
| bounds | A size, depth or work ceiling was exceeded |
| unsupported | The body or an original has a shape the guard cannot check |
| unsealed | A bound client was handed a body the guard did not seal |
| foreign | The seal or records belong to another guard or session |

A refused request reports no usage and produces no answer.

## Fresh conversations only

Reopen and model transfer are not implemented for a bound conversation. They
are refused before any state, hook or checkpoint is touched:

- the builder refuses a selection together with an explicit thread id, a closed
  guard, or the private prepared (headless) construction, before configuration,
  stores, providers or discovery;
- `Session::load_thread` refuses on a bound session;
- `Session::inherit_conversation` refuses when either session is bound.

Checkpoint bytes, native history and `threads.db` are unchanged. Switching a
bound session to a new thread id is not refused in this slice.

## Unchanged without a selection

With no guard selected, the Chat and native clients run their original code
paths: the same serialization, headers, retry, response handling, usage
accounting, cancellation, native history and profile handling. The guard is not
middleware, so a harness profile cannot exclude it once selected.

## Evidence

Local evidence on macOS, core feature mode and gadget feature mode, at the
candidate commit recorded in `target/verification/privacy-send-fable52/`:

- 14 ordinary cases use an actual vault with host-supplied detector reports and
  loopback HTTP. They cover exact sealing, each supported form, each refusal,
  foreign records and seals, re-verification after admission, closed session,
  vault and guard, fixed text, byte and header equality with an unbound client
  on all four wires, zero sends for a known original, the existing Chat retry,
  and a replayed answer split across provider parts on the three native wires.
- 7 cases use the actual shared builder. Five run in isolated child processes
  against loopback HTTP: one per wire, and one for the compaction and delegated
  clients. One checks the marker discipline of that fixture. One checks that
  the private prepared construction refuses a selection before any effect.

Nine uncommitted single-fault controls each removed one guard behaviour and
made at least one of these cases fail; the source was restored after each.

These are synthetic tests. No detector ran, no model service was contacted and
no paid call was made. Linux and Windows have not run these cases, and the
voice feature build was not exercised. Full local validation, all four hosted
contexts in both native modes, and independent review remain delivery gates.

## Known gaps

- No test admits originals or closes a guard between two attempts of one
  request. Re-verification is covered at the guard, and the per-attempt call is
  visible in the source, but that interleaving is not exercised end to end.
- Verification is synchronous CPU work on the calling task, bounded by the work
  ceiling.
- Each Chat attempt copies the sealed body, as the existing checked path does.
