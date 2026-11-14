# Prior art: Verifiable-compute job markets and AI inference receipts

**Search status:** searched 2026-11-12 (WebSearch, 1 query: "verifiable AI inference receipts decentralized compute market Gensyn Bittensor EigenAI Ritual proof of inference 2026"). Vendor blogs are marked as such; production status not verified.

Feature: parity row P-47, [ADR-129](../adr/ADR-129-verifiable-compute-job-market-and-inference-receipts.md); design basis [ADR-079](../adr/ADR-079-verifiable-compute.md).

## Who else does something similar

| Project | What it does | Source |
|---|---|---|
| EigenAI (EigenLayer) | Deterministic inference (containerized runtime pinned to one GPU architecture) plus optimistic re-execution; disputes reduce to byte equality. | [arXiv 2602.00182](https://arxiv.org/html/2602.00182) |
| Gensyn | Verde protocol: refereed delegation with RepOps for bitwise-reproducible operators, needs one honest replica; "Judge" (2026-04) is a quality layer, not a proof (vendor blog). | [survey arXiv 2602.00182 §related](https://arxiv.org/pdf/2602.00182), [blockeden (vendor-adjacent)](https://blockeden.xyz/blog/2026/04/29/gensyn-judge-decentralized-verifiable-ai-evaluation-deai-quality-layer/) |
| Ritual | Verification via TEEs or zk proofs; the survey notes TEE trust assumptions and prohibitive zk cost for large models. | [HadAgent arXiv 2604.18614](https://arxiv.org/pdf/2604.18614) |
| Bittensor | Subnet validators score miners' outputs; a built-in conflict of interest when a subnet scores its own miners; no receipt mechanism found. | [chainscore (blog)](https://www.chainscorelabs.com/blog/ai-x-crypto-agents-compute-and-provenance/cryptoeconomic-ai-model-security/the-coming-standard-proof-of-honest-computation-for-ai) |
| HadAgent | Proof-of-inference consensus for agent serving (academic). | [arXiv 2604.18614](https://arxiv.org/pdf/2604.18614) |
| DeFacts AI (hackathon) | EIP-712 deterministic receipts for a peer-to-peer inference market; prototype. | [ETHGlobal showcase](https://ethglobal.com/showcase/defacts-ai-ntdyu) |

Common finding across sources: providers can substitute cheaper models, quantize aggressively or return cached responses without detection unless the runtime is pinned and replicated.

## What Maya2C has (checked in repo, 2026-11-12)

- ADR-079: job spec, escrow, verification ladder (rung a redundant execution default), AI advisory, no model on a node; no code, no height.
- `WorkClaim { beneficiary }` with **no `kind` byte** (`crates/node/src/core/governance_payload.rs:120-123`), contrary to ADR-044/079's text; `ClaimWork` refused at every height (`state/context.rs:131`).
- `crates/confidential-ai` (RESEARCH): federated training, off-chain.

## How the Maya2C version differs

- Rung (a) as **state records with escrow and bonds**, settled by byte-equality of commitments at block end; the chain never reads a receipt or runs a model (invariants 13, 20, 24).
- Receipts are off-chain signed documents whose hash *is* the commitment; a job class without a deterministic runtime manifest (`k = 1`, advisory) gets receipts and no payment, stating the verified/attested line the survey says vendors blur.
- A new `TxKind::ClaimCompute` instead of a `kind` byte that does not exist; payment from escrow, never a mint (ADR-044).

**Verdict: similar exists (EigenAI's determinism + optimistic model, Gensyn's replication). Differentiate by escrow-settled byte agreement under the chain's determinism invariants and by refusing payment where determinism cannot be declared. No "first" claim.**
