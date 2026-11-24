# Prior art: Succinct chain-state proofs (recursive proofs over checkpoints)

**Search status:** searched 2026-11-12 (WebSearch, 2 queries: "Plonky3 recursion STARK verifier circuit p3-recursion 2026"; "Mina recursive proof of chain state; Ethereum The Verge SNARKified consensus; Starknet S-two STARK recursion; zkBridge succinct light client proof" — the second returned Mina results alone; Ethereum Verge, S-two and zkBridge are from knowledge and marked thin).

Feature: parity row P-31, [ADR-125](../adr/ADR-125-succinct-chain-state-proof.md).

## Who else does something similar

| Project | What it does | Source |
|---|---|---|
| Mina | Every block carries a recursive proof that the state transition and the prior proof are valid; a constant-size chain (figures quoted by third parties: 22 kB chain, 864-byte state proof, ~200 ms verify; a prover needs the full state as witness). | [o1Labs](https://www.o1labs.org/blog/mina-s-superpower-the-ability-to-maintain-a-succinct-verifiable-blockchain), [minaprotocol.com](https://minaprotocol.com/?p=12896), [The Tie](https://www.thetie.io/insights/mina-the-succinct-blockchain) |
| Plonky3-recursion (upstream) | A fixed recursive verifier for `p3-uni-stark` / `p3-batch-stark` proofs, built from Plonky3 primitives with a Poseidon2 `FriRecursionBackend`; its book says active development, unaudited, not for production, not on crates.io (lib.rs lists `p3-circuit-prover`, `p3-fri-air` 0.1.0 from 2025-09). | [book](https://plonky3.github.io/Plonky3-recursion/print.html), [quick start](https://plonky3.github.io/Plonky3-recursion/getting_started/quick_start.html), [construction](https://plonky3.github.io/Plonky3-recursion/architecture_and_internals/construction.html), [lib.rs p3-fri-air](https://lib.rs/crates/p3-fri-air) |
| OpenVM (stark-backend) | Audited STARK backend with recursion for a zkVM; a different stack from this tree's. | [zkSecurity audit](https://reports.zksecurity.xyz/reports/openvm/) |
| Ethereum "The Verge" / zkBridge / Starknet S-two | SNARKified consensus and succinct light-client bridges. | not searched this round; from knowledge, marked thin |

## What Maya2C has (checked in repo, 2026-11-12)

- `crates/zk-stark`: Plonky3 0.7.0 uni-STARKs, Keccak-256 commitments (`config.rs:35-45`), no recursion dependency (`Cargo.toml:14-28`).
- Checkpoints: block id + quorum of ML-DSA-87 attestations (`crates/node/src/consensus/bft/attest.rs:171-215`); finality light client ADR-089 M0 on branch `feat/light-client-finality`, measured 10.8 ms (n = 7) / 152.6 ms (n = 100) per checkpoint.
- CATALOG row `succinct-state-proofs` (W4): "research and an ADR before code".

## How the Maya2C version differs

- The proof is **off-chain and client-side**: nothing on the chain verifies it, unlike Mina's block-carried proof.
- The statement is the epoch-handoff chain of a DAG-BFT committee (ML-DSA-87 quorum), not a PoS/VRF transition; the in-circuit ML-DSA-87 cost is the stated open measurement.
- Recursion is gated on a Poseidon2 config, a measured verifier AIR and a published, reviewed upstream (or this tree's own AIR with two reviewers); until then proofs chain by verification, linear in epochs.

**Verdict: similar exists (Mina; Plonky3-recursion as a component). Differentiate by the off-chain, committee-handoff statement and the written recursion gate. No "first" claim.**
