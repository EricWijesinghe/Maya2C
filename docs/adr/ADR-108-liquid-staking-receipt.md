# ADR-108: Liquid staking receipt token

**Status:** Proposed, 2026-11-12. Post-genesis (`postgen/v4`), dark until scheduled.
**Date:** 2026-11-12
**Related:** parity row P-6, [prior-art/liquid-staking.md](../prior-art/liquid-staking.md), [ADR-103](ADR-103-native-fungible-tokens.md), [ADR-044](ADR-044-mining-rewards-lane.md), [ADR-046](ADR-046-universe-scale-supply.md), [ADR-082](ADR-082-genesis-committed-upgrade-levers.md); invariants 7, 25, 28, 32; Eric's 256-bit rule (2026-11-01)

## Context

`crates/staking` holds validators, delegations keyed `(delegator,
validator)`, and `Unbonding { owner, validator, amount, release_epoch }`
(`types.rs:33-66`). `Staking::slash` cuts the self bond, every delegation to
the validator and every unbonding entry from it by `bps` (`ops.rs:248-290`);
a double sign tombstones the validator and burns the cut (`ops.rs:229-245`).
The node applies `Effect::Debit/Credit/Burn` (`crates/node/src/state/staking_exec.rs:129-140`)
and pays the epoch reward pool from tips (`fees.rs:14-17`). Delegated stake
is locked for `unbonding_epochs` (7 on devnet, `params.rs:43`).

Multi-asset balances exist with `U256` supply (`crates/node/src/state/asset.rs`),
so a receipt can be an asset. No liquid staking code exists. Lido socializes
slashing across a rebasing token; Jito uses an exchange-rate token over an
audited pool program (prior-art file).

## Decision

**One protocol-owned pool delegates deposits across the active committee by a
deterministic rule and mints an exchange-rate receipt asset. The pool's
delegations are slashed by the existing `Staking::slash`, so a slash lowers
the rate for every holder.**

### State (invariant 25)

Under the staking prefix `k:` (`StateLayer::Staking`, already committed):

- `k:lp` → `Pool { receipt_asset: AssetId, pooled: U256, receipts: U256,
  pending_unbond: Vec<(owner, amount, release_epoch)> }`.
- The pool's delegations are ordinary `(POOL_ADDRESS, validator)` entries in
  `Staking::delegations`; `POOL_ADDRESS` is a derived address nobody holds a
  key for, like `FEE_COLLECTOR` (`fees.rs:37`).
- The receipt is an asset record under `d:asset:` (ADR-103 v2 shape) with
  `authority = POOL_ADDRESS` and `cap = U256::MAX`: only the staking module
  mints or burns it, through `Effect`, never through `MintAsset`. A
  `MintAsset` whose sender is `POOL_ADDRESS` is unsignable, so the authority
  field is a label.

### Operations

| TxKind | Rule |
|---|---|
| `StakeLiquid { amount }` | Debit `amount` native; `receipts_out = amount * receipts / pooled` (first deposit: `receipts_out = amount`); mint to sender; `pooled += amount`; delegate `amount` by the rule below. |
| `UnstakeLiquid { receipts }` | Burn `receipts` from sender; `amount = receipts * pooled / receipts_total`; `pooled -= amount`; undelegate `amount` by the inverse rule; push `(sender, amount, epoch + unbonding_epochs)` to `pending_unbond`. |
| `ClaimUnstaked` | Credit every entry of the sender with `release_epoch <= epoch`; delete them. Nothing due is a **no-op** (invariant 7, as `HtlcRefund`). |

`TransferAsset` moves receipts like any asset; a DEX pool over the receipt is
allowed and is where the "liquid" comes from.

### Delegation rule (deterministic, no floats)

At each epoch boundary (`staking_exec.rs:312` `end_epoch`), after slashing
and rewards, the pool rebalances so that every validator in `Staking::active`
that is not `Tombstoned` or jailed holds `pooled / n` (integer division), the
remainder to the lowest validator id. Validators are iterated in `BTreeMap`
order (`types.rs:108`), never `HashMap`. Rewards credited to `POOL_ADDRESS`
by the epoch payout are added to `pooled`, which is how the rate rises.

### Slashing (the interaction asked for)

