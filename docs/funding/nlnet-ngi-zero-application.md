# NLnet NGI Zero — application draft

A draft for the owner to review, edit and submit at <https://nlnet.nl/propose/>.
Check the current call and deadline there first. Every figure below is
measured in this repository and cited so a reviewer can re-run it. Written
2026-09-28.

## Before submitting — two owner decisions

1. **Licence.** The repository is public but has no licence file, which
   legally means "all rights reserved". NLnet funds only open-source work
   under an OSI-approved licence. The common choice for Rust is
   **`Apache-2.0 OR MIT`**: permissive, patent grant, compatible with the
   crates this project uses. Adding it is the owner's call, not Claude's.
2. **Hourly rate and amount.** The budget below uses €50/hour and asks for
   €38,000, inside NLnet's usual €5,000–€50,000 for a first proposal.
   Adjust both to what the owner can honestly deliver.

---

## Proposal name

Maya Chat: post-quantum, peer-to-peer messaging on an open post-quantum
ledger

## Website / code

<https://github.com/EricWijesinghe/Maya2C>

## Abstract

Messages people send today can be recorded now and decrypted later, once
large quantum computers exist ("harvest now, decrypt later"). Messaging
systems have started adding post-quantum key exchange to one-to-one chats,
but group chats, identity, offline delivery and the metadata of who talks
to whom are still mostly classical, centralised, or both.

Maya Chat is a peer-to-peer messenger written in Rust:

- **Post-quantum by default.** Session and group keys are agreed with a
  hybrid of ML-KEM-768 (FIPS 203) and X25519. Identity keys are
  post-quantum signatures (ML-DSA-65, FIPS 204), so a break of either
  classical or lattice cryptography alone does not expose messages.
- **Groups with MLS.** Group chats use the Messaging Layer Security
  protocol (RFC 9420) with a hybrid post-quantum ciphersuite, rather than a
  bespoke group protocol.
- **Identity without a company.** A user's identity key is anchored on
  Maya2C, an open post-quantum ledger, so key rotation and revocation are
  public and verifiable, not a server's say-so.
- **Offline and delayed delivery.** Store-and-forward relays hold
  encrypted messages for offline recipients, designed on delay-tolerant
  networking lines, so the same protocol works over slow or intermittent
  links.
- **Detached by design.** Chat is its own application and binary, sharing
  only an identity-and-transport library with other apps (a wallet, later
  others). It holds no funds and needs no token to send a message.

**The foundation exists and is tested.** Maya2C is 87 Rust crates:
- post-quantum signatures (ML-DSA, SLH-DSA, and a hybrid of both);
- DAG-BFT consensus, and a libp2p network layer with a post-quantum
  handshake;
- 2,961 automated tests passing across the workspace;
- 71 language-neutral conformance vectors checked by an independent
  implementation.

This grant funds the chat layer on top of it, plus publication of the
post-quantum migration tooling already in the tree, which measures how
much of Bitcoin's and Ethereum's value sits behind exposed public keys.

**Expected outcomes:**
- a specified, documented chat protocol (ADR plus specification);
- a Rust library and a command-line client for one-to-one and group chat;
- store-and-forward relays;
- a minimal desktop client;
- published migration tooling;
- all of it under an open licence, with reproducible builds and tests.

## Relevant previous involvement

The applicant has built Maya2C single-handed in Rust, with AI assistance
used and disclosed. It includes post-quantum signature suites checked
against NIST ACVP vectors, a Bitcoin SPV client, and an Ethereum
sync-committee light client verified on real mainnet data. Every
subsystem's status is in `features.toml`, backed by named tests, and
`cargo xtask coverage` fails the build if a claim loses its test.

## Requested amount

€38,000

## What the budget is for

At €50/hour:

| Milestone | Deliverable | Hours | € |
|---|---|---|---|
| 1 | Chat ADR and protocol specification: identities, hybrid KEM, MLS profile, relay protocol, threat model | 80 | 4,000 |
| 2 | `maya-chat` core: identity keys anchored on chain, hybrid ML-KEM-768 + X25519 key agreement, one-to-one sessions; property tests and test vectors | 140 | 7,000 |
| 3 | Group chat on MLS (RFC 9420) with a hybrid post-quantum ciphersuite; membership changes; interop tests | 160 | 8,000 |
| 4 | Store-and-forward relays and offline delivery over libp2p; delay-tolerant retry; abuse limits | 120 | 6,000 |
| 5 | Command-line client and a minimal desktop client (Tauri, Rust backend) | 120 | 6,000 |
| 6 | Reproducible builds, SBOM, documentation, security-review preparation | 80 | 4,000 |
| 7 | Publish the post-quantum exposure tool (BTC/ETH) as a standalone crate with docs | 60 | 3,000 |
| **Total** | | **760** | **38,000** |

## Other funding

None. The project has no investors, no token and no revenue.

## Comparison with existing efforts

- **Signal** added post-quantum key agreement (PQXDH, then ML-KEM in its
  ratchet). It is centralised, and identity is a phone number held by
  Signal's servers.
- **Matrix** is federated, and post-quantum work is in progress there;
  identity lives on a homeserver.
- **Briar** is peer-to-peer (Tor, Bluetooth, Wi-Fi), with no post-quantum
  key agreement at the time of writing.
- **SimpleX** needs no user identifiers and uses queue-based relays; its
  post-quantum ratchet has been on by default in direct chats since v5.6
  (2024), though groups beyond roughly 10–20 members are not post-quantum.
- **iMessage** has shipped post-quantum key agreement (PQ3) since 2024, on
  Apple servers and Apple devices only.

Post-quantum *encryption* is therefore not what sets Maya Chat apart:
Signal, iMessage and SimpleX all ship it today. What this project adds is
a combination of properties. It does not claim to be the first at any one
of them, and each is still to be checked against a dated search:
- **post-quantum authentication as well as secrecy.** Identities and
  handshakes are signed with ML-DSA-65. The systems above protect key
  agreement against quantum attack, but their identity keys and signatures
  are classical;
- **the chat identity is an account on an open post-quantum ledger**, so
  key rotation and revocation can be anchored publicly instead of trusted
  to a provider's key directory;
- store-and-forward designed for delay-tolerant links;
- all in Rust, on a codebase with published conformance vectors.

The dated prior-art search behind this section is
`docs/prior-art/p2p-chat.md` (2026-09-28). It is recorded as partial:
Status/Waku were not resolved. No public comparison claim is made until
it is complete, and the repository enforces that in CI
(`cargo xtask claims-check`).

## Technical challenges

- **An MLS ciphersuite with a hybrid post-quantum KEM** that interoperates,
  following the IETF drafts for post-quantum MLS rather than inventing one.
- **Key sizes.** ML-KEM ciphertexts and ML-DSA signatures are kilobytes, so
  message framing, relay storage and mobile bandwidth must be designed for
  them. The ledger's own signed transfer is already about 13 KB, and its
  wallet splits signed transactions across animated QR frames for exactly
  this reason.
- **Metadata.** Encryption hides content, not who talks to whom. Milestone
  1's threat model decides what relays may learn. Mixnet routing is named
  as future work, not promised in this grant.
- **Offline delivery without a central server.** Relay incentives and abuse
  limits without requiring users to hold tokens.

## Ecosystem and engagement

- Code, specifications and test vectors published in the repository under
  the chosen open licence.
- The chat library shared with other detached applications (the wallet
  already exists) through one identity-and-transport crate.
- Post-quantum migration findings shared with the Bitcoin and Ethereum
  communities through the published exposure tool.
- Security review through NLnet's partners (NGI Zero offers audit
  support), before anything carries real value.
