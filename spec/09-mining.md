# 9. Mining rewards — v0.1.0

Active only on a chain whose genesis carries `bft.mining` (ADR-044). Rewards are
transfers out of the mining reserve, never mints. Reference:
`crates/spec-ref/src/mining.rs`. Node: `crates/node/src/state/mining.rs`.

Timing: a miner's share pays when it is applied. The committee's part of epoch
`e` is paid to the fee collector at the boundary block, the first block whose
height is `(e + 1) × epoch_blocks`; height 0 closes nothing. Epoch 0 accepts no
shares, so mining opens at the first boundary (ADR-044). The reserve holds
`MAX_SUPPLY − Σ allocations`, so the cap is the reserve, not a counter.

- **MINE-1** The reward of epoch `e` is `max(⌊initial / 2^⌊e / halving_epochs⌋⌋, tail)`; after 256 halvings the halved part is 0, so a chain with `tail = 0` stops paying and one with `tail > 0` pays the tail forever (ADR-068). *(positive only: defines a value)*
- **MINE-2** Each epoch's reward splits into the miners' budget `⌊reward × min(miner_share_bps, 10000) / 10000⌋` and the committee's part, the rest; the two always sum to the reward. *(positive only: defines a value)*
- **MINE-3** The lifetime emission is the sum of MINE-1 over every epoch `0 ..= ⌊(2^64 − 1) / epoch_blocks⌋` (the last a `u64` height reaches). A schedule whose lifetime emission is at or above 2^192 is refused; with `tail > 0`, genesis refuses a reserve holding less than it.
- **MINE-4** A share pays `min(⌊budget / target_shares⌋, budget − paid)`, where `budget` is MINE-2's miners' part of the epoch's reward and `paid` is what the epoch has paid to miners so far. A share pays zero once the budget is spent, and it still lands. *(positive only: defines a value)*
- **MINE-5** The next epoch's share target is `⌊target × target_shares / observed⌋`. `observed` is the epoch's demand clamped to `[max(⌊target_shares / 4⌋, 1), 4 × target_shares]`; demand is the share count, or `⌊shares × epoch_blocks / spent_after⌋` (saturating at 2^64 − 1) when the budget ran out `spent_after` blocks into the epoch. The result is capped at the target limit (the genesis target) and is never zero. *(positive only: defines a value)*
- **MINE-6** A schedule is refused, at genesis or on decode, when `halving_epochs`, `target_shares`, `epoch_blocks` or `max_shares_per_block` is zero, when `miner_share_bps` exceeds 10000, when `tail_epoch_reward` exceeds `initial_epoch_reward`, or when `spent_after` exceeds `epoch_blocks`; otherwise invalid.
- **MINE-7** When `miner_share_bps > 0`, a schedule is refused if its first share (MINE-4 with nothing paid, in epoch 0) rounds to zero, or if its tail share rounds to zero while the tail is non-zero. A share that pays nothing would have miners working for free.
- **MINE-8** A record's share target is non-zero and no easier than its target limit: `0 < target ≤ target_limit`; otherwise invalid.
- **MINE-9** The header a share's proof of work commits to is the BLAKE3 derive-key hash, under context `maya2c mining share v1`, of `chain_tag ‖ epoch (u64, little-endian) ‖ anchor ‖ miner`. It binds the chain, the epoch, the epoch's anchor and the payee. *(positive only: defines a value)*
- **MINE-10** A share's proof-of-work digest is a share when it is at or below the epoch's target, both read as 256-bit big-endian integers; a digest above the target is refused.

## Gaps

- Share admission (`apply_mining_share`: epoch 0 refused, wrong epoch, spent budget, block cap, a share already paid, the DAG proof of work) is state-level. It is covered by `crates/node/tests/mining_rewards_tests.rs`, not replayed from vectors. The DAG hash itself is specified in `docs/dag-pow.md`.
- Epoch boundaries (the timing paragraph above) and the reserve payout (`pay_from_reserve`: a payout never exceeds the reserve, and emission ends when it is empty) are inline in the node's apply path and have no pure function.
- Genesis funding (the reserve holds at least `initial_epoch_reward`, and at least the lifetime emission when `tail > 0`) is inline in `crates/node/src/genesis.rs`.
- Open question: an epoch with no shares. ADR-044 says its miner budget is burned and not carried; the node leaves it in the keyless reserve, unpaid. Both keep it out of circulation, but the spec does not yet pick one, so no rule states it.
