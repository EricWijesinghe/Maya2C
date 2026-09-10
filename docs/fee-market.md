# Fee market

A demand-responsive base fee, a stated split of where each fee goes, and a
supply bound that a future emission bug would hit rather than slip past. It is
built, tested, and **switched on nowhere**: every network runs
`FeeConfig::DISABLED`, whose activation height is `u64::MAX`, and nothing in
`src/` calls the crate. `tests/fee_market_tests.rs` checks both.

## What the brief assumed, and what is true

| Brief | This chain | What was built |
|---|---|---|
| base fee follows "block target gas saturation" | there is no block gas; fuel meters contract calls only, as a halting bound | the base fee follows serialized **bytes**, the resource a hybrid signature actually consumes |
| 100% of the tip to "the PoUW miner/validator" | no coinbase, no validators; PoUW is a dark research branch | a one-per-block `FeeClaim` naming a beneficiary — the shape `WorkClaim` already has — so the header does not change |
| burned "from total supply" | a burn credits the unspendable fee sink: `total` is conserved and `circulating` falls | `Supply::after_burn` moves `circulating` only, and the tests assert exactly that |
| a hard maximum supply | nothing mints, so a cap is vacuous today | `MAX_SUPPLY` exists anyway, so the bound is in place before emission ever is |

## The rules

**Base fee.** EIP-1559's control law over block bytes: at target it holds; at
twice the target it rises by exactly one eighth; empty, it falls by one eighth.
A rise is at least one unit, or a fee small enough to round every increase to
zero would stay put under full blocks forever. It never falls below its floor.

**Charge.** A transaction pays `base_fee × its serialized size + tip`, where the
tip is the smaller of what the sender offered and what `max_fee` leaves over. A
`max_fee` that does not cover the base fee makes the block invalid — unlike a
losing trade (invariant 7), this is knowable from the parent when the block is
built, so including it is the producer's error, not a surprise.

**Split.** 80% of the base fee is burned and 20% goes to the treasury; the whole
tip goes to the claimant. The treasury share rounds **down** and the remainder
burns, so no unit is ever created or lost and the only direction a rounding
error moves value is out of circulation. A block with no claim burns its tips.

**Supply.** `MAX_SUPPLY` is 21,000,000 × 10⁸ base units. Nothing in this
repository fixes a decimal convention or a genesis total — the committed
`genesis.json` allocates nothing — so the number is a policy choice recorded in
`fee-market/src/supply.rs`, not a derivation. Once a value-bearing chain checks
it, changing it is a hard fork.

## The bounds no configuration may escape

`fee-market/src/limits.rs`, compiled in and reachable by no transaction, the way
`governance/src/limits.rs` works: a change rate no faster than 1/8 and no slower
than 1/1024 per block, a non-zero fee floor, a block-size target between 64 KiB
(below which one signed transfer is over target on its own) and 16 MiB, and a
treasury share of at most half. `FeeConfig::validate` checks every one, and
`apply_block_fees` refuses an out-of-bounds configuration.

## What activation requires

- Wire `apply_block_fees` into the state transition, with the fee claim beside
  `claim_work` — a consensus change with its own review.
- Move the governable parameters into `governance::ParameterKey`. They are not
  there now because that table is enumerated at genesis and adding keys changes
  every existing chain's genesis state root. From then on they are read from
  state, never from these constants (invariant 17).
- Decide `MAX_SUPPLY` against a real genesis allocation.
- Wallets set `max_fee` and `max_tip` instead of the flat fee-sink output they
  use today.

## Verifying

```powershell
cargo test -p maya-fee-market              # the rules
cargo test --test fee_market_tests         # real transaction sizes, inertness
cargo kani -p maya-fee-market              # no unit created or lost; floor and step
```
