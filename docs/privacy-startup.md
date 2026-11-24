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
**build witness**. Every entry of it is an object of this build. None is a
count, a name or a supported-feature flag.

- **The original request.** Before the frontend hands its request to the
  builder, the conversation takes it down: every prompt, sink and owner by
  the address of that very object, and every choice as it is. The builder
  takes the same down before it consumes anything. The two must be equal.
- **Each model lane's provider.** A view provider issues a receipt inside its
  constructor, when it binds the conversation's guard, and keeps it through
  every later wrapping. The receipt holds that conversation and that guard
  and says which of the two providers issued it. Where the builder takes the
  provider for a lane, before the framework gets it, it records the receipt
  and the receipt is marked with this selection and lane.
- **Each composed part and installed owner.** The builder records the
  address of the reply display, the file review, the controllers it made and
  their adapters, and of the prompts and sinks at the place where it installs
  them.
- **Every middleware it kept**, after it found that each has a source path.

The conversation accepts a built session only when all of this holds:

- the root agent's provider is alive, and its receipt and that of every other
  lane still alive holds this conversation and its guard and carries this
  selection and that lane; none is the native provider, because the policy
  requires that everything sent can be inspected;
- the display, the file review and the adapters are the very objects the
  root agent's boundary holds; an adapter exists exactly for each controller
  the builder made, and those controllers are the built session's own;
- the installed decision prompt, approval prompts, question prompt, activity
  sinks and reply sink are the request's own objects;
- every kept middleware is still there and still states a source path.

A build that fails is shut down for real and refused with the builder's own
words. `PrivateBuiltSession::task_owner` checks the same again, with the
thread, view and send guard, each time it issues a turn, a steering input, a
decision, a listing or a task cancellation. A fresh clear makes a new
selection and session, which get their own witness.

So **no model turn or effect is submitted before this startup's own bound
session exists**. Prompts and drafts that the build itself needs are not held
back, and the password prompt comes earlier still. Creating the pinned
detector is not source consent; each call's own check of the stored approval
remains the gate.

**Limits of the witness.**

- The request's choices (round limits, workflow, memory, verification,
  context, failure controls, gadget and workspace options) are compared
  between host and builder, but no choice is traced to the single part it
  produces. The kept middleware are witnessed as objects, not by choice.
- The token and round sinks and the extra tools are compared in the
  request; where they are installed is not recorded. In a privacy
  conversation the main stream goes to the reply display instead.
- The controller adapters are compared by object with what the boundary
  holds. The framework's own binding check of them runs when the agent is
  bound; it is not run again at issuance.
- A lane whose provider is gone is not a refusal: it can send nothing.

Not implemented, and refused when a startup requests it: a model wire whose
provider items cannot be inspected. It is never downgraded or recorded as
ready.

### Voice input

A build with voice input keeps the voice setting a captured startup asks for.
The private terminal then owns the ordinary local voice controller.

- It is started on the person's request, from the captured configuration.
- Its events go to this frontend's own UI channel and from there to the
  ordinary handler: dictated text enters the same composer draft as typed
  text and is submitted as the same private input.
- It is cancelled and shut down for real first in every retirement, so
  before the password prompt of a fresh clear and inside the 30-second bound.
  Its event task is awaited there.
- A dropped frontend tells it to cancel and aborts its event task; nothing
  can be awaited there, and nothing is claimed about capture that was still
  stopping.

A build without voice input refuses a source that does not set
`voice.enabled:false`: the setting could not be honoured, and it is never
turned off for the user. `enable` and the status follow the same rule.

**Not validated:** no test mode compiles the voice controller, so the
adapter is type-checked only. It has never run with a microphone or a model.

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
- **A running background task is cancelled**, with `/tasks cancel <name>` or
  from the task list. The cancellation goes to this session's own registry
  after the bound owner's checks. The line shown means the registry accepted
  the abort of a running task; the task's own cleanup is awaited by the
  retirement.
- **A shell escape runs.** `!command` is the person's own command, run
  locally by the ordinary runner with the captured configuration's shell
  settings and native limits, inside a cancellation turn of this frontend.
  Its output is shown. Neither command nor output becomes model input or a
  record of the conversation.
- **Local work is owned.** A listing and a shell escape run as one kind of
  task of the frontend. Each checks the conversation's source where it runs
  and again before its result is shown; a source that stopped gives a fixed
  line instead. The diff is of the conversation's captured workspace.
  Pending listings are reported in the status as work until they actually
  return.
- **Controls are classified on one trimmed view.** Leading spaces do not turn
  a control into a prompt. A refused control keeps the raw line in the queue;
  only a new prompt, not another control, lets the queue be tried again.
- **Settings changes are refused** before any mutation: permission mode,
  model, effort, harness, agent, voice model and provider setup. Actions
  that are not saves get their own refusal. A settings change would need
  three things that do not exist yet:
  - a validated writer that replaces one existing member of the user file
    and keeps every other byte (`enable` can only add a member);
  - an adoption of the retired conversation's committed record by the
    freshly unlocked one: every message is bound to its own source in the
    old conversation's vault scope, and a conversation is never rebound;
  - a carry-over of files, todos, custom state and usage with it, without
    replaying any model, tool or hook work.
  Until then the captured source is not rewritten under a live
  conversation, and a fresh clear remains the only rebuild: it starts a new
  conversation and keeps no history.
- **Not executed:** extension commands, `/compact`, `/rewind`, `/session`,
  `/model`, `/effort`, `/mode`, `/harness`, `/voice`, `/setup`,
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
  overwritten or removed. After a refusal before the rename the staging name
  is removed only while it still names the created file; an entry that
  cannot be confirmed as that file is left in place and the command says so.
  Every failure after the rename is reported as written and then changed or
  unverifiable, never as unchanged; nothing is restored over the file.
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

The gadget test binary's unwind section is 16,776,464 bytes, 752 under
the unchanged 16,777,216-byte limit, with no linker warning. The five
captured cases' scripts and their three child functions are each folded into
one future, token for token, which is where the room for the witness came
from.

Not covered: a build whose witness disagrees, a stale or foreign binding at
issuance, cancellation of a task that is actually running, the voice adapter
in any run, a cancelled shell escape, the staging entry's removal and
leftover report after a refusal past its creation, a failure after the
rename, non-default permission options and an Always effect
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
