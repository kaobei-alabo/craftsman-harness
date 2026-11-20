# Private reversible privacy vault

`crates/craftsman-rs/src/privacy_vault/` implements private host-data aliases and
an encrypted **local-only** libSQL/Turso-core vault. This is source and storage
code, with synthetic native database tests. No CLI, TUI/ACP session, model,
tool, gadget, hook or unlock flow selects it. Actual keystore authority,
required-hook enforcement and transport/display/file integration remain unwired.

The proposed production file is `config::config_dir()/privacy-vault.db`. The
private API takes an explicit absolute host config root; it does not discover
home/config, create directories, modify user configuration, resolve credentials
or access keychains. Tests use owned synthetic roots. The database is separate
from `threads.db` and `store_files`; their schema/version/migrations/retention
and connection settings remain unchanged. There is no remote Turso client,
replication or service. A foreign layout, preexisting zero-byte/unmarked empty
database, or orphan sidecar at the reserved vault name refuses without adoption.
Only actual create-new ownership can initialize a fresh file. Interrupted
unmarked initialization is not automatically adopted; a key-authenticated schema
that actually committed may reopen even if its original waiter was dropped.

## Host and key boundary

The `Send + Sync` host boundary `HostKeySource::acquire_privacy_key()` supplies an
opaque `HostKey`. This key must
be separate from model API credentials. There is no plaintext key file,
metadata/history/tool-output key, credential fallback, unlock Boolean or
model-supplied authority. These supplied-host APIs **do not establish actual
normal-session, user or OS-keystore authority**; a subsequent integration must
obtain and bind that authority before activating privacy hooks.

The exact already-locked `ring 0.17.14` supplies HKDF-SHA256, HMAC-SHA256 and
ChaCha20-Poly1305 AEAD; `zeroize 1.9.0` owns key/plaintext buffers. Only these two
direct app dependency edges were added. Framework/SDK/WIT dependencies and
existing versions/features remain unchanged. There is no custom cipher.

HKDF separates encryption, equality indexing, scope and alias-generation keys.
SQL receives encrypted payloads and keyed tags, never original values or raw
namespace names. Both alias and original are encrypted. Associated data binds
vault identity, host/project/thread/profile/context scope, category, version and
original/alias tags. Reopening requires the same supplied host key. Wrong keys,
invalid ciphertext or substituted bindings fail with fixed private diagnostics.
Key loss has no recovery fallback. Owned buffers are zeroized; this does not
promise zeroization of ring internals, OS memory or copies made by host callbacks.

On Unix, roots must already be owned/private, and the database/sidecars must be
owned private regular single-link files; symlinks refuse. Windows checks native
file identity, reparse attributes and link count, but does not establish inherited
ACL privacy. Actual host/keystore integration must resolve that Windows boundary.
SQLite still opens a path: same-user external races, native blocking IO and
directory/path replacement are not claimed to be OS containment.
Native ancestor capture uses complete borrowed absolute paths in root-to-leaf
order. Windows drive/verbatim-drive prefixes require their native root separator
before IO; a bare prefix is refused. This preserves each ancestor check without
normalizing the selected spelling. Ten Windows-only owned-path probes cover
ordinary/canonical paths, raw prefix errors, actual identities, commit/reopen,
hardlinks, junctions, replacement and bounds. Their hosted evidence is separate
from local macOS tests.

## Exact categories and immutable scope

`Vault::session(HostNamespace)` captures five exact host names, format version and
a private session lifetime. There is no rebind/reset or mapping-update API.
Persisted mappings are append-only through production code. Repeated identity
uses the same alias within that namespace; other host/project/thread/profile/
context namespaces use distinct aliases, with a global keyed alias uniqueness
index in this vault. These are pseudonyms, **not anonymous data**: category,
equality, lengths and telephone layout remain observable.

The first formats are deliberately narrow:

- Email: ASCII dot-atom local part and a dotted ASCII DNS name, without quoted,
  Unicode or comment forms. Aliases use `pv1-…@privacy.invalid`; `.invalid` is
  reserved. Email identity is exact bytes, including case.
- Telephone: an explicitly selected conservative ASCII layout with 7–15 digits,
  optional leading `+`, spaces/hyphens/dots and at most one parenthesized group.
  Aliases preserve digit positions and separators. Extensions, Unicode digits,
  malformed/consecutive separators and other unsupported forms refuse. There
  is no normalization, phone assignment check or promise that a dummy number
  is unassigned; these tokens are for display/file fields, never dialing/network.
- Literal customer/account/project identifiers: exact **host-declared** values,
  4–256 UTF-8 bytes without control characters. No automatic names, addresses,
  medical records or government-ID coverage is claimed.

API keys/passwords/private keys need a separate irreversible-redaction or
preserve-in-place policy. They are not automatically restored as UI plaintext.
Host declarations are not an automatic sensitive-data classifier.

## Admission, masked text and field records

`MappingSession::capture_text()` captures original UTF-8 bytes and an input SHA256
for the host-only detector. `mask(CapturedText, DetectorReport,
DeclaredIdentifiers)` validates that hash, exact session binding, byte offsets,
UTF-8 boundaries, nonempty/nonoverlapping spans, format and literal declarations
before replacement. Duplicate/ambiguous spans refuse. Unchanged text/BOM/CRLF/
Unicode bytes remain exact. An omitted exact repeated occurrence of a selected
original refuses; coverage of unknown or encoded sensitive values is not implied.
An empty validated report explicitly records zero replacements and unchanged
text, without claiming detector coverage.

