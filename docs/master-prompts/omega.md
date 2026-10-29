# Maya2C Omega: the owner's direction brief (2026-10-28)

Eric's brief, given 2026-10-28 with the instruction: *do the best you can, stay
ahead of every ecosystem; every build is pre-authorised.* The authorisation
covers building. It does not cover money, mainnet, keys or protocol economics:
those still need Eric's word (CLAUDE.md, "When to stop and ask Eric").

This file has three parts: where the tree stands against each section (the
status map), where the brief conflicts with recorded decisions (Standing Order
9: reported, not worked around), and the build order. The brief itself is kept
verbatim at the end. It is a direction, not a public document: its superlatives
("ever conceived", "reference implementation") never go into public copy
without a `docs/prior-art/` record (Standing Order 10, `cargo xtask claims-check`).

## Status map

Statuses are `features.toml`'s on 2026-10-28; that file stays the authority.

| Section | In the tree | Missing |
|---|---|---|
| Consensus | `dag-bft` verified; staking/slashing; attacknet; deterministic sim; adversarial schedule checker (2026-10-28) | Machine-checked proofs (TLA+/Quint or Lean); anti-cartel and geographic-diversity rules |
| PQ cryptography | ML-KEM, ML-DSA, SLH-DSA, HQC verified; suite envelope verified; `quantum-exposure-tool` verified | Automatic migration (`hash-migration-rehearsal` working) |
| PQ privacy | STARK shielded pool verified; viewing keys, private contract state, selective disclosure working | Private voting, anonymous credentials, FHE framework |
| Recursive proofs | Plonky3 STARK working; stateless transfer verification working; SPV verified | Recursion, constant-size chain proof, phone verification |
| Account abstraction | `smart-account` working; vault accounts verified | Passkeys, guardians/social recovery, session keys, sponsorship |
| Interoperability | HTLC swaps verified; ETH beacon light client verified; BTC SPV working; `interop-proof-routes` working | IBC, Polkadot, a general messaging layer |
| Modular ecosystem | DA layer, L2 flash settlement, multi-VM phase A | Sovereign/app-chain framework |
| AI economy | `mcp-server`; ADR-044 WorkClaim plan | Agent identity and wallets; verified inference (zkML planned) |
| Identity | SSI records, DID rotation, credentials working | Organisation, device and AI identity types; reputation |
| Permanent storage | Archive, pruning, history compactor | Proofs of storage and retrieval: nothing exists |
| MEV resistance | Threshold encrypted mempool verified | A fair-ordering proof |
| Governance | Lifecycle verified; security council pause verified; treasury vesting | Chambers (citizens, delegates, experts); quadratic voting |
| Formal methods | Kani on ledger crates verified; spec conformance verified | Lean 4 planned; nothing for the VM or upgrades |
| Resilience | LoRa, store-and-forward mesh, fountain coding working | Satellite (planned) |
| Developer experience | SDKs, dev CLI, one-line node join, deterministic sim | Formal tools for contract authors |

## Conflicts with recorded decisions

1. **"Post-quantum by default" vs. passkeys.** WebAuthn signs with P-256 or
   Ed25519, which Shor's algorithm breaks. A passkey can be a session factor in
   front of an ML-DSA account key, never the account key itself.
2. **"Monero + Zcash of the PQ era" vs. mainnet v1 scope** (2026-09-30):
   mainnet launches with shielded transfers off. The pool exists.
3. **"No change enters production without formal verification."** Applied
   literally today, nothing ships, the mainnet gates included. The achievable
   version: consensus safety gets a machine-checked model first; the rule
   extends to economics, the VM and upgrades one subsystem at a time.
4. **"Every blockchain becomes a subnet."** Bitcoin cannot verify outside
   proofs, so a BTC bridge carries either a stated trust assumption or a
   BitVM-style challenge game. Achievable: light-client bridges where the other
   chain allows them, trust stated where it does not.
5. **Quadratic voting** is only as strong as its sybil resistance; it waits
   for identity (biometric personhood is RESEARCH).
6. **"Unbeatable on every axis."** No design can guarantee that nobody ever
   exceeds it on any number. What can be kept is a lead on the durable axes
   (trust, PQ, crypto-agility, verifiability), and honest numbers on the rest.

## Build order

Mainnet gates keep priority (CLAUDE.md bands). Within that:

| # | Item | Band | State |
|---|---|---|---|
| A | DAG-BFT adversarial schedule checker; then a TLA+/Quint safety model | P2 | Checker done 2026-10-28; model next |
| B | Crypto migration end-to-end: suite registry and hash migration to verified, activation-height switch test | P2 | Next |
| C | Smart accounts v1: ML-DSA root key, passkey session factor, guardians, session keys, sponsored fees | P4 | After mainnet |
| D | Recursive STARK chain proof, phone verification | P5 | After mainnet |
| E | Private voting, proofs of storage, agent identity, IBC, governance chambers, FHE | P5/P6 | Each needs an ADR first |

