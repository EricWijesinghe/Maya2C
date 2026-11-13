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

## Amendment 2026-11-14: in-app updates are manual until k-of-n

**Decision (Eric 2026-11-14, founder-brief audit Q23, option A).** The
in-app updater (`apps/app-update`, used by the Hub, Maya Wallet and Maya
Chat) does not check, download or install anything on its own. The single
updater key is the reason: whoever holds `TAURI_SIGNING_PRIVATE_KEY` could
push code to every install at once, and an automatic check would deliver it
without anyone looking.

What the apps do now:

1. `maya_app_update::start` registers the updater plugin in a signed build
   and stops there. No background check.
2. A **Check for updates** button (Hub settings, Wallet network settings,
   Chat settings) calls `maya_app_update::check_for_updates`. A build with
   no updater config says that updates are off.
3. When an update exists, a native dialog names the version and the
   publisher key's fingerprint: SHA-256 of the raw minisign public key, in
   groups of four hex digits (`verify::fingerprint`). The minisign key id is
   not used as a fingerprint, because it is chosen at random and another key
   can carry it. Nothing downloads until the user presses **Install now**.
4. After the download, `verify::verify` checks the signature against the
   key compiled into the app, in addition to the plugin's own check. An
   empty signature, a signature by another key, a changed download and an
   unreadable signature are refused. Each case is a unit test in
   `apps/app-update/src/verify.rs`.

**Plan for k-of-n (not built).** Automatic checks return only when all of
this exists:

- A release manifest (version, file names, SHA-256 of each update bundle)
  signed by k of n release keys held by different people on different
  machines. The apps carry the n public keys and k, and refuse a manifest
  with fewer than k valid, distinct signatures.
- The update bundle is accepted only if its hash is in that manifest, so
  the single Tauri key alone can no longer ship an update.
- Rotation: a manifest signed by k current keys can replace the key set,
  so losing one key does not strand installs.
- The key ceremony and the signing path get their own ADR and an audit,
  as the follow-up above already says. `crates/custody-mpc` is a candidate
  for the threshold; separate keys with k-of-n counting are the simpler
  option and need no new cryptography.

Until then the owner publishes the fingerprint of `apps/updater-pubkey.txt`
in `docs/verify-release.md` when the key is created, so a user can compare
it with the one the dialog shows.
