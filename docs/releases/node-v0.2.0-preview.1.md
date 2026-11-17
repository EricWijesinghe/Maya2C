# Maya2C node-v0.2.0-preview.1: pre-genesis preview

**Pre-audit: no external audit yet; validators run by one operator at launch;
external audit planned after funding.** This is a preview of the mainnet
candidate for local devnets and review. Mainnet has not launched, no public
testnet is running, and nothing on any chain built from this release carries
value.

## What it is

The node (`maya2c-node`), the developer CLI, and the Maya2C Wallet, Maya2C Chat
and Maya2C Hub desktop installers, built from `master` by
`.github/workflows/release-binaries.yml` with `--features production`.

It replaces every earlier release. Earlier releases and their tags have been
removed; they are not supported and must not be run.

Full change list: every pull request merged since node-v0.1.0-testnet.4
(2026-10-19). The design record for each consensus change is in `docs/adr/`,
and the reasons are in `DECISIONS.md`.

## Known open work before genesis (re-freeze in progress)

These changes are being built and reviewed now and are **not** in this preview.
A later preview replaces this one when they land:

- governance windows at 1 block/s and a due-height settlement index (#500)
- unknown upgrade bits admitted, halt at activation (#506)
- per-block work bounds for sealed reveals, DEX books and staking (#522)
- incremental state root, VM gas caps and storage pricing, per-delegation
  staking records, a work-kind byte, the transaction version bound into signed
  bytes, a reserved hash-id byte, and block-validity parameters on the
  scheduled-upgrade path (re-freeze bundle 2)

Until they land, a node built from this preview is open to the block-time
denial-of-service cases those PRs describe. Do not expose it to untrusted peers.

## Verify before running

Every file is signed keylessly (cosign, Sigstore) and carries GitHub build
provenance. The desktop installers are not code-signed, so Windows and macOS
warn about an unknown publisher. Check every download against `SHA256SUMS` and
follow `docs/verify-release.md`.

## Report a vulnerability

security@maya2c.dev. See `SECURITY.md`.
