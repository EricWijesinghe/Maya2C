# Prior art: Paid storage markets with possession proofs

**Search status:** searched 2026-11-12 (WebSearch, 2 queries: Filecoin PDP / Arweave endowment, Storj, Walrus). Filecoin's and Walrus's own posts and the Walrus paper are primary; Storj audits were not found and are marked not searched.

Feature: parity row P-36, [ADR-133](../adr/ADR-133-paid-storage-market-with-possession-proofs.md) (OPTIONS FOR ERIC).

## Who else does something similar

| Project | What it does | Source |
|---|---|---|
| Filecoin (Onchain Cloud, PDP) | Proof of Data Possession for hot storage: proofs checked on chain every 24 hours, payments halt when proofs stop; mainnet 2026-03-26 with 49.41 TiB across 478 datasets and 81 payers at launch; two-copy replication; provider SLAs (≥95% success, <1% PDP fault rate) are off-chain terms. Messari: Filecoin Pay annualized run rate about $138K across 149 payers as of 2026-10-17. | [Filecoin: Onchain Cloud live](https://filecoin.io/blog/filecoin-onchain-cloud-is-live-on-mainnet), [Filecoin: introducing PDP](https://filecoin.io/blog/introducing-proof-of-data-possession-pdp-verifiable-hot-storage-on-filecoin), [Messari (secondary)](https://messari.io/report/filecoin-solstice-and-the-shift-to-a-demand-driven-service-economy) |
| Arweave | Pay once; fees go to an endowment that pays miners over 200+ years assuming storage cost declines (about 0.5%/yr); replication by mining incentive, no erasure coding at protocol level; targets about 45 replicas in the pricing. | [Permaweb journal: storage endowment](https://permaweb-journal.ar.io/reference/storage-endowment.html), [economics of large data](https://permaweb-journal.ar.io/article/economics-storing-large-data-on-arweave.html) |
| Walrus (Sui) | Red Stuff two-dimensional erasure coding, about 4.5× replication with self-healing; USD-denominated pricing ($0.023/GB/month, paid in WAL). One secondary source says 80% of cost is foundation-subsidized; the Walrus post does not say. | [Walrus paper arXiv 2505.05370](https://arxiv.org/html/2505.05370v2), [Walrus: USD pricing](https://blog.walrus.xyz/announcing-predictable-pricing-in-usd-on-walrus/), [blockeden (vendor)](https://blockeden.xyz/blog/2026/01/10/walrus-protocol-sui-decentralized-storage-wars/) |
| Storj | Erasure-coded object storage; one comparison lists $0.004/GB/month (early 2025) and notes coordination and repair are run by Storj Labs, a trusted component. Audit mechanism: not searched. | [chainscore comparison (vendor)](https://www.chainscorelabs.com/comparisons/storage-ipfs-vs-arweave-vs-filecoin/large-dataset-archival-solutions/arweave-vs-storj-decentralized-cloud-storage) |

Common finding: every live system proves possession (a challenged piece at a time), not retrieval; retrieval is an SLA outside consensus.

## What Maya2C has (checked in repo, 2026-11-12)

- `crates/archive`: CAR v1 archives of Maya2C's own pruned blocks on local dir, IPFS or an Arweave gateway, verified by CID on read-back; unpaid; REAL, verified (`crates/archive/src/lib.rs:1-17`, `features.toml:2024-2031`).
- `crates/da`: 2D Reed–Solomon square with BLAKE3 row/column roots and `CellProof` Merkle paths — a hash-only possession proof; RESEARCH, working (`crates/da/src/lib.rs:1-27`, `square.rs:112`, `features.toml:3120-3128`).
- ADR-123 (Proposed): blobs with a retention window and a byte fee, bytes local-only and deleted at expiry; no market.

## How a Maya2C version would differ

- The possession proof is the existing `CellProof` (post-quantum by construction: hashes only), challenged by the block beacon at height, never by time (invariant 9).
- Payment from client escrow at block end (ADR-129's pattern), never from issuance (ADR-044); provider bond slashing is an on-chain rule, no oracle (invariant 11).
- No endowment: a cost-decline forecast is a float and a wall-clock assumption, refused in consensus (Standing Order 4).
- Every user-facing string says "possession, not retrieval".

**What may be said publicly:** nothing; no option is chosen. **Forbidden:** "first", "only", "permanent", "guaranteed retrieval".

**Verdict:** similar exists (Filecoin PDP is the same shape). Maya2C's version would differ in the hash-only proof and escrow settlement; demand is MISSING. Options, not a design; Eric decides.
