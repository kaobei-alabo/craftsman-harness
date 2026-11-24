# Private key and trusted policy ownership

This is the Step A key and policy foundation. On this unmerged branch,
`craftsman privacy status`, `init` and `enable` select its explicit
operations, and the eligible interactive `craftsman` invocation asks for the
vault password when the actual user file enables a policy. There is no
`unlock` command. The private app owners are `privacy_key::KeyManager`,
`privacy_policy::CapturedPolicy` and `privacy_cli::PrivateKeyOperations`.
Normal sessions retain their existing behavior when user privacy policy is
absent or disabled. For every frontend other than the interactive terminal an
enabled user policy still refuses before frontend/store/provider
initialization; malformed policy does not turn it off.

The [private startup options component](privacy-startup.md) also retains actual
parsed terminal overrides and the existing host allow-list source. Its opaque
approval-start owner derives one source-bound live state with unchanged normal
precedence; it is neither a detector/source grant nor normal activation. The
captured private entry now takes that owner and checks it against its own
bootstrap before any key work; ordinary captured terminal cases cover it.
The interactive router now enters through it; hosted and native proof remain
separate work.

## Password envelope

The vault gets a random 32-byte key, independent of model API credentials. Only
its authenticated password envelope is persisted as `privacy-key.v1` beside
`privacy-vault.db`. The envelope is exactly 112 bytes:

| Offset | Bytes | Meaning |
| --- | --- | --- |
| 0 | 8 | `CPVKENV1` |
| 8 | 4 | Big-endian version 1 |
| 12 | 4 | Big-endian KDF id 1, PBKDF2-HMAC-SHA256 |
| 16 | 4 | Big-endian iteration count 600000 |
| 20 | 32 | Random salt |
| 52 | 12 | Random ChaCha20-Poly1305 nonce |
| 64 | 48 | Encrypted 32-byte vault key and 16-byte tag |

The existing locked ring, getrandom, zeroize and unicode-normalization crates
implement this; dependencies/features/pins/lock do not change. Every header
byte, including the nonce, is authenticated after the fixed domain
`craftsman/privacy-key-envelope/v1\0`. Version 1 accepts only 600000 iterations.
Both the original UTF-8 passphrase and its NFC form must be 12–1024 bytes. There
is no trim, replacement decoding or credential/environment fallback. The byte
floor is a policy choice, not evidence of password strength; PBKDF2 is not
memory-hard. A wrong password/corruption fails authentication; a correct
envelope for another vault fails that vault's existing authenticated header.

Passphrases, normalized bytes, temporary encodings, wrapping-key material and
vault keys use owned zeroizing buffers. Public Debug/error text is fixed and
contains no secret, path or native/SQL diagnostic. Nothing serializes those
owners into config, argv/environment, agent/tool/model state, history,
checkpoints, OSC, journals or panic messages. This clears owned buffers; it does
not establish resistance to the OS, malware, keylogging, swap/crash dumps,
weak passwords, or erasure of foreign/library copies.

## Ownership and explicit operations

Private capture uses the actual app user config root. It retains no-follow
directory descriptors from the absolute root to the leaf, rechecks their
identities, and refuses symlink/non-private/foreign roots. Unix requires a
current-user private root and regular single-link 0600 key/vault files.
Root-owned sticky temporary ancestors are permitted for owned fixtures; that
does not grant a foreign directory or file. Named-file and descriptor identity,
size, modification/change time and complete EOF reads are checked.

Explicit private initialization requires both names and all vault sidecars to
be absent. Envelope and vault creation are exclusive. Existing, partial,
foreign or raced files are not overwritten, adopted, deleted or repaired.
Cancellation/failure can leave one or both files; the next init refuses them.
There is no automatic recovery, retention/pruning or passphrase-change route.
Presence status reads metadata only and is not an unlock/integrity/privacy
certificate. This implementation never initializes the real user directory or
OS keychain during tests.

Unlock reads the envelope without writing it and calls additive
`Vault::open_existing`. That entry suppresses primary-file creation both in
native open and in SQLite open flags, validates the existing exact layout/key,
and never migrates or adopts an unmarked empty database. Existing connection
WAL/sidecar behavior and all old SQL/bounds remain; existing-only does not mean
an immutable read-only mapping store. Original `Vault::open` behavior stays.

