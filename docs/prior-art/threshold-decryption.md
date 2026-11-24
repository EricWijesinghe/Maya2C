# Prior art: Threshold decryption as a contract primitive

**Search status:** searched 2026-11-12 (WebSearch, 2 queries: "threshold decryption for smart contracts Shutter Fairblock Arcium Fhenix encrypted mempool decryption primitive"; "Fairblock conditional decryption MPC; Arcium encrypted computation Solana; Fhenix FHE coprocessor threshold decryption network; Ferveo Anoma" — Arcium did not surface in either and is marked thin).

Feature: parity row P-41, [ADR-128](../adr/ADR-128-threshold-decryption-host-primitive.md).

## Who else does something similar

| Project | What it does | Source |
|---|---|---|
| Shutter | Keyper committee, one key per epoch (~32 blocks); live on Gnosis Chain, OP Stack module on an Optimism testnet; criticised because every transaction of an epoch loses privacy when the epoch key is released. | [iptf.ethereum.org vendor page](https://iptf.ethereum.org/vendors/shutter/), [ePrint 2024/669 (batched threshold decryption)](https://eprint.iacr.org/2024/669.pdf) |
| Ferveo (Anoma/Osmosis) | DKG + threshold public-key encryption for mempool privacy on BFT chains; a key per transaction. | [ePrint 2022/898](https://eprint.iacr.org/2022/898) |
| Fairblock | Threshold IBE: encrypt to a master public key plus a decryption *condition*; validators assemble the key by DKG; researching witness encryption to drop the validator dependency. | [Fairblock Medium](https://medium.com/@Fair_Block/fast-fairy-series-three-cryptographic-schemes-advancing-programmable-privacy-57637c78b0d6) |
| Fhenix CoFHE | Threshold FHE decryption network (MPC-split key shares, rounding instead of noise flooding); CCS 2025 paper; testnets on Arbitrum/Ethereum Sepolia. | [fhenix.io](https://www.fhenix.io/blog/unlocking-private-defi-20-000x-throughput-and-37x-lower-latency-in-threshold-fhe-decryption), [ePrint 2025/1781](https://eprint.iacr.org/2025/1781) |
| Medusa (Protocol Labs) | Committee re-encrypts to authorised recipients under contract rules. | [spark.money glossary](https://www.spark.money/glossary/threshold-encryption) |
| Survey | MEV-mitigation survey: threshold encryption, delay encryption, TEEs. | [arXiv 2407.19572](https://arxiv.org/pdf/2407.19572) |
| Arcium | Encrypted computation on Solana. | not found this round; thin |

## What Maya2C has (checked in repo, 2026-11-12)

- Sealed mempool: `t`-of-`n` ristretto255 committee installed at genesis, never rotated, trusted setup, optional (`crates/mev/src/lib.rs`, `crates/node/src/sealed/mod.rs:79-95`, `genesis.rs:80-92`); per-ciphertext ElGamal-style shares with Chaum–Pedersen proofs (`cipher.rs:250-262`); open-or-expire at a bounded reveal height (`MAX_REVEAL_DELAY = 256`); classical curve justified by a ~one-block confidentiality lifetime (`lib.rs:56-66`).
- No contract-facing primitive; the VM has no decrypt or seal host function.

## How the Maya2C version differs

- A **separate committee record** for contracts (`m:cc:`) with its own share budget, the same crate and cryptography; the mempool's trigger rule and liveness are untouched.
- Reveal is **height-conditioned**, capped at the mempool's 256 blocks so the classical-exception argument still holds; conditions on state are achieved by scheduling, keeping members condition-blind (contrast Fairblock's condition in the ciphertext).
- Plaintext is delivered to the contract by the chain's timer runner (ADR-114), not by an opener who could withhold; no FHE, no MPC over plaintext.

**Verdict: similar exists (Shutter, Ferveo, Fairblock, Fhenix, Medusa). Differentiate by the second-committee separation, the height cap, and chain-delivered reveal. No "first" claim.**
