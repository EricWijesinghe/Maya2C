# ADR-025: The mainnet MEV position

**Status:** Accepted
**Date:** 2026-09-26

## Context

Master Prompt 18 §4: state which MEV protections are in core, what is left
to the market, and what validators are forbidden to do, with slashing
evidence where detectable. Simulate sandwich and front-running against the
core design.

## What is in core

1. **Uniform-price batch settlement for DEX swaps.** Every swap in a block
   clears at one price (`maya_dex::batch`, `state::dex_exec::settle_trading`),
   so a front-run and a back-run in the same block trade at the victim's
   price. Simulated (`econ`, `maya-dex` pool 10k/10k, victim buys 500):

   | front-run size | continuous curve | batch auction |
   |---|---|---|
   | 100,000,000 | +9,424,022 | −1 |
   | 500,000,000 | +44,322,943 | −1 |
   | 1,000,000,000 | +82,442,253 | −2 |
   | 2,000,000,000 | +144,262,677 | −1 |

   The attacker's sandwich profit goes from growing with size to a loss of a
   rounding unit. `exploit_replays.rs::a_sandwich_around_a_batch_clears_at_one_price`
   pins it on the real apply path.
2. **The sealed (threshold-encrypted) mempool** (`crates/mev`,
   `state::sealed_exec`). The producer who includes an envelope at height *h*
   cannot read it until `reveal_height > h`. It is **optional per network**:
   it needs a committee written into genesis (`SealedGenesis`, absent by
   default), and whoever generated the committee keys held the secret at that
   moment (`crates/mev`, `Committee::generate`). `crates/node/src/genesis.rs`
   points to `docs/sealed-mempool.md`, which does not exist. That dangling
   reference is recorded in `reports/18-economics.md`.

## What is left to the market

- Ordering *between* blocks, and arbitrage that re-aligns prices across
  venues. Removing it would remove price discovery.
- Back-running that does not harm the transaction it follows.

## What validators are forbidden to do

| Behaviour | Detectable? | Evidence |
|---|---|---|
| Signing two different vertices or votes for one round | yes | two signatures by one key for one `(kind, round)`; the remote signer refuses to produce them (`crates/signer`) |
| Reading and front-running sealed envelopes before reveal | not by the chain | prevented cryptographically, not by slashing |
| Censoring a transaction | not provable from one block | out of scope for slashing; bounded by rotating proposers under DAG-BFT |

No slashing exists today, because staking is not built (`spec/06-staking.md`).
The table above says what would be slashable and what evidence would prove
it, for the MIP that builds it.

## Consequences

- Batch settlement protects DEX users on every network. The sealed mempool
  protects only networks that choose to trust a committee setup.
- Front-running a non-DEX transaction (a transfer, a contract call) is not
  addressed by batch settlement. Only the sealed mempool covers it.
