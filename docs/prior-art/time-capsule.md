# Prior art: Time-capsule (messages and payments that open at a future height)

**Search status:** searched 2026-11-10 (web search; sources cited below).

Feature: CATALOG §I row `time-capsule` ([docs/ecosystem/CATALOG.md](../ecosystem/CATALOG.md)). Not built. R0 idea.

**Queries:** "drand tlock timelock encryption time-lock"; "Sarcophagus dead man's switch decentralized encrypted data release"; "post-quantum timelock encryption scheme 2026".

## Who else does something similar

| Project or paper | What it does | Source |
|---|---|---|
| drand tlock | Timelock encryption: data is encrypted to a future drand round and decrypts only once the public randomness beacon publishes that round. Go, TypeScript and Rust libraries; on drand mainnet since March 2023. The docs warn that a threshold of malicious beacon nodes could decrypt future ciphertexts early. Not post-quantum. | [drand timelock docs](https://docs.drand.love/docs/timelock-encryption/), [tlock on fastnet](https://drand.love/blog/2023/03/28/timelock-on-fastnet), [tlock crate](https://docs.rs/crate/tlock/0.0.9) |
| Sarcophagus | Dead man's switch: data is encrypted to a recipient and released by a contract if the owner stops checking in; payloads on Arweave. Sources disagree on the chain (Ethereum in most, Base in one). Current activity not verified. | [techlore forum](https://discuss.techlore.tech/t/sarcophagus-ethereum-dead-mans-switch-for-secret-digital-data-sharing/5597), [Sarcophagus Medium](https://medium.com/sarcophagus-community/sarcophagus-a-dead-mans-switch-92e70de66554) |
| Post-quantum time-lock research | ePrint 2026/2306: isogeny time-lock puzzle, security conjectured in the quantum random oracle model. ePrint 2026/057: timed encryption from isogenies. ePrint 2025/047: lattice time-lock puzzles. All conjectured; no standard, no known deployment. | [2026/2306](https://eprint.iacr.org/2026/2306), [2026/057](https://eprint.iacr.org/2026/057), [2025/047](https://eprint.iacr.org/2025/047) |

## What Maya2C has (checked in repo, 2026-11-10)

- `crates/htlc-lattice/src/timelock.rs`: a claim is admitted while `height < expiry_height`, a refund from `height >= expiry_height`. Heights, never timestamps (invariant 9). This is HTLC settlement. It releases no ciphertext.
- `rg -i "tlock|timelock encrypt|drand"` over `crates`, `docs/adr`: no timelock-encryption code and no ADR.
- Consensus signatures are hybrid ML-DSA-65 + SLH-DSA-SHA2-128s in `crates/node/src/crypto/hybrid.rs`. Transport KEM is ML-KEM (`crates/crypto-pq/src/kem.rs`).

## How a Maya2C version would differ

- **Trust root.** A Maya2C capsule would open at a block height the chain itself commits, with no external randomness beacon. drand's release is trusted to a threshold of beacon nodes; a height release is trusted to chain consensus. Design claim only, not built.
- **Cost of the trade.** The release is only as timely as block production. A stalled chain delays the opening. drand's clock does not depend on any one chain.
- **Post-quantum sealing is research.** Existing tlock is not post-quantum, and the 2026 post-quantum time-lock constructions are conjectured and undeployed. A PQ capsule is R2 until a primitive is chosen and reviewed.

**Verdict: similar exists. Differentiate by chain-committed release height. A post-quantum sealing scheme is not available off the shelf and is not claimed.**

**forbidden**: "first timelock encryption", "only timelock encryption", "post-quantum timelock encryption is live".
