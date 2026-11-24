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
  and the receipt is marked with this selection and lane. A provider says
  in its receipt where it is once it is behind its one shared reference.
  At the statement that installs a lane's provider (the root agent's
  builder, a delegated agent's specification, the summarizer) the builder
  says which object it installs there.
- **What each provider was given.** The same receipt states the sinks its
  provider was given, by object: the token sink in the constructor, the
  round sink and the status sink where the provider takes them. It also
  states which of the three it was given at all, so a sink given as none is
  told from a sink never given.
- **Said once.** The request, each lane's receipt, each part, each consumer
  and the middleware are recorded once. A second, different statement of
  one of them is kept out of the record and refuses the selection. Which
  parts were stated is kept beside their values: a part stated as zero is
  not a part never stated.
- **Each constructed consumer.** The approval gate, the question tool and
  the reply display are recorded as the constructed objects themselves,
  kept weakly. What each holds of the request's prompts and sinks is read
  from that object when the witness is checked, not from the request.
- **Each composed part.** The builder records the address of the file
  review, of the controllers it made and their adapters, of the quiet sink
  it makes for the delegated and the summary lane and of the workflow part,
  and the count and addresses of the extra tools.
- **Each delegated tool gate.** A gate constructed for a delegated agent of
  this conversation tells the conversation's boundary what it holds: the
  approval prompt, the two correlated owners and the prompt for private
  decisions. Gates that disagree leave a value that equals no request.
- **Every middleware it kept**, after it found that each has a source path.
- **Each choice it acted on.** The builder says that it acted on a choice of
  the request (round limits, workflow, memory, verification, context,
  failure controls, the cancellation owner, model interruption, the gadget
  runtime and its options) at the place where the choice's own work has
  succeeded: after the context or failure setting parsed, after the
  verification plan loaded, after the gadget runtime was constructed.
- **The installed tools.** Once the agent exists, every tool the builder
  installed (the registered tools with the file wrappers, the verifier,
  the question tool, the extra tools) is compared with the agent's own
  registry entry of its name, and the builder says so only if each is that
  entry.

The conversation accepts a built session only when all of this holds:

- the builder accepted a provider for each of the three lanes, and
  installed the root agent's. Every provider it installed is alive: an
  installed provider that was dropped or replaced leaves its receipt
  unavailable, and that refuses. Only an accepted provider that was
  installed nowhere (the delegated lane's, when no delegated agent is
  configured) may be gone. Each live receipt holds this conversation and
  its guard and carries this selection and that lane; none is the native
  provider, because the policy requires that everything sent can be
  inspected;
- what the builder installed for a lane (the root agent's provider, a
  delegated agent's, the summarizer's) is the very provider that issued the
  lane's receipt, at the place that provider itself says it is: no other
  object stands in front of it;
- each live receipt states the sinks the builder gives that lane and no
  other: the root agent's provider the display's own token sink and the
  round sink the display made, or the request's two where there is no
  display, and the request's activity sink where that provider takes a
  status sink; the other two lanes the quiet sink the builder made, alone;
- the approval gate, the question tool and the display are alive and are
  the installed ones: the gate is one of the kept middleware, the display
  is the object the root agent's boundary holds. Read from them, the gate
  holds the request's activity sinks, approval prompts and decision
  prompt, the question tool the request's question prompt, and the display
  the request's reply sink and, behind the round sink it made, the
  request's round sink;
- the cancellation owner installed on the agent is the one the frontend
  handed over and still holds, by owner, or none where none was requested;
- the delegated agents' tool gates, where one was constructed, hold the
  request's own approval prompt, correlated owners and decision prompt;
- every tool the builder installed is the agent's own registry entry of its
  name, and the extra tools are the request's own by count and address;
- the gadget runtime of the built session exists exactly where the builder
  says it made one, and what that runtime itself holds as allowed gadgets
  and as workspace trust is what the request allowed and trusted;
- the builder acted on exactly the requested choices where the choice alone
  decides, and on no unrequested one where the configuration decides too;
  a verifier, a controller or a workflow part exists only for a requested
  choice, and the workflow part is one of the kept middleware; no output
  owner, send guard or script capture was requested, because a privacy
  conversation has no place that installs them;
- the file review and the adapters are the very objects the root agent's
  boundary holds; an adapter exists exactly for each controller the builder
  made, and those controllers are the built session's own;
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

