# ADR-078: Omega design: three-speed evolution, bounded controllers, a test ratchet, published moat indicators

- **Status:** Accepted, 2026-11-02. Eric approved O1–O4 ("I approve everything"). O3 is built now (CI only); O1, O2 and O4 come after mainnet.
- **Full design:** [MIP-0002](../mip/MIP-0002-omega-durable-advantage.md)
- **Prior art:** [forkless-upgrades-and-adaptive-control.md](../prior-art/forkless-upgrades-and-adaptive-control.md), searched 2026-11-02

## Context

Eric asked what Maya2C must become so that no future protocol can surpass it.
No design can guarantee that. Code can be copied, and any benchmark can be
beaten by more hardware. The achievable goal is:
- advantages a fork cannot take: track record, identity, evidence, proofs;
- a way to absorb outside breakthroughs;
- self-steering that every node can re-execute and that is provably bounded.

## Decision

| # | Decision | Recommendation | When |
|---|---|---|---|
| O1 | **Three-speed evolution.** Per-block controllers; bounded governance parameters; code upgrades shipped dormant in node releases and **activated by an on-chain vote**. Code is never fetched from chain state (invariant 13). | yes | after mainnet |
| O2 | **Bounded integer controllers.** Every adaptive consensus parameter has a measured variable, a governed setpoint inside compiled bounds, an integer PI law with anti-windup and a per-block step clamp, and a Kani harness or property test for its bounds. | yes | new controllers; existing ones when touched |
| O3 | **Test ratchet.** Kani harnesses, conformance vectors and exploit replays are never removed. CI fails if their count falls, the way `scripts/lint_debt.sh` ratchets clippy debt. | yes | now (CI only) |
| O4 | **Moat indicators.** Days without halt or loss, replay and corpus counts, identities, harness count, Nakamoto coefficient and HHI, all measured from chain data and shown on the status page. | yes | after mainnet |

## Consequences

- Slow-speed upgrades are coordinated releases, not on-chain code swaps. That
  is less convenient than Tezos or Polkadot, and it removes the attack where
  governance installs code.
- O2 makes every new controller provable at the cost of some design freedom:
  no learned model steers consensus.
- O3 means a wrong test must be fixed rather than deleted. A rename keeps the
  count.
- Three weaknesses stay open until people, time or money fix them: the bus
  factor of one, the single-PC launch, and no independent audit (MIP-0002 §9).
