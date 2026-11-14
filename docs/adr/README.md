# Architecture Decision Records

One file per decision that would otherwise be re-litigated by whoever arrives
next. An ADR records what was decided, what it cost, and what would have to be
true to revisit it — not what the code does. For what the code does, read
`CLAUDE.md` and `docs/architecture-vision.md`.

A record is written **when the decision is made**, not afterwards. If a
decision turns out to be wrong it gets a new ADR that supersedes the old one;
the old one stays, with its status changed, because the reasoning that was
wrong is the useful part.

## Format

```
# ADR-NNN: <title>

**Status:** Proposed | Accepted | Superseded by ADR-NNN
**Date:** YYYY-MM-DD

## Context
What made a decision necessary. Measurements, not impressions.

## Decision
What was chosen, stated so that a reader can tell whether the code obeys it.

## Alternatives considered
Each with the reason it was not chosen.

## Consequences
What this costs, including the parts that are worse than the alternative.

## Revisit when
The observation that would make this worth reopening.
```

## Index

| ADR | Title | Status |
|---|---|---|
| [001](ADR-001-workspace-layout.md) | Workspace layout, and why the directory move is deferred | Accepted |
| [002](ADR-002-feature-tiers.md) | Feature tiers gate services, never consensus | Accepted |
| [003](ADR-003-build-profiles.md) | Build profiles and the disk ceiling | Accepted |
| [004](ADR-004-reality-ledger.md) | `features.toml` as the reality ledger | Accepted |
| [005](ADR-005-dependency-unification.md) | One version per dependency, and the duplication that remains | Accepted |
| [006](ADR-006-simulation-harness.md) | A hand-rolled deterministic simulator, not `madsim` or `turmoil` | Accepted |
| [007](ADR-007-signature-suites.md) | Signature suites, a registry, and a suite-tagged envelope | Accepted |
| [008](ADR-008-transparent-zk.md) | Plonky3 STARKs as the only proof system | Accepted |
| [009](ADR-009-kem-suites.md) | KEM suites and the two combiners | Accepted |
| [010](ADR-010-entropy.md) | Entropy — every source health-tested, all mixed into one HMAC-DRBG | Accepted |
| [011](ADR-011-threshold-custody.md) | Threshold custody — on-chain multisig now, threshold lattice signing later | Accepted |
| [012](ADR-012-hash-lock-htlcs.md) | Hash-lock HTLCs are live; lattice locks stay RESEARCH | Accepted |
| [013](ADR-013-suite-envelope-activation.md) | The suite envelope and multisig are live from genesis | Accepted |
| [014](ADR-014-threshold-raccoon.md) | Threshold Raccoon for threshold lattice custody (RESEARCH) | Accepted |
| [015](ADR-015-consensus-design.md) | One consensus design — DAG-BFT orders, work never does | Accepted |
| [016](ADR-016-launch-scope.md) | Launch scope freeze — what mainnet v1 is, and what waits | Accepted |
| [017](ADR-017-parallel-execution.md) | Parallel execution — optimistic with in-order validation first | Accepted |
| [018](ADR-018-async-execution.md) | Synchronous execution for v1; asynchronous (D = 2) next | Accepted |
| [019](ADR-019-storage-engine.md) | Stay on RocksDB until it is the measured bottleneck | Accepted |
| [020](ADR-020-spec-conformance-and-upgrades.md) | A spec with an independent reference; upgrades halt instead of fork | Accepted |
| [021](ADR-021-vote-certificates.md) | Vote certificates — a signature list for v1, a 100-validator cap from its measured cost | Accepted |
| [022](ADR-022-remote-signer-backends.md) | Remote signer — keystore backend now, HSM/KMS when one can be tested | Accepted |
| [023](ADR-023-evm-compatibility-honesty.md) | EVM compatibility is not in v1; when it comes it is labelled classical | Accepted |
| [024](ADR-024-oracle-choice.md) | Oracles — the native module, optional, named authorities until staking exists | Accepted |
| [025](ADR-025-mev-position.md) | The mainnet MEV position — batch settlement in core, sealed mempool optional | Accepted |
| [026](ADR-026-contract-caller-identity.md) | Contracts need to know who called them — a `caller` host function at an activation height | Accepted |
| [027](ADR-027-dag-bft-in-the-node.md) | DAG-BFT in the node — derived blocks, signed votes, a safety log | Accepted |
| [028](ADR-028-staking.md) | Staking — bonded committees, slot-counted liveness, evidence-based slashing | Accepted |
| [029](ADR-029-fee-market-live.md) | The fee market goes live — fees are signed outputs | Accepted |
| [030](ADR-030-vault-accounts.md) | Vault accounts — delayed withdrawals, guardian cancel | Accepted |
| [031](ADR-031-maya-chat.md) | Maya Chat — a detached, post-quantum, peer-to-peer messenger | Accepted |
| [032](ADR-032-testnet-hosting.md) | First public testnet — one seed on Oracle Always Free, one installer | Accepted |
| [033](ADR-033-remote-signer-in-the-node.md) | The remote signer in the node — protection keyed to the DAG, signatures unchanged | Accepted |
| [034](ADR-034-vm-reference-types-off.md) | The consensus VM rejects reference types and typed function references | Accepted |
| [035](ADR-035-bft-fixed-target.md) | DAG-BFT blocks keep the genesis difficulty target | Accepted |
| [036](ADR-036-chain-bound-signatures.md) | Transaction signatures commit to the chain's genesis | Accepted |
| [037](ADR-037-mainnet-shielded-off.md) | Mainnet v1 launches with the shielded pool off | Accepted |
| [038](ADR-038-attested-checkpoints.md) | Attested checkpoints, so a validator can rejoin | Accepted |
| [039](ADR-039-quorum-n-minus-f.md) | DAG-BFT quorum is n − f, not 2f + 1; committee capture open | Accepted |
| [040](ADR-040-stake-weighted-committee.md) | Stake-weighted DAG-BFT committees — engine built, node wiring for decision | Accepted |
| [041](ADR-041-follower-ordered-set.md) | Followers become builders after one checkpoint (gate 10) | Proposed |
| [042](ADR-042-solo-launch-path.md) | The launch path for a project with one operator | Accepted |
| [043](ADR-043-rejoin-after-any-outage.md) | A way back after any outage, not only a short one | Accepted |
| [044](ADR-044-mining-rewards-lane.md) | Block rewards and GPU mining at launch, without letting miners order blocks | Proposed |
| [045](ADR-045-maya2c-hub.md) | Maya2C Hub: one desktop window for mining and every app | Accepted |
| [046](ADR-046-universe-scale-supply.md) | Universe-scale supply headroom: u256 balances, 18-decimal precision, cap decoupled from emission | Accepted |
| [047](ADR-047-single-pc-foundation-set.md) | The foundation validator set runs on one PC, said so publicly (supersedes ADR-042 gate 5) | Accepted |
| [048](ADR-048-maya-amount.md) | `maya-amount`: one dependency-free 256-bit integer for the proved crates | Accepted |
| [049](ADR-049-sealed-validator-keys.md) | Validator keys sealed at rest; a production build refuses plaintext | Accepted |
| [050](ADR-050-owner-control-plane.md) | Owner control plane: full operator control, a bounded 2-of-3 council pause, no backdoor | Accepted |
| [051](ADR-051-security-level-5-defaults.md) | NIST level 5: ML-KEM-1024 transport and a Maximum wallet option now, ML-DSA-87 validator keys at mainnet genesis | Accepted (options 1 and 2); 3 at mainnet genesis; 4 not now |
| [052](ADR-052-follower-advance-by-certificates.md) | A follower advances on certificates the current committee signed (rejoin by certificate replay) | Accepted |
| [053](ADR-053-ordered-set-root-in-header.md) | Commit each anchor's ordered sub-DAG in the block header | Proposed |
| [054](ADR-054-reproducible-builds.md) | Reproducible Linux node builds: path remapping, one shared build script, a twice-built CI check | Accepted |
| [055](ADR-055-quantum-harbor-binding.md) | Quantum Harbor step one: classical-key → PQ-vault binding, anchored on chain; no coins move | Accepted |
| [056](ADR-056-guardian-recovery-transactions.md) | Guardian recovery as four node transactions with a delay and an owner cancel | Accepted |
| [057](ADR-057-hybrid-oracle-pc-testnet.md) | maya-testnet-1 runs one Oracle validator and four PC validators | Accepted |
| [058](ADR-058-batched-catch-up.md) | Catch-up fetches blocks in batches | Accepted |
| [059](ADR-059-finalized-tps-benchmark.md) | What "TPS" means, and `cargo xtask bench-tps` | Accepted |
| [060](ADR-060-trusted-block-bootstrap.md) | A trusted block anchors snapshot bootstrap | Accepted |
| [061](ADR-061-dvp-needs-seller-consent.md) | RWA delivery-versus-payment is off until the seller consents | Accepted |
| [062](ADR-062-state-layout-before-genesis.md) | Choose the accounts tree and the staking record layout before genesis | Accepted |
| [063](ADR-063-block-header-version-and-time-bound.md) | A version byte and a timestamp bound in the block header | Accepted |
| [064](ADR-064-dex-dust-orders.md) | A zero-quote trade cancels the dust order instead of stopping the book | Accepted |
| [065](ADR-065-cross-chain-asset-routes.md) | Which cross-chain asset routes reach consensus, and in what order | Proposed |
| [066](ADR-066-every-amount-256-bit.md) | Every amount is 256-bit behind the interface | Proposed |
| [067](ADR-067-shielded-and-private-amounts-256-bit.md) | 256-bit amounts inside the shielded pool and private state | Proposed |
| [068](ADR-068-forever-mining.md) | Forever mining: a tail reward the genesis reserve pays to the last height | Accepted |
| [069](ADR-069-post-mainnet-feature-program.md) | Post-mainnet feature program (eleven features) | Proposed |
| [070](ADR-070-committee-floor.md) | Downtime jailing keeps at least four eligible validators | Accepted |
| [071](ADR-071-site-integrity.md) | The site build cannot deploy, and the live site is checked against a signed manifest | Accepted |
| [072](ADR-072-max-block-bytes.md) | A block has a byte cap; transactions past it wait for the next block | Proposed |
| [073](ADR-073-hub-multi-rig-pool-profitability.md) | Hub multi-rig, pool view and on-chain profitability estimate | Proposed |
| [074](ADR-074-chat-outbox-receipts-envelope-v2.md) | Chat sender outbox, signed delivery receipts, envelope v2 (and a send-path nonce-reuse fix) | Accepted in part |
| [075](ADR-075-node-update-notice.md) | Node "update available" notice, notify only | Implemented, approval not recorded |
| [076](ADR-076-base-fee-step-clamp-and-ceiling.md) | The base fee rises at most one denominator's share per block, and has a ceiling | Accepted |
| [077](ADR-077-sentinel-recovery-network.md) | Sentinel Recovery Network: scoped quarantine and forward-only governed recovery (MIP-0001), after mainnet | Accepted |
| [078](ADR-078-omega-durable-advantage.md) | Omega design: three-speed evolution (vote-activated, pre-shipped code), bounded integer controllers, test ratchet, moat indicators (MIP-0002) | Accepted |
| [079](ADR-079-verifiable-compute.md) | Verifiable compute: a job market for useful work, a verification ladder, multi-GPU hardware plan, data sovereignty, phases R0 to R6 after mainnet | Proposed |
| [080](ADR-080-release-provenance-attestations.md) | Release provenance attestations for every file; threshold (k-of-n) release signing as a follow-up | Accepted |
| [081](ADR-081-address-text-bech32m.md) | Addresses: Bech32m text form with a network prefix and checksum; account id unchanged | Accepted |
| [082](ADR-082-genesis-committed-upgrade-levers.md) | Genesis-committed upgrade levers: dark features via the schedule, suite sunset record, schedule and PoW floor in the genesis id | Accepted |
| [083](ADR-083-hub-wallet-and-chat-pages.md) | Hub Wallet and Chat pages: watch-only balances, keys stay in their apps; app launch via OS link scheme later | Accepted |
| [084](ADR-084-mcp-read-and-draft-only.md) | The MCP server reads the chain and drafts transfers; it never signs or submits | Accepted |
| [085](ADR-085-chat-v2-groups-on-mls.md) | Group chats on MLS with a hybrid post-quantum ciphersuite (chat-v2) | Proposed |
| [086](ADR-086-wallet-v2-seedless-recovery-and-clear-signing.md) | wallet-v2: seedless smart-account recovery, clear signing, address book, payment requests | Proposed |
| [087](ADR-087-shielded-pool-activation-after-genesis.md) | Enable the shielded pool after genesis through a scheduled upgrade | Proposed |
| [089](ADR-089-native-finality-light-client.md) | Native finality light client (succinct proofs), phase 0 | Implemented for phase 0, approval not recorded |
| [090](ADR-090-testnet-kit-and-node-box.md) | Testnet kit and node-in-a-box live in xtask; `down` stops by PID and start time | Accepted |
| [101](ADR-101-sentinel-v2.md) | Sentinel v2, phase 1: a dead-man heartbeat and consensus detectors | Implemented for phase 1, approval not recorded; phases 2 to 5 Proposed |
| [102](ADR-102-stealth-addresses-ml-kem.md) | Stealth addresses over ML-KEM-768: hash-committed one-time addresses, view tag, v10 spend frame; spend reveals the spend address | Proposed |
| [103](ADR-103-native-fungible-tokens.md) | Native fungible tokens: optional mint authority and an immutable 256-bit cap on the existing asset record; fee in native coin | Proposed |
| [104](ADR-104-unique-assets.md) | Unique assets (NFTs): metadata hash only under `n:`, royalties settled on on-chain sales | Proposed |
| [105](ADR-105-name-service.md) | Name service under `nm:`: ASCII labels, commit-reveal, renewal and premium by height, owner-only reverse records | Proposed |
| [106](ADR-106-sponsored-fees.md) | Sponsored fees: an outer v11 frame where a sponsor co-signs txid, ceiling and expiry under the chain tag | Proposed |
| [107](ADR-107-smart-account-promotion.md) | Smart-account promotion: session keys and spending limits on the v9 frame through `verify_with_state` | Proposed |
| [108](ADR-108-liquid-staking-receipt.md) | Liquid staking: protocol pool, exchange-rate receipt asset, slashing socialized through `Staking::slash` | Proposed |
| [109](ADR-109-scheduled-payments.md) | Scheduled payments under `sp:`: prepaid escrow, executed at a height in the apply path, bounded per block | Proposed |
| [137](ADR-137-hardware-intelligence.md) | Hardware intelligence: Hub GPU tuner with rollback, self-tuning node that cannot change a result, GPU signature checks that stay advisory, inference jobs scheduled around mining | Proposed |
