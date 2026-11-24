# Private terminal: voice input and the shell escape's directory

Status: local unmerged branch, not installed. This page covers one bounded
correction of the opt-in private terminal described in
[privacy-startup.md](privacy-startup.md): its owned voice controller and the
directory of its shell escape. The ordinary terminal, the ordinary voice
controller's defaults and the ordinary `!command` runner are unchanged.
Nothing here is hardware validation.

## Voice input

A build with voice input, in a private conversation whose captured settings
keep `voice.enabled`, owns the ordinary local voice controller.

- **Model cache under the captured root.** The controller is started with the
  configuration root this conversation captured, taken from its built session
  after a source check. Its model cache is that root's `models` directory.
  There is no fallback to the ambient configuration directory: without a
  captured root the start is refused, which is also what happens before the
  session is built. The root is a path that was just checked. It is not a
  held descriptor, later access is by that path, and nothing confines the
  controller to it.
- **What a start changes.** On a voice start the controller creates the
  `models` directory, empty, when it is missing. That is a local cache
  effect which removing the directory undoes; the start is therefore not
  read-only. With the captured `voice.allow_model_download` off it changes
  nothing else: a model file of the expected size is used as it is, anything
  else is the controller's own missing-model answer, no partial download is
  hashed, promoted or removed, and no connection is made. With the opt-in
  on, the controller's ordinary cache handling runs in that directory,
  including its download.
- **Restart.** After three failed sessions the controller answers every start
  with its own pause line, which names `/voice on`. No key asks for a
  restart, so the private terminal answers that one command, as the built-in
  one, only while the captured source holds and has voice input on. It turns
  nothing on and changes no setting; a conversation without voice input and
  every other `/voice` form keep the general refusal. The restart retires
  the controller as described next.
- **Retiring a controller.** A restart, the person's cancel of a dictation
  and the draft bound retire the owned controller in the same way and in
  this order. Its receiver is dropped. It is told to cancel, which stops its
  session and releases capture. Then it is shut down for real as local work
  of the frontend, which holds a token until the controller's own task has
  ended: the status reports that work until it has returned, a settings
  change waits for it, and a retirement of the conversation awaits it inside
  its 30-second bound. A voice key pressed before it has returned is refused
  with a fixed line, and nothing is started for the person afterwards. A
  line says when the controller has stopped. The next voice key starts a new
  controller, which loads the model again and counts failures anew. The
  ordinary terminal keeps its controller through a cancel; the private one
  does not.
- **Events come from the owned controller's own receiver.** The frontend
  holds each controller's event receiver and takes a batch from it on every
  pass of its loop; no task forwards them and they do not pass through the
  frontend's general event channel. An event is current because the owned
  controller sent it. When a controller is retired its receiver is dropped,
  and everything it had sent and the frontend had not taken is discarded
  with it.
- **A dictation ended here stays ended.** The frontend keeps its own mark,
  set by the person's cancel, by the draft bound, by a refused start and by
  a retirement or restart, and cleared only when a start is next acted on.
  No event changes it. While it is set, a partial or final segment and a
  session's end are dropped, and so is a state that would show the dictation
  as live: no text arrives and nothing is submitted with no key behind it.
  Each of these ends leaves no controller owned, and no controller is
  started before the retired one's task has ended. So the start that clears
  the mark has a new controller with a new receiver, and whatever the
  cancelled session still sent has nowhere to arrive. This rests on who owns
  the controller and its receiver, not on time. Finishing a dictation with
  the voice key, or with Enter, is not ending it here: its last segment and
  its end arrive as before, and the controller stays.
- **Draft bound through the end of a dictation.** A partial or final segment
  that could take the draft past the bound of typed and pasted input is
  refused whole and the dictation ends here; the earlier draft stays. The
  end of a live dictation can add one separator; at the bound it adds none,
  so dictation does not take a draft that it submits past the bound.

Limits that stay as they were: a frontend that is dropped, and not retired,
tells its controller to cancel without awaiting it; the controller's task
then lives until the runtime ends. A final model file is trusted by its
size. The error line that names the model file is built from the ambient
configuration directory; that is display only.

## Shell escape

`!command` runs in the workspace the conversation was captured for. The
frontend gives that directory to the ordinary executor, together with the
frontend's own cancellation turn; the process's current directory is not
used. The executor reads only the turn it is given: the same refusal of a
cancelled turn, process group, kill, drain and partial capture as before.
The conversation's source check, which includes the workspace's identity,
runs before the command and again before its output is shown; the directory
is then used by path. An empty command, the output format, the limit and
the process handling are the ordinary escape's. A line typed while other
work runs, and a refused line, are kept as typed.

## What was run, and what was not

Run through the real binary on a synthetic home, key and loopback model, in a
build with voice input:

