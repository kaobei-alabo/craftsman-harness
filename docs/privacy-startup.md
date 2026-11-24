# Private terminal startup capture and the interactive route

This is implemented local source on an unmerged, uninstalled branch. Right
after the parse, `main` passes its own parsed invocation to
`privacy_startup::normal_route`. An enabled policy in the actual user file then
starts the private terminal for the owned interactive invocation, and refuses
every other one with fixed text: another frontend, a one-shot message, a
resumed session, or input and output that are not a terminal. Without an
enabled policy the unchanged `Config::load` and legacy startup follow. Nothing
enables privacy for the user: there is no configuration writer and no
automatic password popup without that policy. `craftsman privacy status` and `craftsman privacy init` are the
two explicit commands. See [the normal interactive route](#the-normal-interactive-route).

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
model or protocol. `require_normal_activation` refuses until every obligation
was recorded as complete for that one startup by the owner that completed it.
A freshly captured startup has none recorded.

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
other caller, including the five captured cases below, leaves the requirements
incomplete: those cases assert the refusal before they enter.

Five ordinary captured terminal cases enter through this path in a marked child:
the production capture of the child's own retained user file, the real parsed
fixture invocation and the child's own terminal, then the approval start and the
consuming four-owner handoff. They repeat the existing entry, question, opaque
review, file, checkpoint, settings-refusal and fresh-clear scripts with the
same assertions and deadlines. They prove this private entry under those
scripts. They are not real-user, normal-dispatch, activation or native-platform
proof, and a cancelled or refused construction is not among them.

## The normal interactive route

`normal_route` runs before the legacy loader and before any terminal, store,
provider or helper effect. In order:

1. It captures the actual user source with the real parsed invocation. A source
   without an enabled policy returns to `main`, which then runs its unchanged
   `Config::load` and legacy startup. A capture refusal is returned only when
   that unchanged loader also reports an enabled policy; in every other case
   the loader's own result or error stands. A refusal can therefore never
   continue as privacy-off, and a legacy user gets no new error.
2. It refuses a source that does not explicitly set `voice.enabled:false`.
3. It takes the startup's approval start, then moves the four owners into the
   private bootstrap for the current directory.
4. It asks the same held root for the key files. A missing key refuses with a
   pointer to `craftsman privacy init`. Incomplete files refuse. Nothing is
   adopted, deleted or prompted for.
5. It records the routing selection on the retained requirements, enters the
   owned terminal and calls `run_captured_private`.

Every refusal before the terminal is fixed text. The route supplies no Config,
approval state, flag or receipt. It reparses nothing and writes nothing. From
step 5 on it is the private entry described above: the owned password popup,
the fresh conversation, the same full builder with every requested extra, and
`/clear` as retirement followed by a fresh unlock and rebuild.

`StartupRequirements` records completion in two steps, each by its own owner.
The router records the routing selection and that the source requests no voice
input. The conversation records the rest when its bound full session exists:
the fresh conversation shared by the main, delegated and compaction lanes,
every requested part with its private view, the owned key with its pinned
detector, and the source check that the build ends with. The frontend of a
routed startup takes no input unless the set is complete. This is runtime
state of one startup. It is not evidence of review or of hosted or native
validation; those gate merge and install, not this object.

Two obligations are not implemented, and each is refused when a startup
requests it: voice input, and a model wire whose provider items cannot be
inspected. They are not turned off or downgraded.

The private terminal is narrower than the ordinary one. These are current
restrictions of an enabled policy, not defects of the legacy path:

- Slash and shell commands other than `/clear`, `/quit` and `/exit` are not
  executed; the draft is kept.
- Settings, model, effort and setup saves are refused.
- The mouse is captured as the invocation chose, but never during a password
  prompt, where the popup must be the only raw reader. Capture starts when the
  ordinary reader takes over and stops before a fresh-clear prompt.
- OSC 7501 status is not reported and `terminal.osc7501` is not read.
- `--session`, `-m`, `setup`, every other subcommand except `privacy`, and a
  non-terminal stdin or stdout refuse.
- Declared identifiers refuse. A build without the gadgets feature refuses.
  Windows refuses until secure key storage is reviewed.

### Status and initialization

`craftsman privacy status` is read-only and prints up to three fixed
statements: whether the key files exist, whether the actual user file enables
a policy, and whether an interactive startup would be refused before a
terminal, with the reason. It does not sample the invocation's terminal, run
the detector or look at the detector's stored source approval.

`craftsman privacy init` creates the key and its vault. It is create-only: it
refuses before any prompt when a key file already exists, asks for a new
password twice in the owned popup, and neither enables privacy nor changes the
user file. The password is the local mapping-vault unlock. It is not a model or
API password and is never logged or stored in history.

There is no enable writer. Enabling is the user's own edit of `config.json` as
described in [privacy-key.md](privacy-key.md); `status` then reports on the
complete file. The detector's source approval is still given through the
existing gadget approval path before the policy is enabled.

### Evidence and limits

Five normal cases repeat the full-entry, safe-entry and three fresh-clear
scripts through the production router, as `main` calls it, in a marked child
with the real parsed fixture invocation. They share one script each with the
captured cases, so assertions and deadlines are the same. One more case covers
the router's early answers on synthetic user files that never reach a detector
or a model: voice not turned off, a disabled policy, and a valid policy without
a key, where `--session`, `-m` and another frontend refuse first, followed by
`privacy init` through the child's own terminal and a second `init` that
refuses. The five normal cases run with the default mouse choice. In one
observed run the terminal switched capture on after each unlock and off before
the fresh-clear prompt and at exit, and the private captured entry emitted no
mouse control sequence. No case sends a mouse event or asserts this.

The first run of that last case missed a screen stage although the child had
completed and the expected line was in the captured bytes. The parent now waits
only for prompts that wait for input, and the child asserts the returned status
text itself. That first failure is retained. No older assertion or deadline was
changed.

The router first returned `anyhow` errors. With the cases calling it, the
gadget test binary's unwind section was 16,787,900 bytes, 10,684 over the
unchanged 16,777,216-byte limit. The router now returns the terminal's own
typed error, and its combinator closures are plain matches. The section is
16,776,188 bytes, 1,028 under, with no linker warning. That margin is thin.

Not covered: a build without the gadgets feature, a non-terminal invocation,
`--session`, non-default permission options, an Always effect across a fresh
clear, the terminal entry's own refusals, Linux and Windows, and a captured
construction that is cancelled, refused or outlives its 30-second bound.

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
replay and void rebinding cannot substitute for fresh ownership. A separately
validated enable writer remains later work; presence is not unlock or source
consent. No route for ACP, `-m`, service or another frontend is selected;
with an enabled policy the router refuses them.

New isolated-file checks exercise actual selected-source capture and consuming
ownership, project/caller exclusion, absent/disabled compatibility, existing
source bounds, malformed policy, file/root replacement, link rules and fixed
output. Test origins exist only under cfg(test), use synthetic user files and
cannot issue runtime origins. The real CLI/TTY readiness path is source-reviewed
and remains unselected; these checks enter no terminal or private MAIN probe.

This document does not certify the final union's size or warnings, core file
consumer corrections, native/full17/all-four/every-step inventories, independent
merged-main proof, review of the route, install or publication. Earlier failed
sources and measurements remain evidence. The same original ledger remains
$24.285934/$25 with 40 unknown reservations and no new paid calls.
