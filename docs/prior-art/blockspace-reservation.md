# Prior art: Blockspace reservation, futures and preconfirmations

**Search status:** searched 2026-11-12 (WebSearch, 1 query: "blockspace futures reservation Ethereum preconfirmations execution tickets Solana leader schedule blockspace reservation"; nothing on Solana surfaced, see `local-fee-markets.md`).

Feature: parity row P-24, [ADR-121](../adr/ADR-121-blockspace-reservation-lanes.md).

## Who else does something similar

| Project | What it does | Source |
|---|---|---|
| Ethereum execution tickets (research) | The beacon chain sells the right to propose future execution blocks; the ticket burns on use; holders can sell forward commitments with collateral. | [ethresear.ch: block-space distribution](https://ethresear.ch/t/on-block-space-distribution-mechanisms/19764), [Justin Drake notes](https://notes.ethereum.org/@JustinDrake/rJ2eXRcKa), [arXiv 2501.16090](https://arxiv.org/abs/2501.16090) |
| Ahead-of-time block auctions / preconfirmations | Pricing future blockspace risk for execution preconfirmations. | [ethresear.ch](https://ethresear.ch/t/ahead-of-time-block-auctions-to-enable-execution-preconfirmations/21345), [vol-market pricing](https://ethresear.ch/t/pricing-ethereum-blocks-with-vol-markets-with-implications-for-preconfirmations/20419) |
| ETHGas (commercial, single source) | A preconfirmation / gas-futures market; its honour-rate claim could not be independently verified by the reporter. | [Cointelegraph](https://cointelegraph.com/news/ethgas-raised-12m-as-it-launches-buterin-proposed-gas-futures) |

## What Maya2C has (checked in repo, 2026-11-12)

- `crates/lanes`: pay-as-bid auction with a per-holder cap and a lane-first single builder (`lib.rs:144-211`), integer-only, not wired (`lib.rs:18-19`).
- DAG-BFT with `n` authors, each packing its own vertex FIFO (`dag-bft/src/validator.rs:97-114`); a deficit round-robin decode walk with a genesis-fixed quantum (ADR-072 CON-13).
- ADR-025 forbids validators selling ordering.

## How the Maya2C version differs

- Nothing is sold about a *proposer*: there is none. A lane is a reserved slice of **every author's quantum** inside the consensus decode walk, so the guarantee is `(n − f)/n` of the reservation under faults, the same bound ADR-072 gives everyone.
- Sold two epochs ahead by an on-chain pay-as-bid auction with compiled share ceilings (50% total, 10% per holder) under governed values; escrow refunded to losers.
- A lane buys certainty, not a fee discount: the base fee still burns per byte.

**Verdict: similar exists (execution tickets, preconfirmations). Differentiate by the per-author slice under DAG-BFT. No "first" claim.**