- Identity is the address of a live object, compared while its owner holds
  it. A receipt is the one exception: the provider that issued it keeps it.
- Where the user's configuration decides with the choice (the gadget
  runtime), the check is that nothing unrequested was produced, not that the
  requested part exists.
- What a consumer holds is read from the constructed consumer. That the
  approval gate is the installed one is shown by its place among the kept
  middleware, and for the display by the boundary's own reference. For the
  question tool it is shown by the registry comparison of the tools.
- The cancellation owner is compared with the agent's installed one by
  owner (`Agent::cancellation_session_is`).
- No provider of a privacy conversation is handed an output owner. That is
  shown twice: a request that has one is refused, and the object installed
  for each lane is the very provider that issued the lane's receipt. A
  provider that was handed an output owner or an output guard is wrapped in
  another object, and that object is not the receipt's provider.
- The registry comparison is by name: a tool that a later one of the same
  name replaced is not the registry's entry, and that refuses the build.
  It covers the root agent's registry. A delegated agent's own tools are
  covered only by what its tool gate says it holds.
- The allowed gadgets are compared as one number folded from the count and
  the bytes of the ids, taken from the request before the build and from
  the constructed runtime after it. Two different sets could give one
  number; it is a comparison, not a proof.
- The choices are compared as the request's bits with the bits the builder
  set where its work succeeded. For the two controllers, the workflow part
  and the gadget runtime the produced object is compared as well; for
  round limits, memory, model interruption and the cancellation choice the
  bit is what is compared (the cancellation owner itself is compared as
  said above).
- The middleware witness is of the root agent's list. The delegated agents'
  own parts are outside it, apart from their tool gates and the provider of
  their lane.
- **The controllers.** The framework validates the installed controllers'
  bindings and the middleware chain itself at every typed entry of the agent
  (a turn, a compaction, a retirement, an adoption). That is unchanged and
  is the check that protects a conversation. The witness adds one
  qualification for the time before the first entry: the builder runs that
  same validation once before it hands the session out
  (`Agent::validate_view_controllers`, which runs no turn, model request,
  tool, hook or checkpoint), and the boundary lists an adapter only after
  its last validation found the installed controllers to fit. Without it
  the witness of a build would list an adapter that no entry had validated
  yet; it is not a second or a weaker check.
- A build whose witness disagrees has never been produced by a test: the
  cases show that real builds satisfy it, with the display and without one.
  No case builds a private session on a native wire through the witness;
  that the native provider states a round sink and no status sink is read
  from its source.

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
  Its event task is aborted and then awaited there. That order is
  defensive: a task left to end by itself waits for every sender of the
  controller, and a sender kept anywhere would hold up the cleanup behind
  it. No such kept sender has been shown; the order does not correct an
  observed failure.
- A voice key typed before a password prompt asks for nothing afterwards:
  the request is dropped before the prompt, and the voice starts only on a
  new one.
- A dictated segment that would take the draft past the bound of typed and
  pasted input is refused whole and the dictation is cancelled; the earlier
  draft stays.
- A submitted draft leaves no dictation marker or text behind, as in the
  ordinary terminal. A request to recreate the controller shuts it down for
  real and resets the voice state.
- A dropped frontend tells it to cancel and aborts its event task; nothing
  can be awaited there, and nothing is claimed about capture that was still
  stopping.

A build without voice input refuses a source that does not set
`voice.enabled:false`: the setting could not be honoured, and it is never
turned off for the user. `enable` and the status follow the same rule.

**Validated without audio.** A case of `tests/privacy_cli.rs` runs only in a
build with voice input, which neither full test mode makes. On a synthetic
home with no voice model and no permission to download one, the controller's
session ends at its model check, before it opens an audio device or a
connection. The case shows the ownership and lifecycle around that: the
captured source keeps `voice.enabled:true`; the voice key starts the
ordinary controller from the captured configuration; the controller's own
event arrives through the frontend's channel at the ordinary handler; a
private turn runs after it; a fresh clear retires the controller before its
password prompt and the key starts a new one; the terminal ends cleanly. A
second case types the voice key behind a clear command: after the unlock a
whole turn runs and nothing of the voice has reported, and a new press of
the key then starts the controller. That case types no input that begins
with a space, because in such a build a space in an empty composer asks for
the voice too.

