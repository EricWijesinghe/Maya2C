---
title: 'Economic security'
editUrl: false
# GENERATED from docs/ECONOMIC_SECURITY.md by scripts/ingest.mjs. Edit the source, not this.
---
What an attack costs, under stated assumptions. From `cargo run --release -p
maya-econ --bin econ-sim` (deterministic, seed 42). Master Prompt 18 §2.

> **Not financial or legal advice, and not a price forecast.** Prices are
> scenario inputs. Token supply, emission and validator costs are the draft
> parameters in `econ/src/params.rs`, not decisions. Token economics, sales
> and legal structure need qualified advisers (`docs/legal/QUESTIONS_FOR_COUNSEL.md`).

## Assumptions

- Supply 10,000,000 tokens (half of `maya_fee_market::MAX_SUPPLY`).
- BFT thresholds as in DAG-BFT (ADR-015): liveness needs more than ⅔ of stake
  honest and online; safety fails when more than ⅓ equivocates.
- **Staking and slashing are not built.** These tables describe the design,
  so they answer "what would it cost" for the network once they exist. Today
  the node runs proof of work, whose cost of attack is rented hash rate, a
  number that cannot be computed from this repository.

## Cost of attack (tokens that must be staked)

| Attack | Stake needed | at 30 % staked | at 50 % | at 70 % | Slashable |
|---|---|---|---|---|---|
| Halt the chain | > ⅓ of stake | 1,000,002 | 1,666,670 | 2,333,338 | no (absence is not provable as malice) |
| Censor transactions | > ⅔ of stake | 2,000,001 | 3,333,335 | 4,666,669 | no |
| Finalize conflicting blocks | > ⅓ of stake equivocating | 1,000,002 | 1,666,670 | 2,333,338 | **yes**: two signatures for one round |
| Long-range attack on light clients | old keys, no current stake | 0 | 0 | 0 | no (keys already unbonded) |

To turn tokens into currency, multiply by a price. That price is the input
the table does not have.

The brief phrases the conflicting-finality attack as "> ⅔". In a BFT
protocol two conflicting quorums of ⅔ must overlap in more than ⅓, so the
stake that must equivocate is **> ⅓**. The simulator uses the correct bound
and this document reports it.

## Long-range attacks and weak subjectivity

Keys that have unbonded can sign an alternative history for free. The
defence is a **weak-subjectivity checkpoint**:

- a new node or light client must obtain a recent finalized block hash
  from a source it trusts, and refuse any chain that does not contain it;
- with a 21-day unbonding period, the simulator's safe checkpoint period is
  **10 days**. A checkpoint older than that may predate keys that have since
  left without penalty;
- obtaining one safely means taking it from several independent operators
  (explorers, exchanges, the client's own previous sync) and requiring them
  to agree. It is never accepted from the peer serving the sync.

## Validator-set health

Nakamoto coefficient (validators needed for ⅓) and Gini on a synthetic
power-law set of 100 validators:

| Stake cap per validator | Nakamoto coefficient | Gini |
|---|---|---|
| none | 3 | 0.618 |
| 5 % | 8 | 0.501 |
| 2 % | 16 | 0.332 |

A cap moves the coefficient more than anything else simulated, but a cap is
evaded by splitting stake across identities. It is not proposed alone.
Delegation incentives toward smaller validators were not simulated. These
metrics are not exposed over RPC, because there is no stake to measure.

## Validator economics

On emission alone at 3 %/yr, with 100 validators costing 24,000 fiat/yr each
and an operator share of 19 %, the break-even token price is **42.105 fiat**.
Below it, the set shrinks to what the budget pays for. Scenario results
(staking ratio, validators, supply, fees): `reports/18-economics.md`.
