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
A private clear closes the old scope before reserving and opening a new one.
It does not raise the 128-scope ceiling, prune, reopen, transfer or replay.

The stable B575 admission interface masks with DeclaredIdentifiers::none(). A
nonempty actual user identifier policy therefore refuses before unlock; it is
never quietly narrowed. The feature-off composition also refuses a detector
selection; standalone private key initialization/unlock remains typechecked
and available only through the owned private operation.

## Full Agent construction remains unavailable

B575's selection is a bare fixture statement permitting a limited set. Its
builder removes middleware that does not declare source support, including
protected call repair and checkpoints, and refuses the three native wires.
This bootstrap does not use that statement as production policy, omit expected
components or fabricate a successful Agent. The full-selection interface must
consume and retain the actual captured policy/key/conversation owner, use its
same reserved id for Agent/checkpointer and every lane, preserve mandatory
component/transport/source capability checks, and refuse before effects while
any required adapter is missing.

Typed PrivacyView turn/steer/decide, the actual accepted human input/decision
producers, native signed items, tracked mutators, compaction/delegation,
checkpoints, HITL, display and file restoration remain integration work. An
eventual terminal /clear requires a new owner and full build; an old void setter
cannot do it. Reopen, model transfer and replay are unsupported in this
terminal/fresh-only first version. No at-rest/history masking claim follows:
actual accepted input/results/effects/history/checkpoints/usage stay exact and
only a separate model view is masked.

The new fixtures use exact argv-marked native PTYs, private temporary roots,
synthetic passwords, real KDF/envelope/vault IO, and an approved source-built
WASM detector. A separate full ordinary builder/loopback/checkpoint control
after successful popup acquisition checks password isolation with privacy
unselected; it is not full-set selected-bootstrap coverage. Unmarked/unknown/
duplicate helper startup returns before environment/config/stdin. Restricted
private MAIN callback/budget/interleaving probes remain uninspected/unexecuted.

This key/B union has no hosted or Linux/Windows validation. The independently
validated installed guard main a903 and its separate main run cover that
earlier source only. Full local verification on the final union, source/native
review, all four hosted jobs and a separate merged-main proof remain required
before public delivery. The same $24.285934/$25 ledger with 40 unknowns, SDK/WIT,
dependencies/pins/lock, SQL/schema/frames/goldens, native permissions/limits and
binary/container publication hold remain unchanged.