**Not validated:** capture, transcription and a dictated draft, and with
them the bound on a dictated segment, the state after a dictated draft is
submitted and the controller's reset. Those need a real voice event; none
was injected. No microphone, model or audio fixture was used.

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
  record of the conversation. The runner takes no directory and runs where
  the process stands, so the command runs only while that is still the
  workspace the conversation was captured for; otherwise it is refused. A
  command that is not executed is kept as it was typed: one that only
  waited for running work is tried again after it, and a refused one waits
  for a new prompt, like every refused control.
- **Local work is owned.** A listing and a shell escape run as one kind of
  task of the frontend. Each checks the conversation's source where it runs
  and again before its result is shown; a source that stopped gives a fixed
  line instead. The diff is of the conversation's captured workspace.
  Pending listings are reported in the status as work until they actually
  return.
- **Controls are classified on one trimmed view.** Leading spaces do not turn
  a control into a prompt. A refused control keeps the raw line in the queue;
  only a new prompt, not another control, lets the queue be tried again.
- **Settings changes** are the person's own `/model`, `/effort` and `/mode`,
  typed or chosen in their pickers. A terminal that the router selected
  carries the conversation over for them; see
  [Settings changes inside a conversation](#settings-changes-inside-a-conversation).
  The private captured entry, which no normal route selects, has no such
  authority and refuses every settings change before any mutation, as before.
- **A manual compaction runs.** `/compact` is the ordinary idle compaction on
  this conversation's own agent, through the agent's tracked path, so the
  summary and the kept messages keep their sources. The bound owner is
  checked before it starts and the sources before its line is shown. It runs
  as owned local work inside a cancellation turn of the frontend.
- **An extension command sends its expansion to the model.** The commands are
  those of the captured user root and the captured workspace, read when the
  line is submitted, as in the ordinary terminal. The person sees the line
  they typed; the expansion is the turn's input and is admitted and masked
  like any other typed input. It is never read again as a control or a shell
  escape, and nothing is restored into it. While work is running or an
  approval is parked, the command waits as it was typed.
- **Still refused,** each with its own line and nothing changed: a model
  catalog refresh over the network (`/model --refresh`), the mode cycle
  key, provider setup, harness, agent and voice-model selection, reopening
  a session, `/rewind`, `/session`, `/tasks message` and `/lsp restart`.
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

### Settings changes inside a conversation

A private conversation is bound to the bytes of the user file it captured, so
the file is never rewritten under it. A settings change therefore retires the
conversation, writes the file, and carries the whole record into a new
conversation of the same thread. Only a terminal that the router selected can
do this: the router lends it the authority, and every other entry holds none.

The steps, in order:

1. **Asked.** The ordinary selection decides what the change means. A named
   `provider/model` keeps its id; an effort or a thinking-token budget
   (`/effort TOKENS`, or the budget choice of the picker) is written where
   that model's own options live
   (`providers.<provider>.models.<id>.settings.reasoning`, or
   `model_settings.<model>.reasoning_effort` without named providers). The
   candidate bytes of the whole file are built and validated here, before
   anything is touched: they differ from the file in exactly those places,
   the policy member is untouched and still the valid enabled one, the
   candidate parses as a startup would take it, and what it then means is
   what the ordinary selection would have saved. The file must be, byte for
   byte, the one the conversation was captured from, under the lease the
   conversation still holds: a fresh read of the user root alone is not
   taken for that. The wire of the selected model is checked here as well,
   because the build of the new conversation would refuse it only after
   the retirement. A refusal is a line; the conversation goes on.
2. **Retired.** The conversation must be idle: no running turn, no local
   work, no running background task and no parked approval. Before anything
   is retired, three things are decided without changing the record or
   the store, and a refusal of any keeps the conversation as it was:
   - *Every recorded message with its source.* The one function that the
     adoption decides with is applied to the live record: the source is
     this conversation's own, on its lane, it fits its message (the
     captured text, the commit, the call, the parts that make up exactly
     the recorded content), a reply has no provider items, and every
     model-origin text can be renamed. A message that has no source yet
     cannot be decided here, and that is a refusal of its own: it does not
     say that the record cannot be carried, because the agent gives such a
     message its source when it next enters.
   - *The renaming.* It is done here with this scope's own placeholders,
     so that an occurrence that cannot be renamed is found now.
   - *The vault's room.* The vault is asked whether it has room for one
     further scope with as many mappings and sealed bytes as this scope
     holds, under the unchanged bounds of an insertion: 512 mappings in a
     scope, 4,096 mappings and the sealed-byte bound in the vault, 128
     distinct scopes. Nothing is reserved, raised or pruned, and the store
     is only read. The question is still a request to the vault like any
     other: if its I/O, its worker or the store's integrity fails, the
     vault is marked failed and refuses everything from then on.

   Its own record of every private value is read while it is live. Then
   the agent retires:
   it issues its whole record and closes the source view, and with it the
   send guard, the vault scope and the key. A refusal up to here keeps the
   conversation as it was, with one exception: a record that no longer
   agrees with the sources kept beside it closes the conversation, and the
   line then says that it has ended.
