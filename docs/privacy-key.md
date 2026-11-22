# Private key and trusted policy ownership

This is an unselected Step A foundation, not an enabled privacy feature. There
is no `privacy init`, `unlock`, `status` or `enable` CLI route and no automatic
unlock popup. The private app owners are `privacy_key::KeyManager`,
`privacy_policy::CapturedPolicy` and `privacy_cli::PrivateKeyOperations`.
Normal sessions retain their existing behavior when user privacy policy is
absent or disabled. An enabled user policy refuses as not yet available before
frontend/store/provider initialization; malformed policy does not turn it off.

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
`close_and_wait` waits for actual handles; it makes no absolute time, rollback,
OS quiescence or billing promise. Returned unlocked vault/key owners are closed
separately on their explicit close or Drop. No lifecycle/concurrency probe is
substituted for the restricted private MAIN scenarios.

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

This schema is a proposal constraint, not a working activation example. IDs and
function names are restricted ASCII up to 64 bytes; each pin is exactly 64
lowercase hex characters. There are at most 64 distinct nonempty literal
identifiers, each at most 256 UTF-8 bytes, 16 KiB combined. The policy's encoded
data is bounded to 32 KiB and its user source to 1 MiB. Unknown/duplicate policy
fields, malformed types/pins, unsupported stronger storage/opaque requirements
and missing enabled detector refuse. Disabled optional values must still be
valid. Literals are plain user policy data, not passwords/keys or wildcard
authority. Source/capability consent and real approval must precede any
describe/compile/call; neither a pin nor this owner supplies that approval.

## Remaining real wiring

The new zeroizing TUI secret field renders bullets through SafeBackend and
never enters UiEvent/composer/history. It reads no stdin and is unselected;
these controls do not prove terminal echo mode. A real popup must borrow the
actual TerminalGuard's retained terminal/input ownership, receive direct owned
events, run the tracked worker and refuse before session construction. CLI
selectors, that non-echoing reader/popup, initialization confirmation flow and
the final activation gate still need coordinated implementation/review.
Escape/Ctrl-C clears the owned secret and returns an explicit cancellation;
it is distinct from pending editing and from an ordinary empty/no-answer event.

The corrected source/view assignment requires the actual fresh app/checkpointer
thread id and captured canonical workspace, one owner for main/delegated/
compaction. It forbids an independent conversation id or constant thread to
avoid 128-scope capacity; existing capacities/exhaustion remain truthful.
Reopen/model transfer and bound void set_thread_id must refuse/close before
rebinding without replay. Step A does not mint that conversation binding.

Canonical accepted input/results/history/journal/checkpoints remain actual
truth. Only a separate source-derived model view is masked. This key envelope
encrypts vault map records, not ordinary history; no placeholder-only export or
at-rest privacy claim is made. Normal privacy/masking/restoration/source-view
activation, user-key UX, full combined/native all-four validation and separate
merged-main proof remain later gates. The original paid ledger and binary/
container publication hold are unchanged.
