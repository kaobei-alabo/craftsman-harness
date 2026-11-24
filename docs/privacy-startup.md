# Private terminal startup capture and the interactive route

This is implemented local source on an unmerged, uninstalled branch. `main`
passes its parsed invocation to `privacy_startup::normal_route` only when it is
the eligible owned interactive one: no subcommand, no `-m`, no `--session`, and
a terminal on both stdin and stdout. An enabled policy in the actual user file
then starts the private terminal. Every other invocation keeps
`Config::load`, which refuses an enabled user policy before frontend, store or
provider effects. Nothing enables privacy by itself. `craftsman privacy
status`, `craftsman privacy init` and `craftsman privacy enable` are the three
explicit commands; only `enable` changes the user file, and only by adding a
policy to a file that has none. See
[the normal interactive route](#the-normal-interactive-route).

`privacy_startup::TerminalStartupCapture::capture_actual` parses the actual CLI
and captures the actual selected user configuration source. The corresponding
crate-private entry can reuse the already parsed CLI at a later routing point.
Neither entry accepts a caller Config, JSON body, digest, permission flag or
merged `Config.extra` as selection authority. Project configuration is not read.
Only the actual user policy can produce the opaque `CapturedTerminalStartup`.

The selected owner holds the same `ConfigRoot`, no-follow ancestor descriptors,
regular single-link user-file lease, original bytes, parsed configuration and
`CapturedPolicy`. The original bytes remain in a separate opaque source owner
with duplicates of the same held root/file descriptors, in a zeroizing buffer under
the existing 1 MiB user-source ceiling. Read-only configuration/policy access and
consuming handoff recheck root identity, descriptor and named-file metadata,
exact bounded contents and EOF. Source changes refuse with fixed text before
the caller receives usable configuration or policy. These observations are
sampled; they do not form a continuous writer lease or remove a later race.
Ordinary policy captures and their type shape remain unchanged; they do not
acquire this additional retained byte buffer.

Missing and disabled sources keep the original legacy loader behavior,
including missing directories, valid legacy directory modes and large unknown
fields when privacy is absent. The loader body is shared without changing its
parse, privacy rejection or default behavior. An early private-root or bounded
capture failure can reach that loader only while no selected owner exists;
the loader already rejects enabled or malformed privacy. It cannot turn those
errors into a disabled policy. Once a source was captured, a stale or changed
source fails directly. Present privacy policy keeps all its existing bounds.

Captured configuration credentials are neither resolved nor logged. Placeholder
strings remain strings, unknown user fields remain present, and vault keys and
passphrases never enter these structures. Owner Debug and error output is fixed.
This operation creates no terminal, thread, key, vault, grant, store, provider,
helper, SDK object or model request, and writes no configuration or environment.

The actual parsed CLI and terminal descriptors supply refusal checks for a
subcommand/frontend, one-shot message, `--session` and non-TTY input/output.
The detector feature must exist; declared identifiers that the private bootstrap
cannot admit are refused. Windows secure-root capture remains unsupported until
genuine owner/DACL support is reviewed. Unix modes are not Windows evidence.
These controls can only refuse a user-source proposal; they are not policy,
source approval or permission grants. Rule-directory capture occurs after the
controls pass, before returning the completed captured policy.

`CapturedTerminalStartup::into_owned` and
`OwnedTerminalStartup::into_configuration_policy` preserve the configuration,
actual policy, separate retained source owner and typed `StartupRequirements`.
The source owner must remain beside the policy through the later bootstrap;
it cannot be discarded or replaced by a policy reconstructed from configuration.
The requirements
retain fresh shared conversation identity, all requested features and actual
native views, owned key/source approval before guest execution, voice/native
inspection and final validation as obligations. They do not infer compatibility
from model names or protocol labels, omit requested capabilities, or switch a
model or protocol. `require_normal_activation` always refuses: these retained
obligations alone activate nothing. `StartupPrerequisite::is_delivery_evidence`
marks the two labels that are process evidence, review of the routing and
final source and native validation. They hold merge and install and are never
recorded by a running startup.

The private parsed-options component now retains the actual CLI permission-mode
override, auto-approve bit and mouse choice inside those still-pending requirements,
plus the existing `CRAFTSMAN_CLI_SHELL_ALLOW_LIST` source captured once. Missing or
non-Unicode environment values add nothing; comma-separated nonempty entries keep
their original trim/order/duplicate behavior. Neither values nor their getters
issue policy, source/capability approval, model support or readiness.

`OwnedTerminalStartup::approval_start` rechecks the original user source and derives
one cached live approval owner. It preserves normal precedence without mutating
Config: CLI mode overrides user mode, then CLI OR user auto-approve forces Bypass
even with explicit Plan; web auto uses that same auto flag OR the user's existing
web switch, and the user shell list precedes the actual environment entries.
`CapturedApprovalStart::state_for` accepts only that startup's retained options and
same live state identity after the consuming moves. A copied matching state,
foreign requirements, absent actual options or poisoned state cannot substitute.
The same Arc retains actual class-specific Always effects through build/clear;
issuing again never resets or replays them.

The captured owned entry is now connected to this opaque owner.
`OwnedSecretTerminal::run_captured_private` takes the `CapturedApprovalStart`
in place of a caller-built approval state. Before any key work the bootstrap
rechecks its source and lends its retained requirements, and `state_for` yields
the live state only for the start issued with them. A bootstrap without startup
requirements is refused as unavailable, and a foreign or unavailable start with
the fixed approval-source refusal; nothing is unlocked in either case. The same
Arc then reaches the bridge extras, the build, the renderer and the fresh-clear
rebuild unchanged. The caller must take `approval_start` before
`into_private_bootstrap` consumes the startup. The raw-Arc `run_private` stays
as the separate, unselected caller-Config route.

The owned interactive router enters here after it selected the startup. Any
other caller, including the five captured cases below, is not a selected
startup.

Five ordinary captured terminal cases enter through this path in a marked child:
the production capture of the child's own retained user file, the real parsed
fixture invocation and the child's own terminal, then the approval start and the
consuming four-owner handoff. They repeat the existing entry, question, opaque
review, file, checkpoint, settings-refusal and fresh-clear scripts with the
same assertions and deadlines. They prove this private entry under those
scripts. They are not real-user, normal-dispatch, activation or native-platform
proof, and a cancelled or refused construction is not among them.

## The normal interactive route

`main` decides the invocation's eligibility before anything of the private
source is read. For an eligible invocation `normal_route` then runs before the
legacy loader and before any terminal, store, provider or helper effect:

1. It captures the actual user source with the real parsed invocation.
   **A refusal of the capture is final.** Nothing is loaded again, so a source
   that proposed privacy can never fail and then start as privacy-off because
   a later read saw other bytes. Only the capture's own legacy answer, for an
   absent or disabled policy and its early compatibility cases, returns to
   `main`'s unchanged `Config::load`.
2. It refuses a source that does not explicitly set `voice.enabled:false`.
3. It takes the startup's approval start, then moves the four owners into the
   private bootstrap for the current directory.
4. It asks the same held root for the key files. A missing key refuses with a
   pointer to `craftsman privacy init`. Incomplete files refuse. Nothing is
   adopted, deleted or prompted for.
5. It runs the entry's own read-only check of the captured source, which
   includes the strict `terminal.osc7501` setting, before any terminal
   effect: a setting that does not parse refuses here, with no alternate
   screen and no password prompt.
6. It records that it selected this startup, enters the owned terminal and
   calls `run_captured_private`.

Every refusal before the terminal is fixed text. The route supplies no Config,
approval state, flag or receipt. It reparses nothing and writes nothing. From
step 6 on it is the private entry described above: the owned password popup,
the fresh conversation, the same full builder with every requested extra, and
`/clear` as retirement followed by a fresh unlock and rebuild.

**What the router does not sample:** the detector's stored source approval.
Every private build admits the system text through the pinned detector, so a
missing or unapproved detector is refused by the build, after the unlock, with
a general text and a pointer to `privacy status`. Sampling it before the
password would cost about 1,900 bytes of the unit-test binary's unwind room.

### Readiness

The shared `StartupRequirements` keeps one observation: the actual router
selected this startup after its read-only preflight. It is shared by every
conversation of the startup, including the ones a fresh clear builds, so
nothing about one built session is stored there.

Readiness of a build belongs to that actual bound session, which holds a
**build witness**. The witness joins three records of the same build, each
made by the object that did the work:

- **What the builder accepted, where it accepted it.** In the selection's own
  report the shared builder marks each model lane whose view provider it
  took (the root agent's, the delegated agents', the summarizer's) with its
  wire, that every installed part has a source path, and which controller
  adapters it installed.
- **What the view providers counted.** Each provider says which wire it
  speaks when it binds this conversation's guard.
- **What the root agent's boundary has installed:** the reply display, the
  file review and the controller adapters.

The conversation accepts a built session only when these agree with each
other and with what was requested: the root lane was accepted; the accepted
lanes are as many, and as many native, as the providers that bound this
conversation; the reply display the frontend asked for, the file review and
exactly the configured controllers are installed; and no wire is native,
because the policy requires that everything sent can be inspected. A build
that fails this is shut down for real and refused with the builder's own
words. `PrivateBuiltSession::task_owner` checks the same again, with the
thread, view and send guard, each time it issues a turn, a steering input, a
decision or a task cancellation. A fresh clear makes a new selection and
session, which get their own witness.

So **no model turn or effect is submitted before this startup's own bound
session exists**. Prompts and drafts that the build itself needs are not held
back, and the password prompt comes earlier still. Creating the pinned
detector is not source consent; each call's own check of the stored approval
remains the gate.

**Limit of the witness:** it does not keep the provider objects themselves.
The framework does not hand an agent's provider back, so their identity
cannot be compared later; the lanes are counted on both sides instead.

Not implemented, and refused when a startup requests it: voice input, and a
model wire whose provider items cannot be inspected. Neither is turned off or
downgraded, and neither is recorded as ready.

### The private terminal

For a startup that the router selected, or that the private captured entry
runs, the terminal does the following.

- **Read-only local commands** answer through the ordinary terminal's own
  text owners: `/help`, `/status`, `/context`, `/doctor`, `/config`,
  `/version`, `/tokens`, `/queue`, `/copy`, `/editor`, `/mcp`, `/skills`.
  They only read the retained configuration and session.
- **Session and registry inspection:** `/tasks` shows the background tasks,
  `/agents` shows the agent picker, and `/checkpoints`, `/memories`, `/lsp`,
  `/gadgets` and `/diff` list through the same code as the ordinary terminal.
  A listing that waits for its owner runs as a task of the frontend and its
  text arrives as returned work. Nothing is saved, started or sent to a
  model, and the line never becomes model input.
- **A running background task is cancelled for real**, with `/tasks cancel
  <name>` or from the task list. The cancellation goes to this session's own
  registry through the bound owner's checks, and the line shown is the
  registry's own answer.
- **Controls are classified on one trimmed view.** Leading spaces do not turn
  a control into a prompt. A refused control keeps the raw line in the queue;
  only a new prompt, not another control, lets the queue be tried again.
- **Settings changes are refused** before any mutation: permission mode,
  model, effort, harness, agent, voice model and provider setup. The captured
  source is not rewritten under a live conversation, and no retirement,
  recapture and rebuild path exists for them yet. Actions that are not saves
  get their own refusal.
- **Not executed:** shell escapes, extension commands, `/compact`, `/rewind`,
  `/session`, `/model`, `/effort`, `/mode`, `/harness`, `/voice`, `/setup`,
  `/tasks message`, `/lsp restart`, and `/agents <name>`.
- **Fixed-text OSC 7501 status** is reported with the ordinary change-only
  reporter, unless `terminal.osc7501` is false. Only fixed states and
  messages are written.
- **The mouse** is captured as the invocation chose, but never during a
  password prompt. The terminal guard keeps its capture marked as possibly on
  until a disable was written and flushed, so restoration still attempts it
  after an incomplete write.
- `--session`, `-m`, `setup`, every other subcommand, and a non-terminal
  stdin or stdout are not routed; with an enabled policy they get the
  loader's refusal.
- Declared identifiers refuse. A build without the gadgets feature refuses.
  Windows refuses until secure key storage is reviewed.

One thing the terminal keeps that the ordinary one does not: after a fresh
clear the earlier transcript is still shown, above the new conversation. The
model sees none of it; the new conversation has its own thread and sources.

### Status, initialization and enabling

`craftsman privacy status` is read-only and prints up to three fixed
statements: whether the key files exist, whether the actual user file enables
a policy, and the result of the read-only preflight. It reserves no
conversation and creates no thread: the inspection takes the captured source,
the key files and the detector's stored approval, and only a real start
allocates a fresh binding. A passed preflight means the policy, the voice
setting, the key files and the detector's stored source approval were found
and the captured settings parse. **It is not an unlock, an approval or
readiness.** The text names what a start still has to check: the vault
unlock, the detector's own call check, the model connection, native-view
support for the configured model wire, and support for every requested
session feature.

`craftsman privacy init` creates the key and its vault. It is create-only: it
refuses before any prompt when a key file already exists, asks for a new
password twice in the owned popup, and neither enables privacy nor changes the
user file. The password is the local mapping-vault unlock. It is not a model or
API password and is never logged or stored in history.

`craftsman privacy enable --detector <id>` adds a privacy policy to the actual
user file. Its explicit invocation is the authorization of that one change.

- It only adds. A file that already has a `privacy` member, enabled or not, is
  left alone; so is a missing file.
- The detector is pinned to the bytes installed under the user root now, read
  with the pinned call's own file rules, and must already have a stored source
  approval for exactly those bytes. Nothing is compiled, run or approved.
- Voice is not turned off for the user, and no provider, model or key field is
  touched: a file without `voice.enabled:false` is refused.
- The candidate is the original bytes with one member inserted after the
  opening brace. Before anything is written it is checked: every original
  member is still there with its value, the loader's own parser reads the new
  policy as valid and enabled, and it parses as a startup would take it.
- **The write.** The candidate goes to a new exclusive user-only file in the
  held root. That staged entry is the command's own by the identity of the
  file it created. Before the commit the staging name is reopened without
  following a link and must still be that file: regular, single-link,
  user-only, with exactly the candidate's bytes. Then the original named
  file, its source and the root's ancestors are checked again, and the staged
  file is renamed over `config.json`. The file is then read back.
- **Refusals.** A leftover with the staging name is never adopted,
  overwritten or removed. After a refusal the staging name is removed only
  while it still names the created file; an entry that no longer does is
  left in place and the command says so. A file that reads differently after
  the write is reported as changed after the write; nothing is restored over
  it.
- **Limits.** The checks before the rename are samples. Another writer can
  still act between the last check and the rename: this is not a
  cross-process compare-and-swap, and no power-loss durability is claimed.

The detector's source approval is given through the existing gadget approval
prompt of the ordinary terminal, before the policy is enabled. Data-file rules
and identifiers are the user's own later edits.

### Evidence and limits

- **Ten terminal cases stay in the unit-test binary:** five inherited and five
  captured ones, which call the private captured entry directly. The captured
  safe-entry case also runs `/version` and a `/status` with leading spaces.
- **The integration test `tests/privacy_cli.rs` drives the real binary**
  through its own `main` on synthetic user files in a private temporary
  directory, with a synthetic model that the test itself serves on a
  loopback address.
  It has eleven cases; eight need the gadgets feature.
  - *Setup through the real runtime.* The ordinary terminal runs once: the
    synthetic model calls the detector gadget, and the person approves its
    first use at the real prompt, which names the gadget and the hash of its
    installed bytes. The runtime stores that approval itself. The test
    writes no grant file and supplies no approval state.
  - *`enable` succeeds.* After a refused run against a leftover staging
    name, the command adds the policy. The file is the original bytes with
    one member after the brace; the member pins exactly the installed
    component and manifest; every other member, including one the program
    does not know, has its value; no grant was written and the mode is
    still user-only. A second run refuses. Status then reports the missing
    key, and after `init` a passed preflight.
  - *The private terminal runs with that policy:* unlock, a masked turn, a
    fresh clear with a new unlock and thread, a second masked turn, fixed
    status reports throughout, and three plaintext thread records.
  - *The five route conversation cases* run here under their earlier names:
    question, reviewed file write, refused settings change and command,
    follow-up, fresh clear, its cancellation by Escape and by Ctrl-C, and
    type-ahead kept as a draft. The safe-entry case also runs the
    inspection commands and a task cancellation that finds no such task;
    its request count shows that none reached the model.
  - *Refusals:* the status text for each local state; which invocations are
    routed; the router's early refusals on a real terminal, including an
    invalid status setting before any terminal effect; every `enable`
    refusal with unchanged files; and a build refused for want of a
    detector.
- **Inventory transition.** The five `actual_normal_…` conversation cases
  moved from the unit-test binary to the integration test under the same
  names. There a test child called the router; here the real binary's `main`
  does. Each keeps its stages, its request count and masking checks, the
  written file, restored terminal modes, the unchanged user file and key,
  and the recorded threads and inputs, now read from the store's own tables.
  The private threads are counted beside the ordinary session's thread,
  which the real approval adds.

First failures, all retained: the real-binary success case's first run typed
its follow-up while the fresh clear's password prompt was still up, because a
stage matched the old composer line under the new popup; the entry cases'
first run found the reporter's fixed wait records, which the check did not
list; the inspection stages' first run sent Escape directly before a command,
which the terminal reads as one key; and the refused-build case found no
first "working" report once under four parallel cases, where it had always
found one alone. Earlier ones: the cases' first run with status reports
missed every stage after the password in the inherited screen reader; an
inherited case asserts the capture's `Invalid` answer for an unparsable file,
so a line-and-column answer was withdrawn.

The gadget test binary's unwind section is 16,775,056 bytes, 2,160 under
the unchanged 16,777,216-byte limit, with no linker warning.

Not covered: a build whose witness disagrees, a stale or foreign binding at
issuance, cancellation of a task that is actually running, the staging
entry's removal and leftover report after a refusal past its creation, a file
changed after the write, non-default permission options and an Always effect
through the terminal, the terminal entry's own refusals, a mouse event,
`/context`, `/doctor`, `/config`, `/tokens`, `/queue`, `/copy`, `/editor` and
`/help` through a terminal, Linux and Windows, and a captured construction
that is cancelled, refused after the builder returned it, or outlives its
30-second bound.

The private consuming composition now exists as
`OwnedTerminalStartup::into_private_bootstrap(workspace)`. It moves the
configuration, actual policy, retained source and requirements into one private
owner. It derives key operations from the same held user root and reuses the
existing workspace observation and genuine fresh-conversation reservation;
it does not reparse configuration or recapture policy from a merged Config.
Read-only `captured_configuration` and `startup_requirements` access checks the
retained source. Status/init/unlock keep that owner through their existing
operations, and successful unlock retains it in the private conversation and
its selected policy. Requirements remain pending; no prototype result marks
normal routing, voice/native support or final validation complete.

The conversation's private `request_captured_full_session` accepts no Config;
it uses that owner's original configuration and forwards every requested
extra to the existing full shared builder. The older Config-taking request
can reach it only with a borrow of that same retained configuration, after
actual source checks; an equal caller or project-merged Config refuses.
Reference identity adds only this substitution check and creates no source,
human-decision or permission authority. Successful built sessions retain the
same actual owner through their policy and task owners. Fresh retirement shares
that immutable four-owner object and held workspace observations, preserving
requirements and delaying its new reservation until unlock; it does not renew
authority from new bytes, transfer old messages or use a constant namespace.
The two new forwarding methods keep the existing shared construction future
in one owned allocation and await its real result. This erases its concrete
runtime/drop type; it starts no task, changes no checks or body, refreshes no
budget and claims no native-worker quiescence.

A captured build that fails or is cancelled keeps its pending construction for
the frontend's 30-second retirement, and a session that a startup-issued owner
refuses after the builder returned it is shut down before the refusal returns.
Cleanup failure is reported separately around the primary error. The details
and limits are in [privacy-bootstrap.md](privacy-bootstrap.md). Requirements
stay pending.

The isolated consuming checks use synthetic files, enter no password popup and
make no private MAIN/model call. This composition needs its own measured binary
and independent/source/native proof; c33's tests and margin cover c33 only.

The bounded layout correction now uses the existing erased worker in the
ordinary gadget I/O timeout adapter. The work, timeout argument, error mapping,
capability/approval checks and key-operation JoinHandle ownership stay exact.
At layout commit `445fbfb4e3a00e169d7e12c60cc5f8835f2f4e67`, the actual gadget
unwind section was 16,749,644 bytes against the unchanged 16,777,216-byte limit,
with 27,572 bytes of measured room and no linker warning. Core measured
9,136,668 bytes. Source/consuming and key checks passed in both modes. The
earlier 1,456/1,404/1,424-byte excesses remain recorded failures; no test or
bound was removed. This is compiled private composition evidence, not proof
that future routing fits or that normal startup/native/activation is ready.

The route consumes these same owners before the competing normal
TerminalGuard/EventStream, store, provider and helper construction. It uses
the owned password UI and an actual fresh conversation shared across every lane,
retains accepted human inputs, and retires/rebuilds on `/clear`. Reopen, transfer,
replay and void rebinding cannot substitute for fresh ownership. The enable
writer is the explicit `craftsman privacy enable` command described above;
presence is not unlock or source consent. No route for ACP, `-m`, service or
another frontend is selected: they are not passed to the router, and with an
enabled policy the unchanged loader refuses them.

New isolated-file checks exercise actual selected-source capture and consuming
ownership, project/caller exclusion, absent/disabled compatibility, existing
source bounds, malformed policy, file/root replacement, link rules and fixed
output. Test origins exist only under cfg(test), use synthetic user files and
cannot issue runtime origins. These isolated-file checks enter no terminal or
private MAIN probe. The real CLI and terminal path is the route above; the
terminal cases and the real-binary test under
[Evidence and limits](#evidence-and-limits) exercise it.

This document does not certify the final union's size or warnings, core file
consumer corrections, native/full17/all-four/every-step inventories, independent
merged-main proof, review of the route, install or publication. Earlier failed
sources and measurements remain evidence. The same original ledger remains
$24.285934/$25 with 40 unknown reservations and no new paid calls.
