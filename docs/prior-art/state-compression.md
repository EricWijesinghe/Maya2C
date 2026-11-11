# Prior art: State compression (compressed NFTs, ZK compression)

**Search status:** searched 2026-11-12 (WebSearch, 1 query: "Solana state compression compressed NFT concurrent merkle tree ZK compression Light Protocol").

Feature: parity row P-21, [ADR-118](../adr/ADR-118-compressed-state-collections.md).

## Who else does something similar

| Project | What it does | Source |
|---|---|---|
| Solana compressed NFTs (SPL account compression) | One concurrent Merkle tree root on chain; leaves in the transaction log via a no-op program; indexers rebuild; a changelog buffer lets stale proofs succeed if paths do not intersect. | [Helius: compression on Solana](https://docs.helius.dev/blog/all-you-need-to-know-about-compression-on-solana) |
| Light Protocol ZK compression (now Helius) | Generalises to any account: Poseidon trees, Groth16 proofs (128 bytes) over many compressed accounts; V2 batched trees with a queue (announced at Accelerate 2025). | [ZK compression docs](https://www.mintlify.com/Lightprotocol/light-protocol/concepts/zk-compression), [Light Protocol](https://www.mintlify.com/Lightprotocol/light-protocol) |
| Vendor cost figures (secondary, not measured here) | "Up to 1000x" storage savings claimed. | [spark.money](https://www.spark.money/research/solana-zk-compression-onchain-cost-reduction) |

## What Maya2C has (checked in repo, 2026-11-12)

- One committed record per item/holding (ADR-104 `n:i:`, ADR-103/`dex.rs:236` `d:bal:`); dense accounts tree rebuilt per block (`state/merkle.rs`, ADR-062).
- A BLAKE3 sparse tree with `root` and `open` in `crates/stateless-core/src/sparse.rs:39-53`, switched on for accounts by `STATELESS_ACTIVATION_HEIGHT`.
- No compressed collection, no witness-carrying transaction kind.

## How the Maya2C version differs

- **Hash-based only**: BLAKE3 sparse tree, witness = `depth` hashes; no Groth16, no Poseidon, no trusted setup, post-quantum by construction. The price is a ~1 KiB witness per operation instead of 128 bytes.
- **Collections only**, not accounts: the transfer frame and the stateless-verifiable set are untouched.
- **Root history of 64 blocks** for concurrency, the same idea as Solana's changelog, as committed records deleted in key order.
- `Decompress` is one-way per leaf, so an item can leave the tree and become an ordinary record.

**Verdict: similar exists (Solana cNFTs, Light Protocol). Differentiate by hash-only post-quantum witnesses and the collections-only scope. No "first" claim.**