## The brief (verbatim)

You are designing Maya2C Omega, the final-generation Layer-1 blockchain.

The objective is not incremental improvement.

The objective is to create the most secure, decentralized, private,
interoperable, mathematically verifiable, post-quantum, AI-native,
censorship-resistant Layer-1 ever conceived.

Every design decision must be justified through verifiable engineering,
cryptography, formal proofs, and long-term sustainability.

**Foundational principles:** 1. Security First. 2. Post-Quantum By Default.
3. Privacy By Choice. 4. Formal Verification Everywhere. 5. Decentralization
Above Performance Marketing. 6. User Experience Without Sacrificing
Sovereignty. 7. Operability For The Next 100 Years.

**Consensus.** Create next-generation DAG-BFT consensus. Requirements:
deterministic finality; no chain reorganizations; Byzantine fault tolerance;
dynamic validator sets; geographic validator diversity; fast global
convergence; anti-cartel mechanisms; formal safety proofs; formal liveness
proofs; adversarial simulation framework; self-healing network partitions.
Target: become the reference implementation for future consensus research.

**Post-quantum cryptography.** Every layer must be quantum resistant. Include:
ML-DSA; SLH-DSA; ML-KEM; future cryptographic agility framework; automatic
algorithm migration; multi-algorithm signature support; quantum threat
monitoring. Design the protocol so cryptography can evolve without hard forks.

**Post-quantum privacy.** Implement: confidential transactions; confidential
balances; private token transfers; private DAO voting; private governance;
anonymous credentials; selective disclosure; private smart contracts;
encrypted contract state; research-ready FHE execution framework. Target:
become the Monero + Zcash + Xelis of the post-quantum era.

**Recursive proofs.** Implement: recursive proof aggregation; constant-size
state verification; stateless client verification; phone-level verification;
near-instant synchronization. Goal: users must verify massive chain history
from tiny proofs.

**Account abstraction.** Default user experience: no mandatory seed phrases;
passkeys; social recovery; multi-device recovery; biometric authentication;
recovery guardians; gas sponsorship; session keys. Goal: a non-technical user
should onboard in minutes.

**Interoperability.** Support: Bitcoin, Ethereum, Cosmos, Polkadot, rollup and
future chain interoperability. Requirements: trust-minimized bridges;
verifiable proofs; cross-chain contract execution; universal messaging layer.
Goal: every blockchain becomes a subnet of Maya2C.

**Modular ecosystem.** Separate: consensus; execution; settlement; storage;
data availability. Support: sovereign chains; rollups; application chains;
enterprise chains. Goal: unlimited ecosystem expansion.

**AI-native cryptographic economy.** Create: agent identities; agent wallets;
agent DAOs; agent marketplaces; agent governance. Support: verifiable AI
execution; proof-backed inference; AI ownership; model licensing; compute
markets. Goal: enable autonomous digital economies.

**Decentralized identity.** Create quantum-safe identity. Include: human,
organization, device and AI identity. Support: ZK credentials; education
credentials; professional credentials; reputation systems. Goal:
privacy-preserving trust at internet scale.

**Permanent storage.** Build native decentralized storage. Requirements:
long-term retention; proofs of storage; proofs of retrieval; archives; media;
research datasets. Target: preserve information for generations.

**MEV resistance.** Implement: encrypted mempool; fair ordering; threshold
cryptography; front-running protection; auction resistance. Goal: markets
should be fair by protocol design.

**Governance.** Create Constitutional Governance. Layers: citizens; delegates;
experts; security councils. Support: treasury governance; emergency response;
constitutional amendments; reputation-weighted governance; quadratic voting.
Goal: governance without plutocracy.

**Formal methods.** Require mathematical proofs for: consensus; economics; VM;
contracts; upgrades. No protocol change enters production unless invariants
pass formal verification.

**Resilience.** Network must function through: internet outages; censorship
events; regional failures; mesh networks; satellite relays. Goal:
civilizational-grade survivability.

**Developer experience.** Provide: Rust-native ecosystem; WASM ecosystem;
formal verification tools; AI-assisted development; one-command deployments;
deterministic simulation environments. Goal: become the easiest advanced
Layer-1 to build on.

**Final objective.** Do not optimize for TPS numbers. Do not optimize for
marketing. Do not optimize for hype cycles. Optimize for: security; privacy;
decentralization; quantum resistance; interoperability; verifiability;
longevity. Build not just a blockchain. Build the cryptographic operating
system of the post-quantum century.