Candidate aliases must avoid existing aliases and all original input data. A
collision with an existing stable alias refuses instead of rotating identity.
New candidates have at most 32 attempts. Complete output and scope records are
built together, with original/alias join checks; no partial masked/raw fallback
is returned after an error. **Every new mapping must commit successfully before
`MaskedText::text_for_later_model_request()` becomes available.** The caller must
recheck immediately before its later request; this module sends no traffic and
cannot retract an already-borrowed reference.

`MaskedText::resolution_records_for_host_fields()` and awaited
`MappingSession::resolve_exact_aliases()` provide bounded immutable records.
`resolve_exact_display_field()` resolves only an exact known alias into a
host-display callback; altered/malformed/unknown/foreign aliases refuse without
guessing, substring scanning or case normalization. `with_host_field(index, …)`
provides borrowed alias/original/category bytes for a later file layer to
validate and escape. There is no raw map getter or serialization, and Debug
contains fixed private placeholders. `validate_session_binding()` checks this
private lease only; an alias view is not a read/write/normal-session grant.

A file consumer must separately bind the actual session and authorization,
verify source/content/span hashes, validate the target field and escape its
syntax before staging/review/apply. Whole display replies need a corresponding
validated host-field adapter and the actual renderer's control escaping. There
is no blind shell/code replacement, tool-argument unmasking or arbitrary outbound
request restoration in this slice.

## Bounds and failure limits

Hard ceilings (host limits may only tighten): source and masked text 256 KiB;
256 spans/resolution fields; 512 mappings/scope, 4096/vault and 128 nonempty
scopes; 8 MiB total ciphertext; 64 MiB charged byte work; 4096 IO calls including
one reserved rollback call; 32 candidate aliases/identity. Namespace names are
at most 256 bytes each/1024 combined. Declarations are at most 128 values/32 KiB.
Root paths are at most 16 KiB/128 components; payloads at most 1024 bytes.

The private connection uses WAL/FULL sync, in-memory temporary storage, a
200 ms SQLite busy timeout, 64-page autocheckpoint, a 1 MiB journal retention
target and 8192 maximum 4096-byte database pages. Native checks refuse main DB
over 32 MiB, WAL/journal over 16 MiB or SHM over 1 MiB. Retention settings and
pre/post file observations are not an absolute peak-size guarantee under races.
One operation slot has no pending per-vault queue. Each operation checks a
5-second elapsed bound including worker queue time, around actual IO; started
blocking syscalls are not interruptible or guaranteed to obey an OS deadline.

Actual IO/commit/rollback is awaited. Failed read/write/decrypt/key/bounds closes
the private operation, never returning raw text or a false masked receipt.
Dropping a started waiter closes its session while the worker retains DB/key
ownership through actual return. A dropped/late-failed operation may already
have committed; there is no promised rollback, power-loss guarantee,
cross-process exactly-once, anti-rollback/deletion proof or worker quiescence.
Reopen observes actual durable mappings; it does not replay a model/tool call.

## Verification evidence

Synthetic tests exercise actual libSQL DB/WAL/SHM scans, keyed persistence,
AEAD/scope/name/category/version substitution, repeated/multiple categories,
collisions, Unicode/escaping/ranges, unknown/foreign aliases, size/IO bounds,
real transaction failure/busy locking, dropped pre/post-commit waits and late
elapsed failure. Old tests/assertions/deadlines are unchanged.

`target/verification/privacy-vault/REPORT.md`, `RESULTS.json`, command logs and
source hashes retain the first two new slot warnings and failed setup/fixture/
compiler evidence: nonprivate default tempfile roots, row-returning PRAGMAs
passed to execute, a live WAL-pragma row blocking COMMIT, the first incorrect
BEGIN expectation, and the missing host-source Sync bound. Corrections preserve
every old test/assertion/deadline. Local proof does not substitute for all four required
hosted contexts/every step/both native modes and independent merged-main proof;
those gates remain before integration delivery/merge/install. No paid model
expense, binary/container publication or user config/keychain change is made.

PR #32's first native run
[44602915778](https://github.com/kaobei-alabo/craftsman-harness/actions/runs/44602915778)
is retained as failed evidence:37 vault and27 restoration cases per Windows
mode failed with fixedIo at opening; all old test statuses and source warning
blocks were preserved. The complete-ancestor candidate addresses the source
path construction, while raw Windows syscall confirmation and fresh all-four
proof remain required. Direct libSQL canonical-path cases passed in the failed
run; the fixed production diagnostic alone did not identify a failing syscall.

The corrected-first run
[44629784168](https://github.com/kaobei-alabo/craftsman-harness/actions/runs/44629784168)
executes all applicable vault/restoration cases and all ten opening probes
successfully in both Windows modes. Successful captured libtest cases do not
provide raw numerical syscall/metadata output, and none is inferred. The run
still fails the separate detector compiler fixture; its narrow compiler-discovery
correction needs new complete native proof before merge and delivery. Existing
vault behavior and key/ACL/normal-session integration limits remain unchanged.

Run44645950249 also passes all applicable vault/restoration/opening cases in
both Windows modes. Its sole native failure remains the separate detector host
compiler. The explicit installed-MSVC fixture candidate does not change this
vault; fresh full native delivery and actual key/consumer integration stay open.