3. **Written and recaptured.** After the frontend retired its owners, the
   candidate is committed through the same staged, identity-bound writer as
   `enable`, and the startup of this same invocation is captured anew. A
   write that changed nothing does not end the conversation: it is carried
   over under the settings it had, and the refusal is shown.
4. **Unlocked and built.** The vault password is asked again. A password
   that the vault refuses does not end the conversation. The retired record
   stays with the pending change, and the prompt says that the conversation
   is kept and how many attempts are left. The startup is then captured
   again, without another write, and the password is asked again through
   the same secret reader, for the same successor. Until the vault opens
   nothing else reads input and no work is admitted: a line typed there is
   a password. There are three attempts, as many as one key owner takes;
   an entry that cannot be a vault password counts as one. The new
   conversation keeps the thread and gets a vault scope of its own: its
   namespace names how many scopes the thread had before, so it is never the
   retired one, and its placeholders differ. A thread whose scopes are used
   up gets no further one: the count is checked, never reused.
5. **Adopted.** The new agent adopts the record; see below. The session's
   usage totals come along, and so do the shell commands and the network
   use that the person allowed during the conversation. An earlier
   "always" does not: it lets the gate decide a reviewed write itself,
   which a start would be refused for, so the new conversation asks again.
   A failure here ends the terminal with a fixed error. What the thread
   store then holds is the conversation's last save that succeeded: the
   recorded state in plain text, under the store's unchanged retention of
   50 rows for a thread. Nothing later than that save is there, and a save
   that failed is not.

What the adoption does with each recorded message:

- **The record is not changed.** Roles, calls, call ids, results, files,
  todos and custom state are the retired conversation's, and the thread's
  save sequence continues.
- **Every retired value is admitted first,** on its own and under the
  category the retired scope recorded for it. From then on the ordinary
  admission refuses any text in which such a value is not reported, so a
  value that was masked before is masked again or the adoption fails.
- **Raw text** (typed input, tool results, host text) is admitted anew from
  the retired capture itself, with the detector. It is never taken from a
  view.
- **Model-origin text** (replies, call arguments, summaries) keeps its words.
  Each placeholder of the retired scope in it is replaced by the new scope's
  placeholder for the same value, and the new send guard checks the result.
  Nothing is restored into it. An occurrence counts where restoration would
  read it as a placeholder of its category, as a whole token. That matters
  for a telephone placeholder, which is digits in the shape of its original:
  the same digits attached to more digits, or as one group of a longer
  spaced, dotted or bracketed number, are not the placeholder and stay. Of
  two occurrences of which one lies within the other the longer is taken;
  two that overlap otherwise refuse. One thing cannot be told apart: a
  number that a model wrote by itself, standing alone, that equals a
  telephone placeholder. It is renamed like the placeholder, as restoration
  would restore it.
- A stored todo keeps naming the reply that wrote it, now by that reply's
  new source.

No turn, model request, tool call or harness hook is replayed, and the
retired originals are sent to no model. The adoption does run the detector:
admitting raw text anew in the new scope is the pinned detector's own call,
as for any other input. The earlier transcript stays on the screen.

What is refused, with the conversation kept:

- a conversation that runs with context, failure or verification controls,
  whose records are not carried yet;
- a conversation with a reply that was received with provider items, a
  message of a delegated agent's lane, or a todo list that names a reply
  which is no longer recorded;
- **a permission mode that decides file writes without asking**
  (`accept-edits`, `bypass-permissions`, `auto`). The gate of a private
  conversation lets such a mode's automatic decision stand for a reviewed
  write, so a write that restores private values would not be shown first.
  `plan` and `manual` are taken. A start whose own option fixes the mode
  refuses a mode change too, because the file would not decide it;
