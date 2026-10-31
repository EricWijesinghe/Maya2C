# Session 2026-10-31: keyless signing and provenance for releases

Branch `ci/release-signing`, worktree `D:/Maya2C-wt-sign`, off master.
Goal: no single developer machine can ship a release signed by that machine.

## What changed

- `.github/workflows/release-binaries.yml`
  - Release split into two jobs. `sign` holds `id-token: write` and
    `attestations: write` and nothing else write-level. `release` holds
    `contents: write` and `actions: write` and no OIDC token. Build jobs hold no
    write permission.
  - `sign` runs cosign keyless (`cosign sign-blob --bundle`) on every file in
    `dist/` and on `SHA256SUMS`. Bundles are `<file>.sigstore.json`.
  - `actions/attest-build-provenance` (already SHA-pinned) runs on
    `SHA256SUMS`, then the self-check `cargo run -p xtask -- verify-release dist`.
  - Both `sign` and `release` use `environment: release`, so required
    reviewers can be set in GitHub settings (not yet set; see below).
  - Placeholders: `scripts/authenticode-placeholder.sh` (Windows build, wallet,
    chat, hub) logs `UNSIGNED: no key` when `WINDOWS_CODESIGN_PFX_BASE64` is
    absent. Android keystore step logs `UNSIGNED: no key` when
    `ANDROID_KEYSTORE_BASE64` is absent. Both fail the job if the secret exists
    and the signing step is not wired.
  - `persist-credentials: false` on the checkouts in `sign` and `release`.
  - Release notes default text now says files are signed and points at the doc.
- `scripts/authenticode-placeholder.sh` (new, executable).
- `xtask/src/verify_release.rs` (new): `cargo xtask verify-release <dir> [--checksums-only]`.
  Checks SHA256SUMS against files (no unlisted files, refuses path traversal),
  then `cosign verify-blob` with identity pinned to the release workflow on a
  `node-v*` tag, then `gh attestation verify --repo EricWijesinghe/Maya2C`.
- `xtask/src/main.rs`: subcommand registered, usage text added.
- `xtask/Cargo.toml`: `sha2` and `hex` (both already workspace dependencies).
  `Cargo.lock` gained two dependency lines.
- `docs/verify-release.md` (new): what is signed, what is not, per-OS commands
  (Windows PowerShell, macOS, Linux), manual cosign and gh commands, maintainer
  to-do list.

## Pinned actions

`grep -h 'uses:' .github/workflows/*.yml` gives 129 lines. Lines not matching
`@[0-9a-f]{40}`: 0. Every action is pinned to a full commit SHA. The new
`sigstore/cosign-installer` pin is `6f9f17788090df1f26f669e9d70d6ae9567deba6`
(v4.1.2), resolved with `gh api repos/sigstore/cosign-installer/git/ref/tags/v4.1.2`.

## Checks run, with real output

Unit tests (`cargo test -p xtask verify_release`):

```
test verify_release::tests::parses_sha256sum_lines ... ok
test verify_release::tests::refuses_path_traversal_and_bad_lines ... ok
test verify_release::tests::sha256_matches_known_vector ... ok
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 31 filtered out
```

Smoke test of the built binary on a temp release directory (`--checksums-only`):

```
checksums: 1 files match SHA256SUMS
signatures: NOT CHECKED (--checksums-only)
exit=0
xtask: Maya.Wallet_0.1.msi: checksum mismatch (listed ba7816bf..., actual 7571ce1f...)
exit=1
```

Note: the first smoke run exposed that Windows `sha256sum` writes ` *` (binary
mode); the parser now accepts both `  ` and ` *`.

Workflow validation:

- `actionlint` (v1.7.12 via `actionlint-py`, run from `D:/Python312/Scripts/actionlint.exe`)
  on all `.github/workflows/*.yml`: exit 0, no findings.
- `python yaml.safe_load` on `release-binaries.yml`: loads; jobs
  `['build', 'wallet', 'chat', 'hub', 'sign', 'release']`; sign permissions
  `{contents: read, id-token: write, attestations: write}`; release permissions
  `{contents: write, actions: write}`.
- `bash -n scripts/authenticode-placeholder.sh`: ok. Run with `PFX=` empty prints
  `UNSIGNED: no key (WINDOWS_CODESIGN_PFX_BASE64 not set); Authenticode skipped`.
- `rustfmt --edition 2024 --check xtask/src/verify_release.rs`: clean after formatting.

## NOT VERIFIED

- NOT VERIFIED: needs a real tag run. Cosign signing, the `sign` job, the
  attestation, and the `verify-release dist` self-check have not run on GitHub.
  Only a `node-v*` tag run does that, and tagging needs `APPROVED: release`.
- NOT VERIFIED: `cosign verify-blob` and `gh attestation verify` against a real
  release. The verify path has only been exercised with `--checksums-only`.
- NOT VERIFIED: the `cargo xtask verify-release` signature path has no unit test.
  It is a thin wrapper over the two external tools.
- NOT VERIFIED: the `environment: release` protection rules. They are not
  configured in GitHub.

## What remains for Eric

1. GitHub settings: create the `release` environment with required reviewers
   (not the tagger). Without it the environment gate does nothing.
2. Windows Authenticode certificate, then wire `scripts/authenticode-placeholder.sh`.
3. Android keystore, with the Android build job it signs.
4. macOS Developer ID and notarization (not in scope here).
5. One real `node-v*` tag run under `APPROVED: release`, then run
   `cargo xtask verify-release` on the downloaded release.
6. Optional: the in-app updater still checks only its Tauri key, not provenance.
   That is a separate change in `apps/app-update`.
