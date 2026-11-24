# Private terminal: voice input and the shell escape's directory

Status: local unmerged branch, not installed. This page covers one bounded
correction of the opt-in private terminal described in
[privacy-startup.md](privacy-startup.md): its owned voice controller and the
directory of its shell escape. The ordinary terminal, the ordinary voice
controller's defaults and the ordinary `!command` runner are unchanged.
Nothing here is hardware validation.

## Voice input

A build with voice input, in a private conversation whose captured settings
keep `voice.enabled`, owns the ordinary local voice controller. What changed:

- **Model cache under the captured root.** The controller is started with the
  configuration root this conversation captured, taken from its built session
  after a source check. Its model cache is that root's `models` directory and
  never the ambient configuration directory. The root is a path that was just
  checked, not a held descriptor: later access is by that path.
- **Nothing existing changes without the download opt-in.** With the captured
  `voice.allow_model_download` off, a model file of the expected size is used
  as it is, and anything else is the controller's own missing-model answer.
  No partial download is hashed, promoted or removed and no connection is
  made. One effect remains: the controller creates the empty `models`
  directory when it is missing. With the opt-in on, the controller's ordinary
  cache handling runs in that directory, including its download.
- **No start before the session is built.** Until then there is no captured
  root to give the controller; the voice key is refused with a fixed line.
- **Restart.** After three failed sessions the controller answers every start
  with its own pause line, which names `/voice on`. No key asks for a
  restart, so the private terminal answers that one command, as the built-in
  one, only while the captured source holds and has voice input on. It turns
  nothing on and changes no setting; a conversation without voice input and
  every other `/voice` form keep the general refusal. The restart ends the
  old controller's event task first, tells the controller to cancel, and
  shuts it down for real as local work of the frontend: the status reports
  it until it has returned, a settings change waits for it, and a retirement
  awaits it inside its 30-second bound. A voice key pressed before that work
  has returned is refused. Afterwards a key starts a new controller with a
  new failure count.
- **Events of a retired controller change nothing.** A voice event that
  arrives while the frontend owns no controller is dropped.
- **Draft bound through the end of a dictation.** A partial or final segment
  that could take the draft past the bound of typed and pasted input is
  refused whole and the dictation is cancelled. The end of a dictation can
  add one separator and can submit the draft with no key behind it: one that
  was already ended in the terminal, by the person or by the bound, adds and
  submits nothing, and a live one adds no separator to a draft at its bound.

Limits that stay as they were: a frontend that is dropped, and not retired,
tells its controller to cancel and aborts its event task without awaiting
either; the controller's task then lives until the runtime ends. A final
model file is trusted by its size. The error line that names the model file
is built from the ambient configuration directory; that is display only.

## Shell escape

`!command` runs in the workspace the conversation was captured for. The
frontend gives that directory to the ordinary executor, together with the
cancellation turn the command runs under; the process's current directory is
not used. The conversation's source check, which includes the workspace's
identity, runs before the command and again before its output is shown. An
empty command, the output format, the limit and the process handling are the
ordinary escape's. A line typed while other work runs, and a refused line,
are kept as typed.

The executor is given the turn as an argument and is not run inside an
entered turn scope. Both reach the same executor with the same turn. The
entered scope would be compiled once more for this call: measured, 1,524
bytes of unwind data in the unit-test binary of the gadget test mode, which
had 220 bytes of room. As written the correction takes 32.

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

Unit cases, in the voice-enabled unit-test binary only: the cache of an
owner's root next to the ordinary cache, with the real controller started on
an empty root; and the executor in a directory that is not the process's own.

Source only, not exercised: every path that needs a dictated text, which is
the draft bound, the separator at the bound and the dropped late end of a
dictation; a start while a retired controller is still stopping; a dropped
event of a retired controller; a start before the session is built; the
download opt-in; a workspace that differs from the process's directory in a
running terminal. No microphone, model, download or transcription was used
and no voice event was supplied by a test.

## Open

- A strictly read-only start would also not create the `models` directory.
  Two retained cases read that directory after a refused start, so it is
  still created; removing that is one statement.
- The unit case of the executor is compiled only where voice input is. The
  gadget test mode's unit-test binary has 16,777,028 bytes of unwind data on
  this branch, 188 under the unchanged limit with no linker warning; it had
  220 of room before. That case took 344 in its first form.
- The voice-enabled unit-test binary is outside both full test modes. It
  holds 17,267,944 bytes of unwind data, 490,728 past the limit, and links
  with the linker's warning about that. The four unit cases ran in it.
- Linux, hosted runs, a microphone and a real model remain unvalidated.
