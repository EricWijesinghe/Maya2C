# ADR-080: Release provenance attestations for every file, threshold signing later

**Status:** Accepted, 2026-11-04. Provenance is in `release-binaries.yml`; threshold (k-of-n) release signing is a follow-up and is not built.
**Date:** 2026-11-04
**Related:** [ADR-054](ADR-054-reproducible-builds.md) (reproducible Linux build), [ADR-032](ADR-032-testnet-hosting.md), [docs/verify-release.md](../verify-release.md), [docs/site/authored/code-signing-policy.md](../site/authored/code-signing-policy.md), `crates/custody-mpc` (invariants 18, 19)

## Context

Zero-trust weakness W4: one key signs releases. Before this ADR the release
workflow had two signing paths, and neither needed a stored key:

- `sign` job: cosign keyless (Sigstore, GitHub OIDC) signs every file, with one
  bundle per file. The Rekor log is public.
- `updater-sign` job: the job that holds `TAURI_SIGNING_PRIVATE_KEY`. It signs
  the in-app update payloads, so a stolen key can push an update, but it
  cannot re-sign the installers or the node binary.

Build provenance was attested for `SHA256SUMS` and nothing else. `gh attestation verify
<file>` looks up an attestation by the file's digest, so a downloaded binary
had no attestation of its own. The site and the verify guide said otherwise,
and that gap is what this ADR closes.

## Decision

1. **One provenance attestation per release file.** The `sign` job runs
   `actions/attest-build-provenance` (pinned at
   `4d101475d8b20a2381f78447822ac1eab6504dd8`, v4.2.2) with `subject-path:
   dist/*`, before cosign, so every installer, archive, SBOM, update `.sig` and
   `SHA256SUMS` is a subject. Each subject gets its own SLSA attestation, signed
   by the job's OIDC identity.
2. **Least privilege.** The workflow default stays `contents: read`. `sign`
   holds `id-token: write` and `attestations: write`. Build, wallet, chat, hub
   and `updater-sign` hold no OIDC token. `release` holds `contents: write` and
   no OIDC token.
3. **One verifier.** `cargo xtask verify-release <dir>` runs the same checks a
   user runs: checksums, cosign bundles for each file and `SHA256SUMS`, then
   `gh attestation verify` for each file and for `SHA256SUMS`. The workflow's
   own `Verify release (self-check)` step runs it before upload, so a release
   with a missing attestation fails before anything is published.
4. **No new secret, no new cost.** Sigstore and GitHub attestations are free for
   a public repository and use the built-in `GITHUB_TOKEN` and OIDC token.

## Consequences

- A release file is verifiable from its own bytes: the attestation names this
  repository, the `release-binaries.yml` workflow and the `node-v*` tag. A key
  that the owner holds cannot produce that record unless it runs the workflow.
- The guarantee is bounded by the workflow. Someone who can push a
  `node-v*` tag and approve the `release` environment can run it. The
  `release` environment's required reviewers are the control for that, and
  they are an owner setting, not something the repository proves.
- Attestations and the Rekor entries are public. Build paths and artifact
  names appear there.

## Follow-up (not built): k-of-n threshold release signing

The remaining single point is the approval that starts a release. The planned
step is a k-of-n threshold over the release manifest, so that no one person's
key, and no one runner, can release alone. `crates/custody-mpc` already
implements threshold custody for keys (invariants 18 and 19). Reusing it for
release signing needs its own ADR, a key-ceremony design, and an audit of the
signing path. Until that ADR is accepted, releases are signed by the keyless
path above and by one owner approval.

Nothing in this ADR adds a key, a threshold, or a signer. The follow-up is
recorded here so the gap is visible, not because it is in progress.