- a model selection that the ordinary catalog refuses;
- **a model on a wire whose provider items cannot be inspected.** A named
  provider with a native protocol (`openai_responses`, `anthropic_messages`,
  `gemini_generate_content`) sends back such items, and the policy requires
  that everything a provider is sent can be inspected. The line is the
  build's own reason. A thinking-token budget is accepted only by such
  wires, so today a budget selection ends in this refusal or in the
  ordinary catalog's own; no budget has been written by this path.

Limits: the conversation is ended before the file is written, so a failure
after the retirement (a file that no longer proposes privacy, a refused
adoption, a cancelled unlock, a third refused password) ends the terminal
and the conversation is not resumed. The thread store holds its last save
that succeeded, as plain recorded state within the store's 50 rows for a
thread, and nothing later.
Every error after the retirement says that the conversation was retired and
what became of the change: not written; written; refused, with nothing
replaced and why; or replaced, or possibly replaced, and not verified. A
write that was made or may have been made is never told as unwritten.
Sub-agents' own records are not carried.

Known defects of the carry-over, from the independent review, not yet
corrected:

- **What only the new scope can decide still fails after the retirement.**
  The check before the retirement and the adoption now decide each
  recorded message with one function, and the vault is asked for room
  first. Three things remain that are decided only when the new scope
  exists, and a failure of one ends the terminal with the error described
  above: the new scope's own admission of the raw text, which runs the
  detector; the new send guard's reading of the renamed text; and the
  vault's insertions themselves, which are checked again when they are
  made. The room that was read is no reservation: a mapping written in
  between by another conversation of the same vault, or a sealed length
  that differs from the retired scope's, can still refuse. The read counts
  the retired scope's mappings; a value the detector reports in the new
  scope and did not report in the old one is not counted.
- **The adoption is polled; ending it while it runs has no case.** The
  adoption is the frontend's own kept work, polled as its build is: the
  terminal is drawn, a shutdown is observed and input is read. What is
  typed stays an editable draft or a queued line, as typed. Nothing is
  submitted, no control runs and no waiter is answered until the adoption
  has returned. The pending handoff (the retired conversation, the change
  and what became of it) stays with the settings authority until the
  adoption has returned success, and ends only there. Ctrl-C, an exit or a
  shutdown ends the kept work by dropping it: nothing was adopted unless it
  had already returned, what the new scope's vault had stored stays stored,
  and a vault or detector call that was running finishes or refuses under
  its own bounds. The terminal then ends with the one error that says that
  the conversation was retired and what became of the change. A case
  enters a line as soon as the composer shows and finds it sent once, after
  the adoption, behind the carried record; whether the build, the adoption
  or the ordinary loop read it is not determined. No case ends an adoption
  while it runs: the adoption would have to be slowed for that, and it is
  not. That path is read from source only.
- **What a closed conversation gives up, and what stays.** The one
  adoption takes the recorded state out of the transfer, so no second copy
  of the whole record stays behind. Each adopted commit keeps the transfer
  for the retired agent's own commit and source of its message. That is
  the message's provenance and it stays, with the raw capture and the view
  of each recorded message, and through it the retired conversation's
  owner stays referenced for the life of the successor. What that owner
  held beyond the record is given up when the conversation closes, by
  whatever still references it:
  - the vault gives up its connection, the database behind it and the
    derived keys; the keys are zeroized where they are dropped. A session,
    a masked text or a record of that vault that is still referenced keeps
    the vault's identity and bounds and refuses everything;
  - the conversation gives up its detector, the last map return it kept
    and its own references to the masked texts of its mappings, and its
    send guard lets go of the originals it was given.

  Stored mappings, their bounds and the file are not touched: a vault
  opened anew resolves the same placeholder to the same original. Limits:
  a vault worker that had started keeps its own reference to the
  connection and keys until it returns, under its existing bounds, so this
  is no statement that every copy in the process is gone at the instant of
  the close, and nothing is claimed about memory the operating system or
  the allocator still holds. An admission that is running at the close
  holds the conversation's record of mappings until it returns, and that
  record is then not cleared. A guard that is only closed, not released
  with its conversation, keeps its admitted set as before. The nodes of
  recorded messages keep their masked texts, and those hold the mapping
  originals in memory, as the raw captures beside them do; repeated
  changes chain the retired owners with them.

