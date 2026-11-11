# Prior art: Blob transactions with expiry and data-availability attestations

**Search status:** searched 2026-11-12 (WebSearch, 1 query: "EIP-4844 blob transactions 18 day expiry blob fee market Celestia data availability attestation blobstream"; Celestia/Blobstream did not surface and is cited from the parity survey, thin).

Feature: parity row P-35, [ADR-123](../adr/ADR-123-blob-transactions-and-da-attestations.md).

## Who else does something similar

| Project | What it does | Source |
|---|---|---|
| Ethereum EIP-4844 | Blob-carrying transactions; blobs in a consensus-layer sidecar, kept 4,096 epochs (about 18 days) then deleted; a KZG commitment stays in the execution layer; a second EIP-1559 market over blob gas (target/limit per block; sources disagree on the per-transaction count, check the spec). | [Consensys](https://consensys.io/blog/ethereum-evolved-dencun-upgrade-part-5-eip-4844), [Blocknative](https://www.blocknative.com/blog/eip-4844-blobs-and-blob-gas-what-you-need-to-know), [Chainlink article](https://chain.link/article/eip-4844-blob-storage), [arXiv 2405.03183](https://www.arxiv.org/pdf/2405.03183) |
| Blob demand incident | "BlobScriptions" drove the blob base fee up and caused missed slots. | [Blockworks](https://blockworks.com/news/ethereum-blob-base-fee-surges) |
| Celestia (thin) | 2D Reed–Solomon square, row/column roots in the header, sampling light clients, Blobstream attestations to Ethereum. | parity matrix row; `crates/da/src/lib.rs` cites the construction |

## What Maya2C has (checked in repo, 2026-11-12)

- `crates/da`: 2D RS square, BLAKE3 row/column roots and `data_root`, repair, sampling (`square.rs`), bad-encoding detection without a fraud-proof message; RESEARCH, uncalled by consensus (`lib.rs:25`).
- 5 MiB block cap (invariant 33), per-byte burning base fee (ADR-029), archive-before-delete pruning (invariant 27), fixed-width versioned header (ADR-063).

## How the Maya2C version differs

- **Hash commitments, not KZG**: BLAKE3 roots, post-quantum, larger proofs; validators re-encode the square before voting instead of checking a polynomial opening.
- The availability record (`bl:c:`) is **under the state root**, not in the header, so a layer proof against any finalized header is the attestation and the header layout is unchanged.
- Expiry is a record field; blob bytes are outside invariant 27 by design (not block bodies); retention 12 days as written, Eric's question.
- No L2 consumer exists in this tree; the attestation is a format, and the ADR says so.

**Verdict: similar exists (EIP-4844, Celestia). Differentiate by hash-only commitments and the state-root attestation. No "first" claim.**
