# Source-built privacy detector fixture

This isolated synthetic fixture exercises `craftsman:gadget@0.1.0` through the
unchanged source SDK and actual `GadgetRuntime`. It implements detection and
host-validation evidence only. Production automatic hooks, trusted required
policy, model masking, display/file restoration and vault/key storage are not
activated by this fixture.

The dispatched task explicitly approved this synthetic source before actual
`describe`/`call`. Build trust is separate from runtime approval. The loaded
manifest has no read, write or HTTP capabilities; the detector makes no host,
filesystem, environment or network calls and receives no vault/key/table. Its
`fixture_adversarial` function exists only to test bounded rejection and deadline
handling; it is never a proposed required hook.

`detect` takes `version: 1`, a host `input_binding`, and `nodes` with `id`, `text`
and `input_binding`. It returns version/binding plus spans with `node_id`,
half-open UTF-8 byte `start`/`end`, `category` (`email`/`phone`) and node binding.
No original matched substring or alias is returned. The guest copies correlation
bindings; the host independently recomputes actual original bindings and rejects
stale/foreign claims, offsets, scalar cuts, malformed/unknown fields, overlap,
wrong categories, counts, output size and missing declared matches.

The fixture bounds 64 nodes, 64 KiB text per node, 256 KiB combined text,
128 spans and 64 KiB complete detector output. Overflow is whole-batch refusal,
never clipping or complete-empty evidence. SDK JSON stays 1 MiB, runtime memory
256 MiB and call timeout 30 seconds; Wasmtime stays 48.0.8 and Rust 1.99.1.
The fixture's standalone lock uses the root workspace's already-locked package
versions/checksums and SDK/macro edges, including WASM-only dependencies. The
first demo-derived lock failed CI's offline build at ref-cast1.0.27; that lock
and failure remain historical evidence. Root/SDK/demo/other locks and WIT are
unchanged. Six graph guards prevent the new fixture from drifting outside
that dependency closure. Registry edge sets are exact except for the verified
root-only serde/serde_core feature edges of hashbrown0.17.1 and semver1.0.28;
required omissions that remain reachable through another package still refuse.

Supported email syntax is bounded unquoted ASCII local parts using letters,
digits, dot, underscore, percent, plus and hyphen; dotted ASCII domains with an
alphabetic final label; no consecutive/leading/trailing local dots. Quoted,
internationalized, escaped, encoded or obfuscated addresses are outside this
small detector. Recognition does not validate deliverability or ownership.

Phone layouts include a leading `+` with 8–15 contiguous digits (first nonzero),
formatted NANP `+1 (202) 555-0142`, `+1 202-555-0142`, `+1 202 555 0142`, local
`(202) 555-0142`, `202-555-0142`, `202.555.0142`; London `+44 20 7946 0958`;
Taiwan `+886 912 345 678`, `+886-2-2345-6789`, `0912-345-678`, `0912 345 678`,
`02-2345-6789`. NANP area/exchange starts 2–9; Taiwan mobile starts 9/09 and
the supported international landline area is 2–8. These are layout checks,
not a numbering-plan database. Bare local digits, extensions, different area
lengths/separators, non-ASCII digits and other international layouts may be missed.
All examples are supplied synthetic literals, never user data or contacted numbers.

Customer/account/government/payment IDs, IPs and names need separately reviewed
explicit literals/config patterns and typed host handling later. Telephone-like
order numbers and mailbox-looking code can be false positives. Real numbers,
names/photos/encoded content can be false negatives. This fixture does not
establish complete confidentiality or secret detection. Secret classes must
never be reversibly echoed. Same-category format-preserving aliases belong to
the host vault core; the detector never guesses or restores values.

Same-layout phone aliases have no marker: unfamiliar/altered numbers cannot be
universally recognized as aliases. Restore only exact known current-scope values
in declared fields, with provenance or a conservative refusal policy. Do not
substitute commands/paths/network arguments or turn aliases into authority.

Run native source tests without guest IO:

```sh
cargo test --offline --locked --manifest-path crates/craftsman-rs/tests/fixtures/privacy-detector/Cargo.toml --target-dir target/privacy-detector-fixture
```

Run the actual marked runtime supervisor (isolated app config, no model calls):

```sh
cargo test --offline --locked -p craftsman-rs --no-default-features --features gadgets --bin craftsman gadgets::privacy_detector_tests
```

The supervisor source-builds this fixture for wasm32-wasip2 and launches exactly
one registered argv marker. Unmarked, unknown or duplicate ignored-helper runs
return before fixture/config/path access. The collector uses the original
runtime, ephemeral approval and an independently checked owned image/manifest;
it never writes a user grant, native-code disk cache, SDK or app config.

On Windows, native build scripts and proc macros also require the installed
MSVC linker when the final guest target is WASIp2. The isolated supervisor
captures the actual `ProgramFiles(x86)` compiler-discovery root, falling back to
`ProgramFiles` only when the first variable is absent. It keeps that one bounded
absolute native path in a private test-helper transfer. Only the Cargo compiler
child receives its standard discovery name; Rust's tooling locates the installed
MSVC/SDK tools. The application does not search the registry or a user profile,
and the unchanged guest receives no inherited environment. The original isolated
HOME/config/temp, actual PATH/CARGO_HOME/RUSTC/RUSTDOC/SystemRoot and all
240-second/64-KiB/5-second supervisor and 30-second guest bounds remain.

PR32 run44629784168's Windows `link.exe` diagnostic rejected object-file operands
as filesystem-link operands; it is retained as wrong-linker evidence. It does
not establish a missing LIB or kernel32 failure. Local compiler-environment
and native-linker probes are macOS evidence; fresh complete Windows validation
is required for this discovery preparation. SDK/WIT/source/dependency locks,
production behavior and the frozen CI/verification scripts are unchanged.

Independent review found two gaps in the new compiler input checks. Drive and
verbatim-drive discovery roots now require their physical root separator;
incomplete roots refuse without normalization or fallback. The new compiler
probe counts registered native marker prefixes before decoding their values,
so a malformed native companion cannot disappear beside a valid marker.
The first failed native-marker control is retained. Windows path controls need
actual Windows execution; the preexisting detector selector and old test bodies
remain unchanged.

For create-only detailed records, select the same compiled native test executable
with `--ignored --exact gadgets::privacy_detector_tests::privacy_detector_native_fixture
--nocapture craftsman-privacy-detector=collect`. The actual executable path is
printed by Cargo's `--no-run` output; do not assume a hash. Output goes to
`target/verification/privacy-hook-detector/{calls,image,logs}`. Running collection
over an existing record fails instead of overwriting evidence.
Create the owned empty `logs` directory before collect mode; missing output
directories refuse even when actual calls have completed. The original
collector's base-label fields are historical; use external source/hash/commit
records for current attestation.

The worker report/API/integration map and source bindings live under
`target/verification/privacy-hook-detector/`. Full local/all-four/every-step/
both-mode/native-Windows/inventory/warning/tree/independent-main gates remain
open; local detector proof is not privacy activation, live-model or release proof.
