---
title: Code signing policy
description: How Maya2C release binaries are built, signed and verified, who approves a signature, and what the apps send over the network.
---

Every Maya2C binary you download is built by GitHub Actions from the public
source at [github.com/EricWijesinghe/Maya2C](https://github.com/EricWijesinghe/Maya2C),
by the workflow `.github/workflows/release-binaries.yml`. Nobody builds a
release on a personal computer.

## Status

| | Today |
|---|---|
| Build provenance (Sigstore) | **On** for every release. Proves a file came from this repository's workflow at a named commit. |
| Checksums and SBOM | **On**: `SHA256SUMS` and a software bill of materials are attached to every release. |
| Windows and macOS code signing | **Not yet.** An application for free open-source code signing has been made to the SignPath Foundation. Until it is granted, Windows SmartScreen and macOS Gatekeeper show "unknown publisher". |
| App update signatures | Wallet and Chat update bundles are signed with the project's update key once it is configured for a release. |

When code signing is granted, this page will carry the signing provider's
attribution, and nothing else about the process changes: the same workflow,
the same source, plus a signature.

## Verify a download yourself

```bash
gh attestation verify <file> --repo EricWijesinghe/Maya2C
sha256sum -c SHA256SUMS --ignore-missing
```

The first proves the file was built by this repository's release workflow.
The second proves it is the file the release published.

## Team and roles

| Role | Who | Responsibility |
|---|---|---|
| Committer and reviewer | Eric Wijesinghe | Owns the repository. Every change from anyone else is reviewed before merge; `master` accepts pull requests only, behind required CI. |
| Approver | Eric Wijesinghe | Approves each signing request by hand. No release is signed automatically. |

Multi-factor authentication is required on the source repository for every
team member.

## What is signed

Only binaries built by the release workflow from a tagged commit
(`node-v*`): the node and command-line tools, Maya Wallet, Maya Chat and
Maya2C Hub (with the GPU miner inside). The product name and version in each
binary match the release.

## Privacy

The apps send no analytics and no usage data. They connect only to what their
job needs:

- **Maya2C Hub:** the network's JSON-RPC endpoint (the public testnet RPC by
  default, changeable in Settings) to show network status, and to submit the
  miner's shares.
- **Maya Wallet:** the JSON-RPC endpoint set in its settings, to read balances
  and send transactions you sign.
- **Maya Chat:** the relays it needs to deliver your messages, encrypted
  between the two ends with a post-quantum key exchange. Chat is research
  software that has not had an external review yet.
- **Node:** other nodes of the network it joins.

Keys never leave your device.
