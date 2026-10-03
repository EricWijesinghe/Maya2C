# NLnet NGI Zero — application draft

A draft for the owner to review, edit and submit at <https://nlnet.nl/propose/>.
Check the current call and deadline there first. Every figure below is
measured in this repository and cited so a reviewer can re-run it. Written
2026-09-28; refreshed 2026-10-04 now that one-to-one chat ships.

## Before submitting — owner decisions

1. ~~**Licence.**~~ Done: Apache-2.0 (`LICENSE`, commit `713c9711`).
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

**What already exists and is tested:**
- **Maya2C**, 87 Rust crates:
  - post-quantum signatures (ML-DSA, SLH-DSA, and a hybrid of both);
  - DAG-BFT consensus;
  - a libp2p network layer with a post-quantum handshake;
  - 71 language-neutral conformance vectors checked by an independent
    implementation.
- **A public test network**, maya-testnet-1, running since 2026-09-29.
- **Maya Chat, one-to-one** (ADR-031):
  - ML-DSA-65 identities that are chain addresses;
  - signed X-Wing (ML-KEM-768 + X25519) prekeys;
  - libp2p store-and-forward relays, one of them public at
    `chat.maya2c.dev`;
  - a command-line client and a Tauri desktop app.
  - It is unaudited, with no groups and no metadata privacy yet.

This grant funds what is missing before people can rely on it:
- group chat;
- metadata protection;
- an external security review;
- publication of the post-quantum migration tooling already in the tree,
  which measures how much of Bitcoin's and Ethereum's value sits behind
  exposed public keys.

**Expected outcomes:**
- group chat on MLS (RFC 9420) with a hybrid post-quantum ciphersuite;
- relays that learn less about who talks to whom, to a written threat model;
- the shipped protocol specified with test vectors, so a second
  implementation can interoperate;
- a security review through NGI Zero's audit partners, with findings fixed
  and published;
- published migration tooling;
- all of it under Apache-2.0, with reproducible builds and tests.

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
| 1 | Specification of the shipped protocol (identities, X-Wing prekeys, sessions, relay protocol) with test vectors, and a threat model that covers metadata | 80 | 4,000 |
| 2 | Group chat on MLS (RFC 9420) with a hybrid post-quantum ciphersuite, following the IETF post-quantum MLS drafts; membership changes; interop tests | 200 | 10,000 |
| 3 | Metadata protection at relays: sealed-sender style deposits, unlinkable mailbox addressing, padding; measured against the threat model | 160 | 8,000 |
| 4 | Groups in the CLI and desktop clients; key backup and multi-device | 120 | 6,000 |
| 5 | Security-review preparation and remediation: fuzzing every decoder, reproducible builds, SBOM; fixing and publishing the review's findings | 140 | 7,000 |
| 6 | Publish the post-quantum exposure tool (BTC/ETH) as a standalone crate with docs | 60 | 3,000 |
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
