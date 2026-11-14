---
title: How Maya2C compares on post-quantum security
description: "A dated comparison (searched 2026-11-01) of Maya2C with 18 post-quantum chain entries on signatures, mandatory use, dual signing, peer transport and status. Every cell is sourced."
---

Maya2C is a testnet project. Mainnet has not launched. This page compares
signature and transport design with other chains, using only what a dated
search of primary and secondary sources supports. The full record, with the
search boundary and the weight of each source, is the
[prior-art file on GitHub](https://github.com/EricWijesinghe/Maya2C/blob/master/docs/prior-art/pq-l1-comparison-2026-11.md).

## At a glance

| Chain | Signature scheme | Mandatory for all transactions? | Two PQ families required? | PQ peer transport | Status (date) | Src |
|---|---|---|---|---|---|---|
| **Maya2C** | ML-DSA-65 + SLH-DSA-SHA2-128s (ML-DSA-87 opt-in suite) | Default account suite; other suites exist | **Yes**, both must verify | Yes: ML-KEM-1024, ML-KEM-768 fallback, layered over Noise | Testnet. Mainnet not launched | 1 |
| QRL 1.x | XMSS | Yes; spending uses XMSS | No | Not documented | Mainnet since 2018 | 2, 24 |
| QRL 2.0 (Zond) | ML-DSA-87 | not confirmed | not confirmed | not confirmed | Testnet V2 Q1 2026; mainnet gated on audits, "50% complete" (2026-06-26) | 3 |
| Algorand | Falcon: state proofs; Falcon-1024 native accounts; Falcon-512 planned | Opt-in accounts | Planned: hybrid Ed25519 + Falcon accounts, end 2026 | Not addressed | Native PQ accounts "live on Mainnet as of August 2026" (no day given); Falcon LogicSig transaction on mainnet 2025-11-03 | 4, 5, 24 |
| Ethereum | leanXMSS (consensus, planned); Falcon, ML-DSA, SPHINCS+ under evaluation for execution | Not decided | Not decided | None stated | Proposal; no milestone live; team estimate 2029. EIP-8355 (ML-DSA precompiles) Draft, created 2026-07-30, no fork scheduled | 6, 7 |
| Bitcoin | None specified. BIP-360 (P2MR) removes the key-path spend; it does not add a PQ signature scheme | n/a | n/a | n/a | BIP-360 Draft, merged to the BIP repository February 2026 (sources differ on the day); not activated | 8 |
| Solana | Winternitz one-time vault (WOTS) | Opt-in; "not a default feature" | No | None found | Vault introduced via GitHub 2025-01-03; no audit mentioned; mainnet deployment not confirmed | 9 |
| Cardano | Ed25519 stays; CIP-0197 adds ZK seed proofs | Opt-in, proposal | No | None found | CIP-0197 in community review since 2026-08-23; not yet implemented | 10 |
| Aptos | SLH-DSA-SHA2-128s (AIP-137) | Opt-in | No | None found | Proposal awaiting acceptance; page says "no urgency to deploy"; undated | 11 |
| Sui | ML-DSA-65 accounts; SLH-DSA-SHA2-128s in Move vaults | Opt-in | Optional: Ed25519 + ML-DSA-65 multisig | None found | Blog 2026-08-06: nothing live; vaults on mainnet targeted "this year", accounts on testnet end-2026. Devnet ML-DSA-65 reported 2026-10-12 (secondary) | 12, 13 |
| IOTA | Base ledger: no PQ scheme found. Identity library v1.7 (beta): ML-DSA, SLH-DSA, Falcon | Opt-in, identity layer only | Composite ML-DSA + Ed25519 key (identity only) | None found | Base-ledger PQ not found | 14, 15 |
| Mochimo | WOTS+ | Yes, since genesis, no classical path | No | Not documented (one secondary source says plaintext TCP) | PoW, no finality layer. Mainnet since 2018-06-25 (aggregator). No usable independent audit of current code | 16 |
| Abelian | Lattice (Dilithium/Kyber family) with linkable ring signatures | not confirmed | not confirmed | not confirmed | Mainnet "since April 2022" (self-reported). Current status not confirmed | 17 |
| Cellframe | Dilithium, Falcon, Shipovnik, SPHINCS+ (sources conflict on SPHINCS+); Kyber-512 KEM | not confirmed | Dilithium "can run in hybrid mode" (docs) | Kyber-512 listed; whether it protects node transport: not confirmed | Mainnet release announced 2022 (secondary). Current status not confirmed | 18 |
| NEAR | ML-DSA (FIPS 204), account level | Opt-in key rotation | No | Not covered | Mainnet upgrade 2.13, 2026-07-20 (vendor release) | 19 |
| Verona | ML-DSA-65 native account authenticator | Opt-in; optional "PQ-required" policy forces classical + PQ co-signing | Optional, per account | Not covered | Fermi v32 mainnet, about 2026-10-28 (inferred from an image filename). Validator keys to ML-DSA-65 in phase 2, undated | 20 |
| Quantus | ML-DSA-65 and ML-DSA-87 | not confirmed | not confirmed | not confirmed | Mainnet 2026-09-10. Article says four firms reviewed it pre-launch | 21 |
| Naoris | Not named ("NIST-approved") | not confirmed | not confirmed | not confirmed | Mainnet 2026-04-01, invite-only phases. No audit mentioned | 22 |
| QoreChain | ML-DSA-87 signatures; ML-KEM-1024 key encapsulation | not confirmed | Conflicting: one article says hybrid secp256k1 + ML-DSA-87, another omits it | KEM listed; transport use not stated | Mainnet announced for 2026-06-07; PQ transaction 2026-07-02. Company claims | 23 |

Cells marked "not confirmed" were unconfirmed in the search. Cells that rested
on general background knowledge, not on a fetched source, are left out. The
finality column is left out for that reason.

## Where Maya2C differs

Each point is what the search found. "None found" means not found in the
search dated 2026-11-01, not that no chain does it.

1. **Two PQ signature families, both required.** A transaction's signature is decided by its own frame: an untagged frame, which `l1-wallet` signs, is the ML-DSA-65 + SLH-DSA-SHA2-128s hybrid and both must verify; a suite-tagged frame (ML-DSA-87 among others) is opt-in, and the governance parameter `crypto.default_signature_suite` (ML-DSA-87 at genesis) names what wallets should offer, never what verifies.
   None of the 18 comparison rows was found requiring two post-quantum families.
   The nearest designs pair a classical scheme with a PQ one: Sui (optional
   multisig), Verona (optional co-signing policy), Algorand (planned hybrid),
   QoreChain (conflicting sources), Cellframe (hybrid mode per docs). That is a
   different design, not a better or worse one.
2. **Stateless PQ signatures on the default path.** ML-DSA and SLH-DSA need no
   one-time-key state. QRL 1.x (XMSS), Mochimo (WOTS+) and the Solana vault (WOTS)
   use stateful or one-time hash schemes.
3. **A PQ KEM layer on peer transport, documented, with an ML-KEM-768 fallback.**
   It sits above libp2p Noise (X25519), not in place of it. The component
   is "working", not externally reviewed, and its `Required` policy is not
   yet wired to operator configuration. Among rows where transport was
   addressed, none documented a PQ layer. QRL and Mochimo: not documented.
   NEAR, Algorand, Sui, Aptos and Verona: not covered. Cellframe and QoreChain
   list a KEM; deployment is not confirmed.
4. **DAG-BFT finality, on the testnet.** QRL 1.x, Mochimo and Quantus are
   proof-of-work chains with no finality layer stated. Verona's PQ validator
   keys are phase 2, undated.
5. **Mining that does not order blocks.** Miners are paid shares from a genesis
   reserve while validators order blocks ([ADR-044](https://github.com/EricWijesinghe/Maya2C/blob/master/docs/adr/ADR-044-mining-rewards-lane.md)).
   Quantus and Mochimo mine to produce blocks. Status: design, enabled by
   genesis configuration, **not yet on the testnet**.

Not compared: decentralised exchange designs (not searched), and any finality
timing (not measured).

## Where others are ahead

- **Time in production.** QRL 1.x (2018) and Mochimo (2018-06-25, per an
  aggregator) have run post-quantum spending on mainnet for years.
  Abelian reports mainnet since April 2022, self-reported. Maya2C has a testnet.
- **Mainnet post-quantum signatures now.** Algorand (Falcon accounts, August
  2026), NEAR (2026-07-20), Verona (about 2026-10-28), Quantus (2026-09-10),
  Naoris (2026-04-01) and QoreChain (announced for 2026-06-07) all report
  post-quantum signature mainnets. Maya2C does not yet.
- **Audits.** Quantus reports four firms reviewed it before launch. QRL 2.0
  reports 50% of audits complete (June 2026). Maya2C's transport KEM layer is
  marked "not externally reviewed" in the repository. Mochimo has no usable
  independent audit of current code, so audits are not a clear advantage for
  anyone.
- **Installed base.** NEAR, Sui, Aptos, Solana, Cardano and Ethereum have large
  existing user bases. Their post-quantum work is opt-in migration on top of them.
- **Smaller signatures.** Falcon (Algorand; evaluated by Ethereum) is much
  smaller than ML-DSA-65 (3,309 bytes) plus SLH-DSA-SHA2-128s (7,856 bytes).
  Maya2C's dual-signature size cost is real. Its throughput cost is not measured.
- **Standards track.** Ethereum has a dedicated team, a public roadmap and an
  EIP draft (8355). Bitcoin BIP-360 is in the official BIP repository.

## Scope and method

- Chains searched: QRL, Algorand, Ethereum, Bitcoin, Solana, Cardano, Aptos,
  Sui, IOTA, Mochimo, Abelian, Cellframe, and 2026 launches (Quantus, Naoris,
  QoreChain, NEAR, Verona).
- Not searched: other Cosmos-chain post-quantum forks, Hedera, Polkadot/Substrate,
  Tezos and Zcash. One search for BTQ returned nothing.
- Several sites returned errors (403) during the search. Those rows rest on a
  secondary page.
- Maya2C's own column is checked against the repository tree (feature register,
  architecture decision records, transport document) as of 2026-11-01.

## Sources and dates

Weight: P = primary (project documents, BIP/EIP text, official blog);
S = secondary or press release; A = aggregator or index.

1. Maya2C repository: [features.toml](https://github.com/EricWijesinghe/Maya2C/blob/master/features.toml), [transport](https://github.com/EricWijesinghe/Maya2C/blob/master/docs/pq-transport.md), [architecture decision records](https://github.com/EricWijesinghe/Maya2C/tree/master/docs/adr). Checked 2026-11-01.
2. [QRI, Quantum Resistant Ledger](https://qrindex.org/projects/quantum-resistant-ledger/), 2026-08-20 (A).
3. [QRL blog, "Celebrating 8 years"](https://www.theqrl.org/blog/celebrating-8-years.md), 2026-06-26 (P).
4. [Algorand post-quantum roadmap](https://algorand.co/blog/algorand-post-quantum-cryptography-roadmap), 2026-06-18 (P).
5. [Algorand state proofs](https://dev.algorand.co/concepts/protocol/state-proofs/), no date (P).
6. [pq.ethereum.org](https://pq.ethereum.org/), no date (P).
7. [EIP-8355](https://eips.ethereum.org/EIPS/eip-8355), created 2026-07-30 (P).
8. [BIP-360 text](https://github.com/bitcoin/bips/blob/master/bip-0360.mediawiki), merged February 2026 (P).
9. [Quantum Insider, Solana vault](https://thequantuminsider.com/?p=2373792), 2025-01-04 (S).
10. [Coingape, Cardano CIP-0197](https://coingape.com/cardano-opens-review-on-quantum-proof-wallet-cip-as-upgrade-2026-gains-momentum/), 2026-08-24 (S).
11. [AIP-137 (mirror)](https://gregnazario.github.io/AIPs/aips/137/), no date (P).
12. [Sui blog, "Making Sui quantum ready"](https://www.sui.io/blog/making-sui-quantum-ready), 2026-08-06 (P).
13. [CryptoBriefing, Sui devnet](https://cryptobriefing.com/sui-first-quantum-safe-devnet-transaction/), 2026-10-12 (S, search snippet only).
14. [IOTA identity, post-quantum how-to](https://docs.iota.org/developer/iota-identity/how-tos/post-quantum), no date (P).
15. [U.Today, IOTA post-quantum upgrade](https://u.today/iota-implements-post-quantum-upgrade-dominik-schiener-teases-exciting-times-to-come), no date (S).
16. [QRI, Mochimo](https://qrindex.org/projects/mochimo/), 2026-08-20 (A).
17. [Luma, Abelian PQBD 2026 event page](https://luma.com/47tylonu), 2026 (S, self-reported, search snippet only).
18. [Cellframe wiki, encryption protocols](https://wiki.cellframe.net/02.+Learn/Technology/Architecture/Encryption+Protocols), no date (P; does not say what is deployed).
19. [PR Newswire, NEAR mainnet 2.13](https://www.prnewswire.com/news-releases/near-protocol-brings-quantum-safe-signing-to-mainnet-302829646.html), 2026-07-20 (S, vendor).
20. [Yellow, Verona Fermi v32](https://yellow.com/news/verona-launches-fermi-mainnet-v32-with-nist-post-quantum-cryptography), about 2026-10-28 (S).
21. [KuCoin flash, Quantus mainnet](https://www.kucoin.com/news/flash/quantus-launches-mainnet-with-post-quantum-cryptography-from-block-one), 2026-09-10 (S).
22. [Quantum Insider, Naoris mainnet](https://thequantuminsider.com/2026/04/01/naoris-protocol-launches-mainnet-introducing-post-quantum-layer-1-blockchain/), 2026-04-01 (S).
23. [Quantum Computing Report, QoreChain](https://quantumcomputingreport.com/qorechain-implements-complete-nist-post-quantum-cryptography-stack-on-public-mainnet/), 2026-07-03 (S, company claims).
24. [Post-quantum signatures from genesis (repository prior-art file)](https://github.com/EricWijesinghe/Maya2C/blob/master/docs/prior-art/pq-from-genesis.md), searched 2026-09-26.

Full record, with the search boundary and every unconfirmed claim:
[docs/prior-art/pq-l1-comparison-2026-11.md](https://github.com/EricWijesinghe/Maya2C/blob/master/docs/prior-art/pq-l1-comparison-2026-11.md).
