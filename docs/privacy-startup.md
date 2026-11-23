# Private terminal startup capture

This is an implemented local foundation, disconnected from normal dispatch.
It adds no public command, configuration writer or automatic password popup.
`main` continues to use `Config::load`; enabled normal policy still returns its
existing not-yet-available refusal before frontend, store or provider effects.

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
model or protocol. `require_normal_activation` always refuses as unavailable.

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

The component is compiled privately. Core must still connect the captured owned
entry to this opaque owner after checking its actual bootstrap requirements;
raw-Arc trusted-host prototype compatibility stays separate and unselected.
Ordinary isolated source/options child checks prove only this component, not real
TTY/secret/UI/model/native readiness or normal dispatch.

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

The normal route and actual startup-to-frontend handoff remain unselected. The
isolated consuming checks use synthetic files, enter no password popup and
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

The next route must consume these same owners before the competing normal
TerminalGuard/EventStream, store, provider and helper construction. It must use
the owned password UI and an actual fresh conversation shared across every lane,
retain accepted human inputs, and retire/rebuild on `/clear`. Reopen, transfer,
replay and void rebinding cannot substitute for fresh ownership. Explicit
create-only init/status and a separately validated enable writer remain later
work; presence is not unlock or source consent. No route for ACP, `-m`, service
or another frontend is selected by this foundation.

New isolated-file checks exercise actual selected-source capture and consuming
ownership, project/caller exclusion, absent/disabled compatibility, existing
source bounds, malformed policy, file/root replacement, link rules and fixed
output. Test origins exist only under cfg(test), use synthetic user files and
cannot issue runtime origins. The real CLI/TTY readiness path is source-reviewed
and remains unselected; these checks enter no terminal or private MAIN probe.

This foundation does not certify the final union's size or warnings, core file
consumer corrections, native/full17/all-four/every-step inventories, independent
merged-main proof, normal activation, install or publication. Earlier failed
sources and measurements remain evidence. The same original ledger remains
$24.285934/$25 with 40 unknown reservations and no new paid calls.
