# Private owned password input and conversation bootstrap

This slice implements real owned-terminal init/unlock and key/user-policy/
pinned-detector/conversation composition. It remains private and unselected.
There is no public `privacy init`, `status`, `enable`, `/privacy`, ACP, `-m` or
service route. Normal enabled privacy policy still refuses before frontend,
store or provider effects. The existing absent/disabled path is unchanged.

## Password input

`OwnedSecretTerminal` installs the existing Shutdown signal owner, captures
terminal authority/modes through TerminalGuard before raw entry, and installs
its fixed-text panic hook. It reads directly from a clone of the retained
input descriptor before an ordinary EventStream exists. It creates no
background input task, crossterm event queue, UiEvent, composer/history entry,
clipboard/editor/voice draft, model/tool/agent value or journal/checkpoint
containing a password.

SafeBackend renders only bullets and fixed popup messages. The directly owned
UTF-8/paste/field buffers are zeroizing and bounded; the field reserves its
1024-byte capacity before typing and zeroes removed scalar bytes before
backspace truncation. Bracketed paste is admitted as a whole or refused, with
no trimming, lossy conversion or partial paste append. Each popup consumes at
most 16 KiB of terminal bytes, including control framing. The original and
NFC passphrase independently require 12–1024 bytes under the unchanged
[112-byte password envelope](privacy-key.md). The floor is policy, not strength
evidence. Multiline/control-bearing input is unsupported; Escape and an
unsupported escape sequence cancel rather than submit another draft.

The owner checks that its retained terminal is noncanonical and has ECHO and
ECHONL disabled before reading. It never shells out to stty/read/password
helpers. Input waiting is interactive, with no promised scheduling latency or
absolute user-input deadline. The popup requires sole input ownership; it is
not safe to start it beside a normal EventStream or during an active turn.
Unrelated TUI drafts and the existing active-turn cancellation paths are not
modified or routed through it.

Escape, Ctrl-C or an owned shutdown signal closes the operation explicitly;
none is an empty model answer, approval, Always choice or privacy opt-out.
Initialization requires real confirmation and exclusive absence of both
primary names and all sidecars. Unlock reads the envelope and opens only an
existing authenticated vault. The key manager's finite attempt allowance,
single directly owned KDF admission and original vault limits stay unchanged.
Cancellation drops the waiter, closes new admission, and awaits every started
handle before returning. Started blocking work may already have effects; it
is not interrupted or rolled back and partial files are never automatically
deleted or adopted. An actual key error remains an error; successful key work
cannot conceal failed cleanup.

The retained guard restores captured modes on normal release and uses the
existing best-effort signal/panic restoration. Owned PTY checks retain raw mode
observations; only macOS's kernel-transient PENDIN bit is excluded in their new
comparison. This proves behavior of those fixtures, not resistance to the OS,
malware, keylogging, crash dumps or foreign copies. Windows secure-key storage
still refuses explicitly without reviewed owner/DACL support; Unix mode bits
or read-only attributes do not substitute for it.

## Captured ownership and the private composition

`PrivateBootstrap` consumes a real ConfigRoot. It duplicates the already held
no-follow ancestor descriptors for independent key and policy owners, captures
the actual user config source before any project merge, and retains it for
the conversation. It accepts no merged Config.extra, project/model/gadget
label, caller string/hash/status or host Boolean as that authority. It captures
the canonical workspace and a directory handle, rechecks its sampled identity,
and reserves FreshConversationBinding from the application's own allocator.
No independent or constant conversation id is used.

After owned existing-only unlock, the private composition creates the detector
from the actual user policy's id, function and both pins, under the same
captured user root and workspace. Runtime construction executes no guest. On
each actual admitted input, the pinned call checks the owned source bytes,
both hashes, empty declared capabilities and the real stored exact source
approval before compile/describe/call. Key acquisition and policy capture have
already succeeded; a pin is a selector, not approval. The composition writes
no grant, builds no user gadget and calls no help program. Source authoring,
explicit build and human source/capability approval remain separate.

