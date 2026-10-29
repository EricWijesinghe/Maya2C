# 9. Mining rewards — v0.1.0

Active only on a chain whose genesis carries `bft.mining` (ADR-044). Rewards are
transfers out of the mining reserve, never mints. Reference:
`crates/spec-ref/src/mining.rs`. Node: `crates/node/src/state/mining.rs`.

- **MINE-1** The reward of epoch `e` is `max(⌊initial / 2^⌊e / halving_epochs⌋⌋, tail)`; after 256 halvings the halved part is 0, so a chain with `tail = 0` stops paying and one with `tail > 0` pays the tail forever (ADR-068). *(positive only: defines a value)*
- **MINE-2** Each epoch's reward splits into the miners' budget `⌊reward × min(miner_share_bps, 10000) / 10000⌋` and the committee's part, the rest; the two always sum to the reward. *(positive only: defines a value)*
- **MINE-3** The lifetime emission is the sum of MINE-1 over every epoch `0 ..= ⌊(2^64 − 1) / epoch_blocks⌋` (the last a `u64` height reaches). A schedule whose lifetime emission is at or above 2^192 is refused; with `tail > 0`, genesis refuses a reserve holding less than it.

## Gaps

- The share target retarget and the per-share payout (`share_reward`) have no vectors yet.