Windows secure key-file creation/unlock is explicitly unsupported because the
locked enabled capabilities do not establish owner/DACL privacy. Unix mode
0600 or Windows read-only attributes are not substituted as DACL evidence.
The existing generic vault APIs retain their previous Windows behavior/limits.

Each key manager admits at most three operations and one owned worker at a
time, with no reset/queue. A process-wide private gate allows only one started
KDF. Started blocking KDF/IO cannot be interrupted; dropping a waiter closes
future admission and the worker retains its owned data through return. The
manager retains pending join handles, including if cleanup waiting is dropped.
The admission mutex now ends in a lexical scope at the same point before the
waiter is created and its receiver is awaited. This permits Send erasure of
the new owned fixture futures without changing checks, attempts or KDF work,
and starts no extra secret worker.
`close_and_wait` waits for actual handles; it makes no absolute time, rollback,
OS quiescence or billing promise. Returned unlocked vault/key owners are closed
separately on their explicit close or Drop, including through a poisoned key
guard. Source liveness checks no longer copy the key. The actual 32-byte vault
key remains resident for the required KeySource contract until close; derived
vault material can remain with existing references until their release. This
is revocation and owned-buffer cleanup, not erasure of foreign copies.
The popup performs one operation per entry and closes that manager afterward;
the three-attempt manager ceiling is not a three-try UI promise. No lifecycle/
concurrency probe substitutes for the restricted private MAIN scenarios.

## Actual trusted policy

Only the actual user `config.json` before project merges is a policy source.
The private captured owner retains the real root/file descriptors and does not
accept a merged Config, caller Boolean/string/hash/status, project manifest,
gadget label or model output as authority. Policy changes/replacement and a
file appearing after captured absence fail its source recheck.

```json
{
  "privacy": {
    "enabled": true,
    "detector": {
      "gadget": "privacy-detector",
      "function": "detect",
      "component_sha256": "64 lowercase hex characters",
      "manifest_sha256": "64 lowercase hex characters"
    },
    "identifiers": []
  }
}
```

This schema is a proposal constraint, with placeholder pins. `craftsman
privacy enable --detector <id>` writes this member with the pins of the
installed, approved detector. The interactive route also needs a created key,
`voice.enabled:false` and no identifiers; see
[privacy-startup.md](privacy-startup.md). IDs and
function names are restricted ASCII up to 64 bytes; each pin is exactly 64
lowercase hex characters. There are at most 64 distinct nonempty literal
identifiers, each at most 256 UTF-8 bytes, 16 KiB combined. The policy's encoded
data is bounded to 32 KiB and its user source to 1 MiB. Unknown/duplicate policy
fields, malformed types/pins, unsupported stronger storage/opaque requirements
and missing enabled detector refuse. Disabled optional values must still be
valid. Literals are plain user policy data, not passwords/keys or wildcard
authority. Source/capability consent and real approval must precede any
describe/compile/call; neither a pin nor this owner supplies that approval.

## Private owned input and remaining wiring

The [private password popup](privacy-bootstrap.md) now owns actual input from
the descriptor TerminalGuard retained before raw entry. It renders bullets
through SafeBackend and keeps secret buffers outside UiEvent/composer/history.
Real initialization asks for confirmation; unlock authenticates the existing
envelope and vault. Escape/Ctrl-C or an owned shutdown signal returns explicit
cancellation, closes key admission and awaits started handles. This differs
from pending editing and an ordinary empty/no-answer event. Every prompt/work
return or abandonment clears its owned decoder; TCIFLUSH on the SAME retained
sole-reader descriptor discards all queued type-ahead, including input intended
for what follows. Bytes arriving after that flush are outside the observation.
Work ticks discard at most 64 raw bytes without spending the unchanged 16 KiB
prompt-validation allowance. Started work is still awaited, not interrupted.