`PrivacySession::open` binds the reserved id and workspace to its actual vault
scope and send guard. `PrivateConversation` retains CapturedPolicy,
UnlockedVault, workspace and session together and closes the session/key on
release. Actual typed admission uses the original terminal draft, real detector
reply and authenticated vault commit before the separate model view is
released. Source/key/root liveness is sampled before and after admission;
this is not a continuous external-writer lease or an OS race-proof guarantee.
Only pre-build clear rotates a scope while retaining its live unlock. Failed
or abandoned rotation closes the key too. Built private `/clear` instead
retires its actual owners and requires another exclusive existing-only unlock
and a complete fresh build. No path raises the 128-scope ceiling, prunes,
reopens, transfers or replays.

The current admission interface masks with DeclaredIdentifiers::none(). A
nonempty actual user identifier policy therefore refuses before unlock; it is
never quietly narrowed. The feature-off composition also refuses a detector
selection; standalone private key initialization/unlock remains typechecked
and available only through the owned private operation.

## Private terminal construction and truthful limits

The captured real key/user-policy owner makes the full-set selection; no
fixture permission, merged Config.extra or ready Boolean substitutes for it.
`OwnedSecretTerminal::run_private` consumes the fresh popup, finishes secret
input/started-key cleanup, retains the original guard/panic/signal owners,
flushes the sole-reader descriptor, and hands off to a synchronous owned normal
poll/read loop with a 10 ms idle sleep. No EventStream polling thread is created
by this private entry; the ordinary terminal keeps its existing EventStream.
The actual UiBridge receiver is pumped through the entire build and loop;
no successful path leaves an undrained channel in the popup. The unattached
historical request method still refuses before bridge/build effects.

Every original terminal extra remains requested. The shared builder now has
actual controller adapters in the imported core. It verifies the same thread,
view and guard and keeps the key/policy owner with the real Session. Typed
turn/steer/decision tasks retain the SAME four-Arc view and actual agent while
the UI continuously drains its actual mutable notification receiver. Original
accepted text is passed without trimming or expansion; host catalog messages
remain their separate actual records. No shell/reopen/transfer route is silently
substituted for a typed private entry. Unsupported commands remain explicit.

The receiver presents core's original opaque approval request, keeps that
exact presentation in the modal, displays the complete bounded derived call,
and returns core's original decision after a real human choice. Raw feedback
and Edit JSON are retained before core parsing/projection. Positive choices
require the real review frame to be drawn; denial/cancellation stays available
at small sizes. The raw review cap is 1 MiB, unsafe controls/backslashes are
shown escaped before wrapping (up to six times the raw width), and full text
is scrollable. This is a conservative display bound, not a promise that every
real request will fit. The new private event batch is 32; ordinary 4096-event
batching is unchanged, and no global queue-byte cap is claimed.

Cancellation stops the actual agent before releasing prompt waiters. An absent
reply is not a synthetic denial/approval/model answer. Always effects remain
core's accepted-decision effects. Actual end/save errors and returned results
stay errors; closing revokes session/key even when a task retains their Arcs.
Owned shutdown awaits actual tasks/background joins/LSP under the original
30-second error bound and reports failure honestly; dropped async waiters do
not establish interruption or quiescence of native/blocking work.

The entry intercepts settings actions BEFORE configuration/permission/model
mutation and displays `SettingsSaveRefused`, preserving the live configuration
and conversation. Actually changed/unavailable captured source closes both
session and key with its separate fixed line. Built private `/clear` takes the
sole normal reader after its poll/read future returns, closes the old session
and key before releasing old prompts, and awaits actual owned work/background
joins/LSP. Cleanup/checkpoint failure refuses a replacement. The retired guard
and key owner must actually refuse before another password is requested.
At most 32 ready decoded normal events are retained in the same App as ordinary
drafts; this does not promise retention of incomplete parser fragments or input
arriving after the handoff. The same retained descriptor then belongs only to
the password popup, which clears owned secret fragments and flushes queued
bytes as described above. The old retained root/workspace must still match;
policy is recaptured from that actual source, and only successful existing-only
unlock reserves another opaque thread id. All original extras are rebuilt with
that id, new cancellation/bridge/receiver/approval owners and the new session.
Escape/Ctrl-C cancels the entry explicitly; it does not keep the old agent alive
or fall back to privacy-off. Pre-build scope rotation is distinct. There is no
reopen, transfer, replay or source-receipt renewal.

