# Prior art: Post-quantum signatures from genesis

**Search status:** searched 2026-09-26.

**Who else does something similar.** QRL has used XMSS (hash-based, stateful) since its June 2018 genesis and is adding ML-DSA ([QRL docs](https://docs.theqrl.org/what-is-qrl/)).

**How Maya2C differs.** Maya2C uses a stateless hybrid, ML-DSA-65 and SLH-DSA-SHA2-128s, where both must verify (TX-1). It is not the first post-quantum chain.

**What may be said publicly.** "first post-quantum L1" is **forbidden**. "hybrid lattice + hash-based signatures on every transaction from genesis" is accurate and needs no superlative.

## Re-search, 2026-10-28

Sources are mixed: project pages, one SoK paper, and several vendor or
exchange blogs that disagree with each other. Marked where unconfirmed.

| Chain | What it does | Source |
|---|---|---|
| QRL | XMSS since 2018; QRL 2.0 (Zond) moving to a newer scheme. One source says ML-DSA-87 on testnet, another says SPHINCS+: unresolved | [Nervos KB](https://www.nervos.org/knowledge-base/top_quantum_resistant_cryptocurrency_projects_in_2026), [QRL docs](https://docs.theqrl.org/what-is-qrl/) |
| Algorand | Falcon-signed state proofs; a Falcon logic-signature transaction on mainnet on 2025-11-03. Native Falcon accounts: sources conflict on whether they are live | [SoK, arXiv 2512.13333](https://arxiv.org/pdf/2512.13333), [matrix](https://katemgrey.substack.com/p/seven-chains-one-quantum-question) |
| Abelian | Lattice-based privacy; mainnet listed as live, unverified | [Nervos KB](https://www.nervos.org/knowledge-base/top_quantum_resistant_cryptocurrency_projects_in_2026) |
| Quantus | Claimed PQ-only ledger, ML-DSA-65; single unverified source | [matrix](https://katemgrey.substack.com/p/seven-chains-one-quantum-question) |
| Bitcoin | BIP-360, now P2MR, merged into the BIP repository Feb 2026: removes the quantum-exposed key-spend path; not activated, no PQ signature BIP yet | [delvingbitcoin](https://delvingbitcoin.org/t/changes-to-bip-360-pay-to-quantum-resistant-hash-p2qrh/1811) |
| Ethereum | Multi-year roadmap to 2029; BLS, KZG and ECDSA all exposed today | [ethereum.org](https://ethereum.org/en/roadmap/future-proofing/quantum-resistance/) |

**How Maya2C differs, still accurate.** Every transaction carries two
signatures from different families (lattice ML-DSA and hash-based SLH-DSA)
that must both verify, from genesis; validator votes are PQ-signed; the
transport is ML-KEM; and a suite registry adds or retires a scheme at an
activation height without a hard fork. None of BLS, KZG or ECDSA is in
consensus.

**Still forbidden:** "first" or "only" post-quantum chain. QRL predates it,
and others now ship PQ pieces.