- the paused controller and its restart: three voice keys end at the real
  controller's missing-model answer, the fourth gets its pause line,
  `/voice on` shuts it down and says so, the next key reaches a new
  controller, `/voice off` keeps the general refusal, and a complete-size
  partial download placed in the cache beforehand is still there, alone and
  at its size;
- the shell escape: a command sees a file created in the workspace under a
  relative name; a line typed while it runs waits and then runs; error
  output, exit code and the empty command are shown as the ordinary escape
  shows them; `/voice on` in a conversation with voice input off keeps the
  general refusal; one request and one recorded input remain;
- the two earlier voice cases and the earlier shell case, unchanged.

In those cases every voice event the terminal showed was taken from the
owned controller's own receiver, which is the only path there is, and the
restart ran the retirement of a controller for real. No case cancels a
dictation: on this host a session ends at its model check before a person
could cancel it. The cancel and the draft bound select that same retirement
and are not run.

Unit case in the core and gadget test modes: the executor in a directory
that is not the process's own, with the ordinary escape beside it and a
cancelled turn that spawns nothing.

Unit case in a build with voice input only, because the voice module exists
only there: the cache of an owner's root next to the ordinary cache, with
the real controller started on an empty root. That unit-test binary is
outside both full test modes and links past the unwind limit with the
linker's warning, so its passes are not clean evidence.

Source only, not exercised: every path that needs a dictated text, which is
the draft bound, the separator at the bound and everything the ended mark
drops; a cancelled dictation and the new controller after it; a start while
a retired controller is still stopping; the discarded events of a retired
controller; a start before the session is built; the download opt-in; a
workspace that differs from the process's directory in a running terminal.
In particular nothing shows, on a real device, a transcript that was queued
when its dictation was cancelled. No microphone, model, download or
transcription was used and no voice event was supplied by a test.

## Unwind data of the unit-test binaries

The unit-test binary of the gadget test mode must stay under 16,777,216
bytes of unwind data. These are the sizes of that section in the binary
Cargo selected, read from its section header.

On this branch, which stands on `670d750`:

| Source | Bytes | Under the limit | Linker warning |
|---|---|---|---|
| `670d750` | 16,775,932 | 1,284 | no |
| `2750805`: the executor's unit case in every mode, its forwarder forced inline | 16,776,272 | 944 | no |
| this branch: the forwarder an ordinary function | 16,776,388 | 828 | no |

The correction and its unit case take 456 bytes there. The core test mode's
unit-test binary holds 9,156,940 bytes.

Earlier, on `2e1b7d2` and then `4372ca1`, the same binary had 220 and 188
bytes to spare, and these were read while the correction was shaped:

| Source | Bytes | Under the limit | Linker warning |
|---|---|---|---|
| `2e1b7d2` | 16,776,996 | 220 | no |
| first form: a formatted sibling inside an entered turn scope, and a unit case | 16,779,208 | 1,992 over | yes |
| the same without the unit case | 16,778,864 | 1,648 over | yes |
| the turn as an argument, no entered scope | 16,777,340 | 124 over | yes |
| a plain match in place of one closure | 16,777,244 | 28 over | no |
| the executor's own future, formatted by its caller | 16,777,096 | 120 | no |
| the forwarder inlined (`4372ca1`) | 16,777,028 | 188 | no |
| `4372ca1` with the unit case in every mode, its results taken by `let`-`else` | 16,777,360 | 144 over | yes |
| the same case driven by a hand-written poll loop | 16,777,476 | 260 over | yes |
| the same case with both outcomes of each result matched | 16,777,344 | 128 over | yes |
| that case, and the event handler's session lookup as a plain match | 16,777,280 | 64 over | yes |
| that case, and all three session lookups as plain matches | 16,777,068 | 148 | no |
| two session lookups as plain matches, no unit case | 16,776,848 | 368 | no |
| those two lookups and the unit case in every mode | 16,777,160 | 56 | no |

Two things were read off these. Entering a turn scope for the shell escape
cost 1,524 bytes there; the executor is given the turn, which reaches the
same executor code. And one more unit case cost more than 300 bytes, which
that binary did not have at `4372ca1`: the case fitted there only with
session lookups of the frontend rewritten as plain matches. That rewrite is
not on this branch: the base gained room and the three lookups are as they
were. The executor's forwarder was forced inline for the same reason and is
an ordinary function again. What remains from that time is that the
frontend formats the executor's result itself, with the ordinary escape's
two formatters.

The voice-enabled unit-test binary held 17,267,944 bytes at `4372ca1`,
490,728 past the limit; it was not linked again.

## Open

- Cancelling one segment while speaking is not an end here and sets no
  mark; the controller stays. As in the ordinary terminal, a partial of that
  segment which its session had already sent can still be inserted, within
  the bound.
- A cancel costs the private terminal its controller: the next start loads
  the model again, and failures before the cancel no longer count towards
  the pause.
- Linux, hosted runs, a microphone and a real model remain unvalidated.