The ordinary macOS clear checks exercise actual owned PTYs, real key/KDF/vault,
captured policy, approved source-built WASM and the shared Chat builder. Fresh
clear, Escape/Ctrl-C cancellation and decoded ordinary draft retention pass,
including config/key byte preservation, restored modes, exact accepted human
checkpoint records and password absence. The first fresh-clear capture timed
out after the fresh request because completion overtook an already queued
committed reply in the private UI. That failure is retained; actual completion
now enters the same FIFO behind the earlier reply, with direct returned-result
handling retained when cleanup has closed the receiver. No injected callback,
budget/interleaving or restricted private MAIN probe supplies this proof.

Voice has no reviewed private input adapter: v1 requires an actual explicit
`voice.enabled:false` and refuses other settings. The captured policy grants
no opaque-provider exemption. Native wires that require one refuse, without
being called inspectable or downgraded to Chat. These are current restrictions,
not normal feature activation.

Generic name/format/operation-only data proposals remain inert. An actual
captured user declaration may select `contact_lines_v1` or `contact_json_v1`,
which use the compiled contact-line or closed owner/note contact-record schema.
The source-bound target precheck refuses ineligible destinations before private
data reads, selector comparisons or restoration; it grants no image or effect.
Both whole images, every selected/restored value range, JSON decoded-key and
escape boundaries, semantic exclusions, the same current captured policy and
the retained parent/held-file effect must still validate before a write.
No generic format callback, pointer, model label or fixture DTO grants data.
The existing 16-rule/source/path limits, 256 KiB image bound and bounded JSON
depth/string indexing remain; an unsupported object or field refuses.

The independently observed private FullEntry at exact `5dc83d9c` now restores
an actual issued alias into supported Contact data through the captured user
application, opaque human review and retained effect. The resulting real file,
exact human/checkpoint records, config/key bytes and password absence are
checked. The separate SafeEntry still writes an ordinary constant without a
restoration grant. ContactJsonV1 has the compiled whole-document/value-only
validator and retained-effect source path; the owned terminal FullEntry proves
ContactLinesV1, not a general JSON or platform claim. The earlier constant-only,
NoRule, responder/missing-field and partial captures remain failed or narrower
evidence. Normal selectors/activation, password input during live work without
retirement, platform/full17/hosted/native/security review remain open.

The new fixtures use exact argv-marked native PTYs, private temporary roots,
synthetic passwords, real KDF/envelope/vault IO, and an approved source-built
WASM detector. A separate full ordinary builder/loopback/checkpoint control
after successful popup acquisition checks password isolation with privacy
unselected; it is not full-set selected-bootstrap coverage. The current owned
FullEntry exercises actual question/opaque restored-contact review/file/
settings-refusal/follow-up/checkpoint behavior with the captured user owner and
real application/effect grant; SafeEntry remains its constant-write control.
The first NoRule and other responder/missing-field failures, partial captures
and original client retry remain evidence. Unmarked/unknown/duplicate helper
startup returns before environment/config/stdin. Restricted
private MAIN callback/budget/interleaving probes remain uninspected/unexecuted.

The new fixture runtime wrappers and reader tasks use erased types to avoid
duplicating generic runtime code in the large native test binary. The private
key admission lock now ends in a lexical scope at the same point before its
waiter and receiver await. Its checks, three-attempt ceiling, owned handles,
KDF and file work stay identical. The modal still runs on its owning thread;
the erased future's Send bound starts no extra task. No existing assertion,
deadline, compiler flag, dependency, warning setting or bound changes.
First fixture/interface/format failures, the failed Send attempt and compiler
incremental error, and linker warnings are retained. The exact larger consumer
also warns against the unchanged 16 MiB unwind threshold; historical A erasures
or another worker's byte counts cannot certify this union. Canonical reductions
remain separately coordinated and final warning acceptance is still blocked.

This key/B union has no hosted or Linux/Windows validation. The independently
validated installed guard main a903 and its separate main run cover that
earlier source only. Local verification records its exact head in the new
ignored report; source/native review, all four hosted jobs and a separate
merged-main proof remain required
before public delivery. The same $24.285934/$25 ledger with 40 unknowns, SDK/WIT,
dependencies/pins/lock, SQL/schema/frames/goldens, native permissions/limits and
binary/container publication hold remain unchanged.