The private composition captures actual user policy before project merges,
retains the same root with the key owner, reserves the application's fresh
thread id and captured canonical workspace, and opens PrivacySession with a
user-policy-pinned WASM detector. Stored exact source approval remains required
before compilation/describe. A nonempty declared-identifier policy refuses
because the stable admission interface currently supplies none; it is not
silently omitted. Pre-build clear stops the old scope and reserves another;
every failed/abandoned clear closes both session and key. Once an Agent has
been built, private `/clear` closes the old conversation/key before waking
old prompts and awaits its actual task/background/LSP owners. A sole normal
reader transfers input only after its synchronous poll/read has returned;
up to 32 available decoded ordinary events are retained in the same UI before
the password is requested. The popup then authenticates an existing key/vault
again through the old retained root/workspace authority. Only a successful
unlock reserves another actual id and builds the complete fresh session.
Cancellation or any failure ends the entry without starting a replacement or
disabling privacy. The 128-scope ceiling and exhaustion remain unchanged.

The unselected owned terminal entry now requests the complete actual terminal
extras and retains/pumps its real receiver through construction and turns.
Its typed tasks hold the same view/agent/key-policy owners while the UI drains
the actual notification receiver continuously. Questions and opaque core
request/presentation/decision approvals go through actual input and SafeBackend.
No limited-set fixture permission is used as user policy. The first current
owned Chat/PTY check passes without file restoration, including an ordinary
constant write, exact input/checkpoint retention and active-save refusal.
Three additional ordinary macOS cases pass for fresh clear, Escape/Ctrl-C
cancellation, and a decoded ordinary draft retained across password input.
They retain config/key bytes, restored terminal modes, exact accepted human
records in distinct threads, and password absence from capture/wires/store.
The first clear case lost its queued committed reply in the UI; its failed
capture remains evidence. Completion now enters the same event FIFO after
the committed reply, so idle state cannot discard that earlier queued event.
Voice must be explicitly disabled in this private v1 entry; unsupported
configured parts and native opaque items refuse rather than being omitted.

Captured `file_data` declarations name an absolute workspace/destination,
`plain_text` or `json_document`, and both `write_file`/`edit_file` operations.
They have 16-rule, 4096-byte path, 128-component and existing policy/source
bounds, retain no-follow parent chains, and reject changed or symlinked parents.
These are INERT proposals: path/format/purpose/pointer alone grants no secret
restoration. The actual containing-object/schema/application/data-field guard,
semantic exclusions and retained relative-create/held-update consumer are
pending the one canonical opaque interface. Config, credentials, key/vault,
instructions, schema, source and executable application content must remain
excluded. The successful ordinary file check is not restoration acceptance.

The configured user root is refused as a data location by the directory its
owner holds, not only by how a rule spells it. Each rule retains every
directory on the way to its destination; if the device and inode of one of
them are those of the root's own held final directory, the policy is invalid
at capture and at every recheck, and a lookup for that rule's destination
refuses. So the root in another letter case on a volume that ignores case, by
the macOS data-volume path, or after it was renamed, is still the root. The
pathname-prefix and protected-name refusals stay as further checks; a
destination no rule names has no retained directory and keeps only those. The
comparison reads two held descriptors: it hands out no descriptor, path or
identity, opens nothing and grants nothing. A rule inside the root under the
root's own spelling used to be captured and refused at each lookup; it now
fails capture as an invalid policy. Where a platform has no second spelling
that opens without following a link, only the renamed-root case exercises the
comparison. The two helpers that read a format from a workspace and
destination path alone exist in test builds only; a production build has no
such answer, and the legacy restoration method still returns nothing.

Normal init/status/enable and /privacy selectors, password entry during live
work without retiring its owners, guarded data restoration and full native/source/security
validation remain pending. Reopen/transfer are unsupported; void setters
cannot replace a fresh owner and full rebuild.

Canonical accepted input/results/history/journal/checkpoints remain actual
truth. Only a separate source-derived model view is masked. This key envelope
encrypts vault map records, not ordinary history; no placeholder-only export or
at-rest privacy claim is made. Normal privacy/masking/restoration/source-view
activation, user-key UX, full combined/native all-four validation and separate
merged-main proof remain later gates. The original paid ledger and binary/
container publication hold are unchanged.