The router applies the same rule to a start: when the invocation's own
option or the captured file selects a mode that decides file writes, or an
automatic approval, the start is refused before any terminal or key effect,
and `craftsman privacy status` reports it. Nothing is changed for the user.
The private captured entry, which no normal route selects, takes the mode it
is given; five inherited cases state that there the gate's automatic policy
decides a reviewed write.

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
  unverifiable, never as unchanged; nothing is restored over the file. A
  rename that was refused, with the staging name gone afterwards, is
  reported as such: that absence is the one sign that the rename may have
  taken effect, so the command says that the file may have been replaced
  and does not call it unchanged. No case produces that outcome.
- **Limits.** The checks before the rename are samples. Another writer can
  still act between the last check and the rename: this is not a
  cross-process compare-and-swap, and no power-loss durability is claimed.

`craftsman privacy set <setting> <value>` changes one setting of a user file
that enables privacy, without a conversation. It knows `permission_mode`,
`model` and `agent`, each with its own check of the value. It takes only the
permission modes that still ask before a file write, `plan` and `manual`,
for the reason given under
[Settings changes inside a conversation](#settings-changes-inside-a-conversation).

- An existing member's value is replaced where it stands; a missing member
  is inserted after the opening brace. Every other byte is kept. A member of
  the same name inside another value is not touched, and a file that names
  the member twice is refused.
- The whole candidate is validated before anything is written: only that
  member differs and holds the value, the policy is still the valid enabled
  one, and it parses as a startup would take it. The voice rule of the build
  applies; nothing is turned off.
- The same staged, identity-bound replacement as `enable` commits it, with
  the same refusals and limits.
- It starts no conversation. The change applies to the next start. A private
  conversation that is running elsewhere ends when its captured file
  changes; from inside it, `/model`, `/effort` and `/mode` carry it over.
- The candidate builder and the committer are the ones the in-conversation
  changes use.

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
  It has fifteen cases. Ten need the gadgets feature, and two more need
  voice input too, so the full test modes run thirteen and three of them.
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
    still user-only. A second run refuses. `set` then replaces one member
    where it stands, inserts a missing one and replaces that, each time
    with every other byte kept, and refuses an unknown setting, an invalid
    value and each permission mode that decides file writes without asking. Status then reports the missing key, and after `init` a
    passed preflight.
  - *The private terminal runs with that policy:* unlock, a masked turn, a
    fresh clear with a new unlock and thread, a second masked turn, fixed
    status reports throughout, and three plaintext thread records.
  - *The five route conversation cases* run here under their earlier names:
    question, reviewed file write, refused settings change and command,
    follow-up, fresh clear, its cancellation by Escape and by Ctrl-C, and
    type-ahead kept as a draft. The safe-entry case also runs the
    inspection commands and a task cancellation that finds no such task;
    its request count shows that none reached the model.
  - *Settings changes inside a conversation.* One conversation with a named
    provider and two models, and a second provider on a native protocol
    that is never contacted. A selection the catalog refuses, a model of
    the native provider, a budget the current model does not declare and a
    non-asking permission mode each give a line, no password prompt and an
    unchanged file. `/model` with an effort, then `/mode plan`, each retire
    the conversation, write the file, ask for the password and carry the
    record over. The file keeps every byte but the named model id, that
    model's reasoning setting and one inserted member. The next request
    goes to the newly selected model with the earlier roles, call, id,
    result and reply in order, every text under the new scope's alias and
    none under a retired one; the stored todo is still offered. Usage
    totals come along. An extension command's expansion is the next turn's
    input; `/compact` then summarizes the carried record with one
    summarizer request. The store has one private thread with one unbroken
    save sequence, and its record before the compaction holds every input
    in order.
  - *A refused password after a settings retirement.* Four conversations.
    In each, `/model` retires the conversation with a draft typed behind
    the command, and a wrong password gives the prompt that says the
    conversation is kept; the file was written once and no request was
    made. In the first, an ordinary-looking line typed at that prompt is a
    second refused password and nothing else. The right password then
    carries the record over: the earlier transcript and the draft are
    there, the usage totals came along, and the next request goes to the
    new model with the earlier roles, call and result under a new alias.
    In the second, Escape ends the terminal with one error that names the
    cancelled unlock, the written change and the thread store. In the
    third, a third refused entry ends it the same way. In the fourth,
    Ctrl-C typed behind the command ends the terminal after the retirement
    and before the writer: the one error says that the conversation was
    retired, that the entry was cancelled and that the change was not
    written, and the file is as it was. Each store holds one private
    thread with one unbroken save sequence, with the record up to the
    retirement in the three that ended. No refused entry is echoed or
    sent.
  - *A mode that decides file writes* is refused at the start: by the
    status for each such mode of the file, and by the router on a real
    terminal for the file's mode, the file's automatic approval and both
    options of the invocation, each before any terminal effect. A build
    without the detector feature gives that feature's refusal first.
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
found one alone. In this slice: the carry case's first run expected no
catalog note among the carried messages, and it is carried like every other;
its first compaction expected a conversation too short to summarize, and the
agent summarized it; and the report check expected a reply as the last work,
where it was the compaction. In the corrections after the independent
review: the kept shell escape was first expected to run as soon as the
running one ended, but in that case the queue is held by the case's earlier
refused command, which only a new prompt releases, and the check now is
that the line is kept; the voice type-ahead case first typed inputs with
leading spaces, which in a build with voice input ask for the voice
themselves, and it now types none. The status check for the modes that decide
writes first expected the mode refusal in every build; a build without the
detector feature answers with that feature's refusal first, as it does for
every source, and the expectation now says so. A rule that asked a person
about every reviewed
write under an automatic permission mode was tried and withdrawn: five
inherited cases state that the gate's automatic policy decides such a write,
so the non-asking modes are refused instead. Earlier ones: the cases' first run with status reports
missed every stage after the password in the inherited screen reader; an
inherited case asserts the capture's `Invalid` answer for an unparsable file,
so a line-and-column answer was withdrawn.

**A fresh-clear case of the unit-test binary failed twice,** once in the
full run on the carried-conversation commit (the inherited
`actual_fresh_clear_reacquires…` case) and once in a run of the module (its
captured twin). Both stopped at the last stage, 60 seconds after the
follow-up was typed, with the first reply and an empty composer on the
screen. The terminal bytes that each failure recorded show the cause, and
it is in the test driver. The password prompt after a clear is drawn top
down over the earlier screen, so its first row and the old composer line
are on the screen together until the rows between them are drawn. In both
recordings the stage for the prompt and the next stage for the composer
are first true at the same byte. When a read from the terminal ended
there, the case typed the password and then at once the follow-up, at the
prompt, which discards what is queued behind the password; no second
request was made. The four sequences that wait for a composer after that
password now wait for the prompt's last row first. Replayed over both
recordings, the composer stage then matches only the new session's
composer. No deadline, assertion or screen reading changed, and the real
binary's cases already waited for the prompt to be gone. The ten entry
cases passed once afterwards; the failure was rare, so the replay is the
evidence for the correction and that run is not. Runs of these cases under
artificial load, made before the cause was read from the recordings, are
no evidence either way and are not counted.

The gadget test binary's unwind section is 16,776,996 bytes, 220 under
the unchanged 16,777,216-byte limit, with no linker warning. The start's
mode check and the four added stages had brought it to 288 under, 84 bytes
more than on the carried-conversation commit. The unlock loop of the retry
then replaced an adapter call with a plain match in the function that this
binary links, which left 388. The corrections after the independent
review are linked into this binary: the lanes' record, the record that is
said once, the writer's phase and the shell escape's directory check.
Written with adapter calls they left 40 bytes; as plain matches that do
the same work they left 212, and 220 with the phase disclosure, whose call
site is a match as well. The budget selection, the wire check, the
retry's own state and the voice corrections are not linked into it. The first
wiring of the settings flow was 1,932 bytes over. What that binary reaches
of it now adds no generic instantiation there: the adoption fact behind a
commit is held erased, an adopted message is made of the parts the request
view already knows, the authority's awaited work has the type of the
frontend's other local work, and the new branches are plain matches. The
flow itself is reachable only from the router, which the unit-test binary
does not link; the real-binary case covers it.

Not covered: a build whose witness disagrees, a stale or foreign binding at
issuance, cancellation of a task that is actually running, voice capture and
transcription, a cancelled shell escape, a settings change whose write or
adoption fails after the retirement, a carried conversation with host notes
of a background agent or with a summary, the model and effort pickers
through a terminal, a budget selection that is written, a signal or a
changed user file during the retry prompt, a write that is replaced and
not verified, a user file that is not the captured one, a thread whose
scopes are used up, a retirement refused for an inconsistent record, a cancelled
compaction, the staging entry's removal and
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
A settings change is the one path that carries a record, through the
retirement and adoption described above, into a startup captured anew.
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
