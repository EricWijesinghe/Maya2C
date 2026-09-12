//! The guard's thresholds, and why each is the number it is.
//!
//! Compiled in, and reachable by no [`ParameterKey`]. This is invariant 12's
//! rule applied to the guard: a governable threshold is one proposal away from
//! being set to a value that disables the check, and a monitor an attacker can
//! turn off before the attack is not a monitor.
//!
//! [`ParameterKey`]: crate::governance::ParameterKey

/// Blocks a tripped module stays in emergency read-only mode.
///
/// A hundred blocks is roughly twenty-five minutes at this chain's target
/// spacing — long enough that a human sees the alert and short enough that a
/// false positive is an outage measured in minutes rather than a governance
/// emergency. Nothing re-arms it automatically: it expires, and if the
/// condition still holds the next block trips it again.
pub const BREAKER_BLOCKS: u64 = 100;

/// How far a pool's value per share may fall in one block, in basis points.
///
/// The quantity checked is `k / s²`, where `k` is the product of the reserves
/// and `s` is the issued share count — the value one share redeems for, up to
/// the curve. Under every operation this chain has, it may only rise:
///
/// - a swap adds the LP fee to the reserves and issues no shares;
/// - a deposit rounds the shares it issues *down*;
/// - a withdrawal rounds the reserves it pays out *down*.
///
/// So a fall is not a rounding artefact of the intended rules. It means value
/// left a pool without a matching share being burned, which is the whole
/// AMM-manipulation class: a curve bug, a fee accounted the wrong way, or a
/// sequence of operations that leaves the pool poorer than it started.
///
/// Zero, because there is no legitimate fall to tolerate. Any non-zero value
/// here is a budget an attacker can spend once per block.
pub const POOL_VALUE_TOLERANCE_BPS: u64 = 0;

/// How much of the shielded pool may leave in one block, in basis points.
///
/// [Conservation](super::conservation) cannot see inside the pool: a joinsplit
/// that mints hidden value and then withdraws it balances perfectly at the
/// boundary, because the pool's public balance falls by exactly what the
/// transparent side gains. The only thing the transparent chain can observe
/// about a Groth16 soundness break is the *rate* at which the pool empties.
///
/// So this is a rate limit, not a correctness rule, and it is set high on
/// purpose. Half the pool in a single block is far beyond ordinary traffic —
/// [`MAX_SHIELDED_PER_BLOCK`](crate::state::shielded::MAX_SHIELDED_PER_BLOCK)
/// is 64 joinsplits — while still leaving a large legitimate exit room to
/// clear. The cost of a false positive is that an unusually large withdrawal
/// waits [`BREAKER_BLOCKS`]; the cost of not having it is that a break drains
/// the pool in one block and the first anyone knows is the balance.
pub const SHIELDED_DRAIN_BPS: u64 = 5_000;

/// The pool balance below which the drain rate is not checked at all.
///
/// A rate with no floor is not a safety rule, it is a lever. `before` is the
/// pool's balance immediately prior to the block, so on a thin pool "half of
/// it" is a small absolute number — and anybody holding that much can trip the
/// breaker at will, halting *every other user's* shielded transactions for
/// [`BREAKER_BLOCKS`] and repeating it every window at a cost of one fee. The
/// same arithmetic makes an honest large exit from a young pool a false
/// positive with the whole module as its blast radius.
///
/// The rule the rate implements is "a soundness break should not empty the
/// pool before anyone notices". Below this floor there is nothing worth
/// noticing: the entire loss is bounded by the pool, and halting the subsystem
/// costs more than the value at risk. So the check applies once the pool is
/// large enough for that trade to go the other way.
///
/// The number is a policy choice and is stated as one. This codebase has no
/// decimals constant — every amount is base units — so it is anchored to the
/// largest ordinary amount the tree names: `MAYA_FAUCET_DAILY_CAP` defaults to
/// 1,000,000 base units. A shielded pool holding less than a testnet faucet
/// hands out in a day is not a pool worth halting the network over. Revisit it
/// if the coin's scale is ever fixed.
pub const SHIELDED_DRAIN_FLOOR: u64 = 1_000_000;

/// Basis-point denominator.
pub const BPS_DENOMINATOR: u64 = 10_000;