`Staking::slash(id, bps)` (`ops.rs:248`) cuts `delegations[(POOL_ADDRESS,
id)]` and any pool unbonding entry from `id` by `bps`, exactly as it cuts a
person's. The pool then sets `pooled = sum of its delegations + its pending
unbonds`, which is lower, so `amount per receipt` falls for every holder at
once. There is no insurance fund and no DAO; the loss is the holders'. A
tombstoned validator's remaining pool stake is undelegated at the next
boundary and redistributed. Downtime jailing (`lib.rs:16`) removes a
validator from the rebalance set until it is unjailed.

### 256-bit arithmetic with a written bound

`amount * receipts / pooled` is one multiplication and one division.
Bound: `amount` and `pooled` are native amounts, each below the total
native supply; the emission schedule (ADR-044, ADR-068) keeps supply far
below `2^128`, and `MAX_SUPPLY` in `crates/fee-market/src/supply.rs:30` is
`Amount::MAX` only because "it bounds nothing today" (`supply.rs:10`).
`receipts <= pooled` always, because the rate never falls below one receipt
per unit except after a slash, where `receipts` can exceed `pooled`; to keep
the product in range, the pool **refuses a deposit or withdrawal whose
`amount` or `receipts` is at or above `2^128`**, with the bound written beside
the check. Then every product is below `2^256` and `checked_mul` cannot
fail. No 512-bit intermediate (Eric, 2026-11-01; option A: convert only with
a written bound).

Rounding: `receipts_out` rounds down on deposit and `amount` rounds down on
withdrawal, so dust stays in the pool and favours remaining holders, never
the leaver.

## Alternatives considered

- **Rebasing receipt (Lido).** Every holder's balance changes every epoch:
  a balance write per holder per epoch, and DEX pools over a rebasing asset
  misprice. Rejected; exchange rate chosen (Jito's shape).
- **Per-validator receipts (Cosmos LSM style).** One asset per validator
  gives holders the slashing risk of one operator and fragments liquidity.
  Rejected for v4.
- **Pool delegates by stake weight rather than evenly.** Concentrates the
  pool on the biggest validators; `crates/staking/src/concentration.rs`
  exists to measure the opposite goal. Even split chosen; a later ADR may
  use the concentration metric.
- **Insurance fund from a cut of rewards.** An economics decision; left as
  an open question, not built.

## Consequences

- Holders share every slash pro rata; wallets must show the live rate and
  that it can fall.
- Three new `TxKind` arms under `Module::Staking` (invariant 28), refused
  from a vault (invariant 32).
- One record `k:lp`; the asset record for the receipt; the rebalance step in
  `end_epoch`.
- The pool's even split changes committee economics slightly: small
  validators receive pool stake equal to large ones.

## Invariants touched

- **25**: `k:lp` under the committed staking prefix; undo covered.
- **7**: `ClaimUnstaked` with nothing due is a no-op.
- **28**: `Module::Staking`.
- **32**: liquid-staking kinds are not transfer doors.
- Standing Order 4: no floats; `BTreeMap` order; heights and epochs only.

## Activation

`FeatureSet::LIQUID_STAKING = 1 << 16`, compiled
`LIQUID_STAKING_ACTIVATION_HEIGHT = u64::MAX`, scheduled under ADR-082 with
the v4 bundle at **genesis + 15,552,000**. Depends on ADR-103's v2 asset
record.

## Test plan

Red tests, `crates/node/tests/liquid_staking_tests.rs`:

- `the_first_deposit_mints_one_receipt_per_unit`
- `a_double_sign_slash_lowers_the_rate_for_every_holder`
- `a_pool_unbonding_entry_is_slashed_like_a_persons`
- `rounding_favours_remaining_holders_on_deposit_and_withdrawal`
- `an_amount_at_two_to_the_128_is_refused_before_multiplying`
- `rebalance_splits_evenly_in_validator_id_order_and_skips_tombstoned`
- `claim_with_nothing_due_is_a_no_op_and_moves_no_root`
- `a_reorg_restores_pooled_and_receipts`

Property test in `crates/staking/tests`: for random deposit, slash and
withdraw sequences, `pooled == sum of pool delegations + pool pending
unbonds` and total receipts conserve. Vectors (`spec/06-staking.md`
LS-1..LS-4): a deposit, a 5,000 bps slash and the resulting rate, a
withdrawal after it.

## Open questions for Eric (economics and pricing only)

1. Pool fee on rewards (bps retained by the treasury or burned), if any.
2. An insurance reserve funded from that fee, or none (as written)?
3. A cap on the pool's share of total stake (Cosmos LSM has a global cap)
   to keep one pool from dominating the committee?
