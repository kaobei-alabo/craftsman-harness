# Service-agent OCI image

`service-agent.Dockerfile` packages checksum-verified `craftsman-linux-*`
release inputs into a minimal multi-architecture image. The workflow targets
`ghcr.io/trend-mgai/craftsman-harness-service-agent:<tag>`. The November 4, 2026
identity migration changes binary/asset/image names; existing published releases,
image tags and consumer pins are not republished or relabelled. Select explicit
post-migration release/image tags once those release inputs actually exist.
No image is published by the source migration. See
[the migration notes](../docs/repository-name.md) for external prerequisites.
The image contains no platform credentials; addon companions supply only their
own scoped runtime broker configuration.

The runtime is Debian Trixie because the released Linux binaries need GLIBC
2.38 and 2.39 symbols. The build runs `craftsman --version` as UID 65532, so a
binary that cannot start fails the build. The verifier stage also refuses a
binary that still bundles `emSign ECC Root CA - G3`, `emSign Root CA - C1` or
`emSign ECC Root CA - C3`, the roots that `webpki-roots` 1.0.11 removed after
the eMudhra intermediate compromise (CVE-2026-111943; RUSTSEC-2026-0381, -0382
and -0383). CLI v0.2.12 is the first 0.2.x release without them, and it also
carries the distrusted-key list of `rustls-webpki` 0.103.16.

| Tag | CLI release | Runtime | Notes |
|---|---|---|---|
| `v0.2.9` | v0.2.9 | Debian Bookworm | The binary cannot start (GLIBC 2.36); trusts the emSign roots |
| `v0.2.11-runtime1` | v0.2.11 | Debian Trixie | Trusts the emSign roots |
| `v0.2.12-runtime1` | v0.2.12 | Debian Trixie | Historical pre-migration publication; not new-name asset availability |

To publish a tag, dispatch the workflow from `main` with an existing release
post-migration tag and a new image tag, for example
`gh workflow run service-agent-image.yml --ref main
-f release_tag=POST_MIGRATION_RELEASE -f image_tag=NEW_IMAGE_TAG`. The
`release: published` trigger does not run for releases that `release.yml`
publishes, because GitHub starts no workflow for events made with the workflow
token. The job prints the pushed index digest. Consumers pin that digest, so a
new tag reaches them only when their pin changes.
