# CI validation and cost controls

CI uses four full validation contexts: `fmt`, `test (ubuntu-24.04)`,
`test (macos-15)` and `test (windows-2025)`. Before merge, every required
platform must complete the default voice/gadget build, contract/provenance
checks, both complete native test modes and unchanged-golden check at the
current head. A skipped job or cache hit does not establish that evidence.
Merged main still receives independent full validation. Binary/container
publication remains disabled.

Linux prerequisite downloads use Ubuntu's official HTTPS archive over IPv4,
with up to three APT transport retries. An incomplete package index still fails
the job through `--error-on=any`; the voice packages and every build/test step
remain mandatory. Merged-main run 43458891311 failed this setup step when the
package endpoint attempted an unavailable IPv6 route. Its passing native suites
do not turn that failed prerequisite into a successful job; the failed run is
retained separately from validation of the corrected workflow.

PR #22 run `43608477313` at `7dfa658` later failed the same mandatory setup
step because the kernel.org endpoint's IPv4 HTTPS connection timed out for all
four Ubuntu suites. The workflow now selects `archive.ubuntu.com`, retaining
the original suites/components/signing configuration, IPv4 transport retries,
`--error-on=any` and every package/build/test requirement. A local read-only
probe received the official archive's index response; fresh native CI remains
required and that probe is not runner validation. The failed setup and any
tests that continued after it remain separate from a passing full job.

## Draft and full validation

| Event | Validation |
| --- | --- |
| Draft PR opened, code updated or reopened | Formatting and small cache/workflow contract tests on Linux |
| PR marked ready for review | Fresh full four-context validation |
| Ready PR updated or reopened | Fresh full four-context validation |
| PR converted back to draft | Light validation; supersedes any running PR validation |
| Push to main | Full four-context validation |
| Manual workflow dispatch | Full four-context validation, even when the ref has a draft PR |

The workflow explicitly includes `ready_for_review` and `converted_to_draft`
events. Only an actual draft flag on a pull-request event skips the native
matrix. Main and manual events select full validation. Drafts must be marked
ready and the resulting full run audited before merge; the earlier light run
is not a release/readiness result. All existing platform labels, native commands,
assertions, deadlines and the failed-core/independent-gadget behavior are retained.

Superseded PR runs are cancelled through the existing concurrency group.
Main and manual runs remain uncancelled by that policy. While editing, keep a
PR in draft and push coherent changes; mark it ready when full validation is
needed. Reducing draft runs does not change the final verification gate.

## Cargo dependency caches

First-party `actions/cache/restore@v4` and `actions/cache/save@v4` restore
downloaded registry data and a staged dependency compiler cache. The key includes
OS, architecture, the pinned toolchain, Cargo lockfiles/manifests, Cargo config,
the workflow and cache helper version. There is no prefix fallback across
different locks, toolchains or platforms. The resolved restore key is reused on
save so fixture-generated files cannot change the key after tests.

`scripts/ci-cargo-cache.py` selects dependency libraries, dep-info, native build
folders and fingerprints. It excludes current workspace packages/targets,
agent/test executables, incremental data, gadget fixture outputs, user config
and credentials. The compiled artifacts are capped at **2 GiB per platform**;
complete files beyond the cap are skipped. Registry downloads are cached
separately. This avoids archiving an accumulated whole `target` directory.

Only successful main platform jobs can stage and save a new cache. PRs restore
trusted main caches but do not save caches; this limits storage churn and
prevents a PR from supplying build artifacts to main. Cache misses or optional
cache failures fall back to compilation. An exact hit is not saved again.
Every required build/test step still executes. The helper preserves Cargo
freshness timestamps, re-filters workspace artifacts on restore and leaves
existing current outputs intact.

Gadget components and cold authoring fixtures continue to build in their own
isolated target directories. Their capabilities, approvals, source provenance
and deadlines are unchanged. The advisory benchmark workflow is unchanged.
The cache does not contain Craftsman agent binaries or test-result evidence.

The first full PR/main runs can still be cold. Savings begin once a successful
main run seeds each platform cache; measured hosted savings require both cold
and warm runs. Caching reduces future runner usage, but does not resolve failed
payments or a GitHub Actions spending-limit block.

## Local verification

`python -I -B scripts/test-ci-cargo-cache.py` checks dependency/workspace separation,
bounded complete-file copies, create-only staging, freshness, restore precedence,
link refusal and the workflow's draft/full/cache-save contracts. Formatting runs
it on every CI event. YAML structure was also validated locally using an
isolated tooling environment, without adding a workspace dependency.

An isolated offline Rust 1.99.1 probe staged/restored actual dependency artifacts,
then rebuilt: Cargo reported external dependencies fresh while recompiling the
workspace program. Local proof is retained under
`target/verification/ci-cost-reduction/`. Hosted restore/save has since been exercised by successful public main runs,
including the November 13 public-repository validation and November 16 run
`43799188076`. Earlier billing-blocked attempts are retained historical failures.
The repository is public and standard GitHub-hosted runner compute is free;
cache size/storage/concurrency limits and the publication hold remain.

The active `Protect` ruleset currently guards main against deletion and
non-fast-forward updates. It does not yet enforce the four required status
contexts server-side; the engineering merge gate above remains mandatory.
