# ADR-068: forever mining — a tail reward the genesis reserve pays to the last height

- Status: Accepted (2026-10-28, Eric: "OK I approve the chart"). Amends
  [ADR-044](ADR-044-mining-rewards-lane.md) and decisions 2 and 4 of
  [ADR-046](ADR-046-universe-scale-supply.md).
- Scope: consensus, pre-genesis. Takes effect at the next re-genesis; the running testnet
  carries no `bft.mining` and is untouched.

## Context

ADR-046 decision 4 promised "a small perpetual tail so miner/validator reward never reaches
zero". The code never had one: `MiningRecord::epoch_reward` halved the reward to zero, as
Bitcoin's does around 2140, after which only fees pay for security. Eric's goal (2026-10-28):
mining that never ends, numbers no other chain beats, and no one gaining a numbers advantage.

Coins are never minted here: rewards are transfers out of `MINING_RESERVE`, funded at
genesis, and the conservation equation depends on that. A tail paid forever therefore needs
a reserve that cannot run dry.

## Decision

1. **Tail reward.** `bft.mining.tail_epoch_reward` (256-bit, default 0). The epoch reward is
   `max(initial >> (epoch / halving_epochs), tail)`. The tail may not exceed the initial
   reward. Mining record format version 4 carries it.
2. **Funded to the last height, still never minted.** With a tail, genesis refuses a reserve
   holding less than `MiningRecord::lifetime_emission()`: an exact upper bound on every unit
   paid from epoch 0 to the last epoch a `u64` height reaches (2^64 − 1 blocks; at one block
   a second, about 5.8 × 10^11 years). The reserve therefore pays the tail for as long as
   the chain can count blocks. A test cross-checks the bound against an epoch-by-epoch sum.
3. **Ceiling 2^192 for lifetime emission.** Any amount below it times any `u64` factor
   (a basis point, a share count, a fee gap) fits 256 bits, so no reward arithmetic can
   overflow. Genesis refuses a schedule at or past it.
4. **Decimals: 24** (amends ADR-046 decision 2, which chose 18 for interop). Decimals are a
   display convention: the chain stores base units and fixes none. The change is in wallets,
   SDKs, explorers, the governance `COIN` and genesis amounts. 21M coins × 10^24 ≈ 2^104.
   Built as its own step; until then, everything is 18.
5. **Fee burn: unchanged.** The base fee is already burned in full (ADR-029, `state/fees.rs`)
   and tips pay validators. That burns more than the 50% Eric approved, so nothing changes.
6. **512-bit intermediates.** Every amount × amount product in consensus goes through a
   512-bit product (`Amount::mul_div`, `full_mul`) or a checked one that refuses. Auditing
   the remaining sites is its own step.

## Consequences

- Rewards never end, and the reserve can never run dry. The "total supply" figure includes
  the whole lifetime reserve, so it is large; circulating supply grows by the schedule, and
  inflation as a share of supply falls every year.
- Worked figure (assumes about 2 s blocks and 3,600-block epochs, as on the testnet): a tail
  of 0.5% of 21M a year is about 24 coins an epoch; block numbers last about 1.2 × 10^12
  years; the genesis reserve is about 1.2 × 10^17 coins, about 2^136 base units at 24
  decimals. The reserve has no key and releases only by the schedule, so it is not a premine.
- Mining record version 4 is unreadable by older nodes and v3 records are refused: re-genesis
  only (rust-reviewer, 2026-10-28). `validate` applies the 2^192 ceiling to every record,
  from genesis or decode (security-reviewer).
- Lost keys are offset over time by the tail.
- Changing the tail after genesis is a protocol upgrade at an activation height, like any
  consensus parameter. The ecosystem stays upgradable that way, not through a live knob that
  a governance majority could turn into inflation.
- **Gap:** `spec/` has no mining rules at all (ADR-044 shipped without them). The tail joins
  that gap; a `MINE-*` rule set with vectors is in BACKLOG.

## Prior art

Monero has paid a fixed tail of 0.6 XMR a block since 2022. Dogecoin pays a fixed reward
per block. Ethereum has no supply cap. Funding the tail from a genesis reserve sized to the
last representable height, rather than by minting, is how this differs; no claim of
"first" is made.
