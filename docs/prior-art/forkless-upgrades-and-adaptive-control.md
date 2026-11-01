# Prior art: forkless protocol upgrades and on-chain adaptive control

**Search status:** searched 2026-11-02 (web search; project documentation and papers, cited below).

Feature: the Omega design's three-speed evolution and bounded integer controllers ([MIP-0002](../mip/MIP-0002-omega-durable-advantage.md), [ADR-078](../adr/ADR-078-omega-durable-advantage.md)).

## Who else does something similar

| Project or work | What it does | Source |
|---|---|---|
| Tezos | On-chain self-amendment: bakers propose, vote on, test and activate protocol upgrades without a hard fork; several upgrades activated this way since 2018 | [Tezos docs](https://docs.tezos.com/architecture/governance) |
| Polkadot / Kusama | The runtime is a Wasm blob in chain state; governance (OpenGov) swaps it (`set_code`), and nodes follow the new logic without a manual upgrade | [Polkadot wiki](https://wiki.polkadot.network/docs/learn-runtime-upgrades), [Kusama guide](https://guide.kusama.network/docs/learn-runtime-upgrades) |
| Ethereum EIP-1559 | A per-block feedback rule moves the base fee toward a gas target | from knowledge, not searched |
| Control-theoretic tokenomics (Akcin et al.) | PID and model-predictive control for token reserves; reports MPC beating a PID baseline in simulation | [arXiv 2210.12881](https://arxiv.org/pdf/2210.12881) |
| Ethereum research: dynamic blob targets | Proposes a PID controller for the blob target | [ethresear.ch](https://ethresear.ch/t/dynamic-blob-targets-for-better-blob-pricing/20687) |
| YieldBasis "Net Pressure" | A deployed PID controller funding a reserve from fees | [YieldBasis docs](https://docs.yieldbasis.com/dev/core/net-pressure) |

## How Maya2C's proposal differs

- **Upgrades:** Tezos and Polkadot let governance install new code. Maya2C's
  slow speed lets governance choose only the **activation height** of code
  already shipped in reviewed node releases. It never runs code from chain
  state (invariant 13). That is more conservative, and it closes a
  governance-installs-code attack.
- **Controllers:** feedback control on chain is established (EIP-1559, PID
  proposals). The difference proposed here is a rule: every adaptive consensus
  parameter uses integer-only law (invariant 20) with a machine-checked output
  bound.

## What may be said publicly

- Accurate: "Maya2C steers its adaptive parameters with integer controllers
  whose bounds are machine-checked" (once O2 is built).
- **forbidden**: "first self-evolving blockchain", "first forkless upgrades", "cannot be surpassed", "unprecedented".
