# Craftsman identity migration

> **Approved:** November 4, 2026 (UTC). The migration covers every case of the
> product name in tracked code/docs and source file/directory names. It is a
> breaking identity migration, not a runtime-feature or release announcement.

## Canonical repository and interfaces

The existing repository is **`trend-mgai/craftsman-harness`**. It was renamed
in place: repository identity, commit history, issues, pull requests and
protection rules are retained.

```bash
git clone https://github.com/trend-mgai/craftsman-harness.git
cd craftsman-harness
```

Existing clones use:

```bash
git remote set-url origin https://github.com/trend-mgai/craftsman-harness.git
```

Current first-party identities are:

| Interface | Current name |
|---|---|
| Primary command | `craftsman` |
| Compatibility launcher | `craftsman-rs` |
| App package/directory | `craftsman-rs` / `crates/craftsman-rs` |
| Framework | `craftsman-harness` |
| Gadget SDK/macros | `craftsman-gadget` / `craftsman-gadget-macros` |
| User/project config | `~/.craftsman` / `./.craftsman` |
| Config/token environment | `CRAFTSMAN_CONFIG_DIR` / `CRAFTSMAN_PLATFORM_TOKEN` |
| Other first-party environment | `CRAFTSMAN_*` |
| ACP custom metadata/methods | `_meta.craftsman.*` / `_craftsman/*` |
| Gadget WIT package | `craftsman:gadget@0.1.0` |
| Trace/result/event namespace | `craftsman-harness/trace@1`, `craftsman-harness/exec-result@1`, `craftsman-harness/exec-event@1` |
| Advisory/release namespace | `craftsman-harness/gadget-benchmark@1`, `craftsman-harness/release-manifest@1` |
| Image identities | `craftsman-workspace-runtime`, `craftsman-harness-service-agent` |
| Framework roadmap | `docs/craftsman-harness-plan.md` |

## Existing data and compatibility

User configuration, credentials, installed gadgets and databases are **not
moved, rewritten, loaded or deleted by this source migration**. A new default
configuration directory is not proof that previous data is absent. To reuse
an existing explicit configuration directory, select its absolute path with
`CRAFTSMAN_CONFIG_DIR`; otherwise back up and deliberately migrate selected
configuration/data into the new directory before running the upgraded client.
Keep permission/grant checks and source/component hashes intact; copying an
identity label is not a new approval.

Git directory/session compatibility aliases preserve earlier absolute checkout
paths. Keep those aliases until their dependent sessions/worktrees have been
deliberately migrated. They are not additional clones. Do not repair, prune or
inspect unrelated frozen/blocked worktrees as a side effect of rebranding.
Git history and sealed evidence are not rewritten. Historical documentation
uses translated current names while retaining original dates, hashes, counts
and failure outcomes. The frozen v0.4.0 corpus data itself is unchanged; path
and documentation labels do not make a new binary its original producer.

Clients consuming custom ACP methods, WIT components, schemas, release assets
or image pins must migrate their identifiers together. Standard ACP framing
remains standard and custom extensions stay declared at initialization.
Desktop protocol v4 gains no new frame shapes. The new schema namespace has
v1 layouts/semantics, but it is not wire-compatible with a prior namespace.
Previously built components require a rebuild for the current WIT package;
no cached component, hash or grant is relabelled into approval.

## Publishing is separately gated

Release asset names and OCI names now use Craftsman. Existing releases, image
tags/digests and installed sidecars are not renamed or republished. A source
build, metadata scan or hosted test pass is not release approval.

The workspace-image workflow requires explicit `platform_repository` and
`sdk_repository` inputs under the repository owner. External repositories are
not silently renamed or assumed to exist. Select a source ref whose Dockerfile
and smoke script support the current CLI/package/asset identities. Configure
`CRAFTSMAN_CI_APP_ID` and `CRAFTSMAN_CI_APP_PRIVATE_KEY` before publishing; this
migration cannot read or copy existing encrypted secret values. Its token
still selects only the app plus the two explicitly selected source repos.

The service-agent workflow requires explicit post-migration release/image tags
and checksum-verified `craftsman-linux-*` inputs. Old published tags are not
assumed to contain the new assets. No workflow dispatch, tag, image publication,
secret mutation or change to branch-protection requirements is performed here.

## Implementation and validation boundaries

The name change does not expose `craftsman exec`, fix OPEN #138, settle usage,
enable callable headless gadgets or approve P2g/live/default-on/release work.
The pinned Rust/Wasmtime versions, dependency versions/features, permission
rules, provider control, native-query ceilings and all old test deadlines stay
unchanged. Golden/schema identifiers and model-visible branding are translated
consistently, not regenerated to hide unrelated behavior changes.

The migration requires locked checks, namespace/CLI/contract tests, existing
golden and native suites, and every mandatory step of all four hosted jobs in
both modes. Old failed local runs remain failures; a rename/hosted pass is not
a root-cause fix. See [runtime-status.md](runtime-status.md) and
[headless-mode.md](headless-mode.md) for private and planned work.

## Validation checkpoint — November 4, 2026

Both locked all-target compilation modes and the new CLI/config/SDK/WIT/schema
identity tests passed without warnings. Existing golden-frame, golden-request
and native headless-contract suites also passed. A case-insensitive tracked
source/path scan found zero previous-brand occurrences; dependency versions,
checksums/features, native limits and original corpus data are preserved.

The first renamed full local runs remain **FAILED**: feature-off1585 passed /
6 failed /7 ignored; gadgets1728 /6 /9. Five native shell cases reproduced the
open #138 class, and one exact expected environment-lookup order reflected the
previous identity's collation. Only that two-name expectation was transposed
to the current lexicographic order; the production capture/no-scan behavior
and every lookup/count assertion remain intact. Its focused test passes in
both modes. The failed full invocations are not retried or replaced by those
focused passes. PR145's corrected-head CI41803187695 and its independent automatic
merged-main CI41808669282 (attempt1) now pass ALL FOUR jobs/EVERY setup, mandatory
and post step/BOTH modes. Raw exact named/pass/ignored/doctest inventories and
warning locations match; macOS coverage is actual15, not the retained required
context14. Existing Windows9 compiler-warning signatures/17 occurrences remain
unchanged; no new warnings. The squash tree7370fb412666c165c7a68c1d64fe4efba518b7f3
matches the verified PR tree, and both canonical live checkouts were advanced to
it without mutating user data/history or frozen/blocked worktrees. Final source
scan remains364 tracked files/zero previous-brand content or path occurrences.
No CI retry/dispatch occurred. This completes the identity-source validation gate,
NOT OPEN #138, usage/settlement/public exec/callables/P2g/live/default-on/release or
the externally configured publishing prerequisites above.
