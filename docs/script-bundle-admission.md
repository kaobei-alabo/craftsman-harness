# Immutable script bundle data admission

`gadgets/script_bundle` implements private validation of supplied preprepared
code bundles. `CandidateBundle` borrows original data; `admit` returns an owned,
immutable `CheckedBundle` only after the whole candidate passes. Checked bytes
remain untrusted executable code awaiting actual session source consent.
Read-only app getters expose original files, metadata, fixed runtime identity and
content digests. Sensitive data types implement no `Debug`, `Serialize` or
`Clone`, and checked fields have no caller constructor or mutable accessor.

The module is currently registered only for gadget-feature unit tests. No
frontend selects it, no session field is added, and no callable/catalog/CLI/config
surface is introduced. Selecting it for an actual source-consent/host path needs
separate reviewed integration. The implementation has no filesystem, compiler,
package resolver, dispatcher or permission dependency.

The fixed backend records are reference data:

| Profile | Image SHA-256 | Bytes |
| --- | --- | ---: |
| JavaScript ESM / TypeScript with in-WASM Sucrase | `244f18978b7a0165660974d558f5d25d04e3e79b6dc2692de10884a49682802a` | 1,381,447 |
| python flat v4 / cpython 3.14.0 | `47b3d888c28d2618fa9c0ac9c7afa062e4dcea1733f36c99fca531c35f02faaf` | 19,741,769 |

Runtime metadata must match the exact profile, image length/hash, engine, loader
source hash and embedded transform identity. Supplied image bytes additionally
must have the exact known digest and length within 32 MiB. Omitting image bytes
retains only the fixed reference; it does not claim an image was inspected.
Identity verification neither compiles nor approves an image.

The first supported representation has these whole-admission limits:

| Data | Limit |
| --- | --- |
| Code files, including entry | 64 |
| Aggregate UTF-8 code | 256 KiB; the JS map additionally counts original name bytes within that same bound |
| Read-only asset data | 64 files, 64 KiB/file, 256 KiB total |
| Opaque package artifact data | 32 records, 256 KiB total |
| Metadata JSON | 64 KiB, depth 12, 4,096 nodes, 256 array items, 32 fields/object, 64 decoded key bytes, 1,024 decoded string bytes |
| Original file names | 16 KiB total; 1,024 bytes/path, 255 bytes/component, depth 8 |
| Source provenance / exact packages | 64 / 32 |
| Declared code and package dependency edges | 256 combined; depth 8 |
| Complete data-view / code-only guest envelope | 1 MiB each after real JSON escaping, base64 and framing |

Original counts and byte lengths are bounded before metadata decoding, hashing
and source copies. A stack-only JSON preflight checks decoded UTF-8 lengths,
surrogate pairs, duplicate keys (including escaped equivalents), depth and work
before bounded deserialization. Strict records refuse unknown fields, authority
flags, malformed digests and unsupported declarations. No clipped source,
metadata, artifact or partial closure is accepted.

JS declarations use exact relative `.js`/`.mjs` names; the TypeScript profile also
accepts executable `.ts` names. It refuses absolute `/skill` names, leading `./`,
traversal, ambiguous/device/reserved names, declaration-only files and unsupported
module kinds. The three `node:fs`, `node:fs/promises` and `node:path` adapters can
be named as image-owned dependency data and cannot be supplied or overwritten.
These adapters do not establish general JavaScript server-runtime compatibility.

python's entry spelling is exactly `main.py`, matching the real code envelope.
Supplied imports are flat ASCII identifiers. This conservative first subset
accepts the private `bundle_…` namespace and the retained `six`/`struct` names;
other names, preloaded/runtime names and package directories refuse without
alias repair. Asset names retain the separate ASCII virtual-file profile. The
known `struct` wrapper declaration must pin cpython 3.14.0 and
declare the image-owned `_struct` dependency. A future actual host must preserve
the guest's real preloaded-module refusal at invocation. Admission does not
prove arbitrary import compatibility or inspect/compile python syntax.

Strict version-1 metadata declares every supplied entry/module/asset/artifact,
its exact original name, SHA-256 and source provenance record. Provenance records
contain original origin/license data and local, exact package or supported
stdlib metadata. Package pins currently require three numeric version components;
ranges, extras, markers, scoped-package resolution and hooks refuse. Every
package has exactly one supplied opaque artifact and provided code/data members.
All declared package references must resolve to exact provided pins, and code
edges across package owners must match those declarations. Unreachable members,
gaps, cycles and duplicate/canonically colliding declarations refuse. Memoized
longest paths bound work even for shared dependency diamonds.

All actual supplied file digests are recomputed. Artifact-to-module provenance
is a bound declaration, not authenticated extraction: there is no archive
opening, fetch, install, host import or general resolver. The defu 6.1.7 and
six 1.17.0 / struct test examples use owned synthetic stand-ins as metadata
fixtures; they execute no third-party package code.

The deterministic length-framed SHA-256 binds fixed runtime/profile/loader/
transform identity, image-data presence, complete original metadata and every
file's original kind/name/order/bytes. Changing original metadata spelling or
order also changes this conservative binding. Case/Unicode keys only refuse
collisions and are never execution aliases. Source BOM/CRLF and arbitrary binary
asset/artifact bytes remain exact; empty files remain present records.
The existing bounded NFKC + lowercase key also drives asset suffix denial and
the same forbidden punctuation/control/component rules used for original names.
Executable-equivalent compatibility suffixes (including fullwidth letters/dots
and long-s), canonical traversal/separators/device names, and fullwidth forbidden
punctuation refuse whole. Valid original Unicode names remain unchanged in the
snapshot, metadata, encoded data view and digest. Code-module suffix acceptance
still checks the original profile spelling; no canonical execution alias is
created. The version-1 canonical repair retains the independent 17-row DATA
controls and their initial failures; it demonstrates denial consistency without
source consent or source/package/VM execution.
Structural data admission does not establish language syntax, type checking,
artifact extraction or runtime import compatibility.

Sizing respects the distinct actual envelopes: JS uses entry/modules/work/input
without a guest version field; python uses version 1/code/modules/fixed `/work`
cwd. These private sizing passes dispatch nothing. Assets/metadata are separate
data records and are never injected as new guest fields or task work. A future
call must admit its complete actual work/input/arguments again.

Source consent, actual human task identity, retained roots and read captures,
VM isolation/compatibility, selected cancellation/owned returns, complete
prospective CRUD review and guarded real apply remain unwired here. Raw data,
digests and provenance labels create none of those authorities. The declined
JS source/compiler/transformer mutation and incremental-build experiment remains
excluded and unproven. This slice changes no framework/SDK/WIT/dependency/lock/
pin/native/SQL/frame/golden/default frontend behavior and makes no paid calls or
publication. This test-only foundation is merged as PR #31 at `29349c5`/tree
`93748936`. Final local17 and independent merged-main
[44567715919](https://github.com/kaobei-alabo/craftsman-harness/actions/runs/44567715919)
passed all four CI contexts/every mandatory step/both modes with complete old/new
inventories, source boundaries and unchanged warnings. Source is installed
locally; no public script/VM/apply route was enabled. Each later runtime slice
still requires fresh full local/native/source and independent-main proof.
