# Verify a release

Every file in a `node-v*` release is signed by the release workflow itself,
with no signing key stored anywhere. You can check three things, and each
one is independent:

1. **The checksum.** The file is exactly the one listed in `SHA256SUMS`.
2. **The signature.** The file was signed by `release-binaries.yml` running on a
   `node-v*` tag in `maya2c/Maya2C` (named `EricWijesinghe/Maya2C` before 2026-11-24; older releases carry that name and still verify), and the signature is in the public
   Sigstore transparency log (Rekor).
3. **The build provenance.** GitHub recorded which commit and workflow built
   the file.

A release also contains `<file>.sigstore.json` for each file and for
`SHA256SUMS`. Those are the signature bundles. Keep them next to the file.

## What is and is not signed

| Thing | Signed how | Status |
|---|---|---|
| Every release file, `SHA256SUMS`, the SBOM | cosign keyless (Sigstore), bundle per file | On every `node-v*` release |
| Every release file and `SHA256SUMS` | GitHub build provenance (SLSA, Sigstore), one attestation per file, `gh attestation verify` | On every `node-v*` release |
| Windows `.exe` and installers | Authenticode | **Not yet.** No certificate. The log says `UNSIGNED: no key`. Windows SmartScreen warns. |
| macOS `.dmg` | Developer ID and notarization | **Not yet.** Gatekeeper warns. |
| Android | Keystore | **Not yet.** No Android build exists in this workflow. |

A keyless signature proves who built the file and from which tag. It does not
prove the code is safe. Read the source, and check the SBOM if you care which
dependency versions shipped.

## One command (any OS, from a checkout of this repo)

Install `cosign` and `gh` first (see the per-OS notes below), then:

```
cargo xtask verify-release <folder with the downloaded files>
```

It checks the checksums, then the signatures, then the provenance, and it
stops at the first failure. `--checksums-only` skips the last two, which need
the internet and the two tools. The command is in `xtask/src/verify_release.rs`.

## Check it by hand

For a file named `maya2c-node.tar.gz`:

```
cosign verify-blob --bundle maya2c-node.tar.gz.sigstore.json \
  --certificate-identity-regexp '^https://github\.com/(maya2c|EricWijesinghe)/Maya2C/\.github/workflows/release-binaries\.yml@refs/tags/node-v.+$' \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com \
  maya2c-node.tar.gz

gh attestation verify maya2c-node.tar.gz --repo maya2c/Maya2C
```

Do the same for `SHA256SUMS` with its own bundle and its own attestation
(`gh attestation verify SHA256SUMS --repo maya2c/Maya2C`). Then check
the checksum (below). Each command prints an error and exits non-zero on any
mismatch.

The provenance check does not rely on this page or on the release notes: the
attestation names the repository and the workflow that built the file. A file
built elsewhere, or uploaded by someone else, fails `gh attestation verify`.

## Per operating system

**Windows (PowerShell).** Checksum:

```
Get-FileHash .\Maya.Wallet_0.1.0_x64-setup.exe -Algorithm SHA256
```

Compare the `Hash` value with the line for that file in `SHA256SUMS`. Ignore
case. Install the tools with `winget install sigstore.cosign GitHub.cli`, then
open a new terminal. Run the `cosign` and `gh` commands above.

**macOS (Terminal).** Checksum:

```
shasum -a 256 -c SHA256SUMS --ignore-missing
```

Install the tools with `brew install cosign gh`. Intel and Apple-silicon
downloads are both signed the same way.

**Linux (any shell).** Checksum:

```
sha256sum -c SHA256SUMS --ignore-missing
```

Install the tools from your package manager, or from
<https://docs.sigstore.dev/cosign/system_config/installation/> and
<https://cli.github.com/>.

`--ignore-missing` skips files you did not download. Each checksum line must
say `OK`. Any `FAILED` line means the file is not the one that was released.
Delete it.

## What the in-app updater checks

The Maya Wallet, Maya Chat and Hub updaters check a separate signature, made
with the Tauri update key (`apps/updater-pubkey.txt`). That is not the
keyless signature described here, and the updater does not check the build
provenance. To verify provenance of an installer you downloaded yourself, run
the steps above.

## For the maintainer: what is still missing

- **Authenticode.** Add the certificate as the `WINDOWS_CODESIGN_PFX_BASE64`
  secret, and write the signing step in `scripts/authenticode-placeholder.sh`.
  Until then the release log says `UNSIGNED: no key`. The script fails the job
  if the secret exists before the step is written.
- **Android keystore.** Add `ANDROID_KEYSTORE_BASE64` and the Android build job
  together. The `sign` job fails on the secret until signing exists.
- **Updater key scope.** When `TAURI_SIGNING_PRIVATE_KEY` and its password are
  added, add them as secrets of the `release` environment, not as repository
  secrets, so only jobs that pass the environment gate can read them.

## Repository settings the release depends on

`updater-sign` (Tauri update key), `sign` (OIDC) and `release`
(`contents: write`) run only for `node-v*` tags and all use the `release`
environment; `cargo test -p xtask workflow_hardening` fails if a workflow edit
drops either. The rest of the gate is in GitHub settings, which no file in the
tree can pin. Recorded 2026-11-22 with the read-only commands below:

| Setting | Recorded value | Check |
|---|---|---|
| `release` environment reviewers | required reviewer: the owner | `gh api repos/EricWijesinghe/Maya2C/environments/release --jq '.protection_rules'` |
| `release` deployment refs | custom policy, one tag rule `node-v*` | `gh api repos/EricWijesinghe/Maya2C/environments/release/deployment-branch-policies --jq '.branch_policies'` |
| Tag ruleset "release tags: owner only, immutable" | active, all tags; creation, update, deletion and non-fast-forward restricted; bypass: admin role | `gh api repos/EricWijesinghe/Maya2C/rulesets/24884483 --jq '{enforcement,conditions,rules,bypass_actors}'` |
| Secrets | none at repository or `release` environment level (Tauri key not added yet) | `gh api repos/EricWijesinghe/Maya2C/actions/secrets --jq '.total_count'`; `gh api repos/EricWijesinghe/Maya2C/environments/release/secrets --jq '.total_count'` |

The environment allows administrators to bypass it (`can_admins_bypass:
true`) and does not prevent self-review; with one maintainer both are
expected. Re-run the commands before each release and update this table when
a value changes.
