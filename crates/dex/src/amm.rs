//! Constant-product automated market maker.
//!
//! # The invariant, stated precisely
//!
//! A swap must never decrease `reserve_base * reserve_quote`. That product is
//! the pool's solvency: it is what guarantees the reserves can honour every
//! outstanding share, and a swap that lowered it would have paid a trader out
//! of the providers' capital rather than out of the curve.
//!
//! Fees are what make it *increase*, and that is the whole mechanism by which
//! providers are paid — see [`crate::fees`].
//!
//! Every function here recomputes the product and refuses to return a pool that
//! violates it. That check is redundant with the arithmetic — the rounding
//! rules make a violation unreachable — and it stays because "unreachable"
//! rests on an argument about integer division that a future edit can quietly
//! break.
//!
//! # A pool cannot be emptied
//!
//! Draining the last unit of a reserve would require infinite input, so
//! [`Pool::swap_exact_out`] rejects an output equal to the whole reserve rather
//! than computing a price for it. This is a property of the curve, not a
//! configured limit.
//!
//! # The locked minimum
//!
//! [`MINIMUM_LIQUIDITY`] shares are minted to nobody on the first deposit. The
//! attack it closes is specific: without it, the first depositor can mint one
//! share, donate a large amount directly into the reserves, and thereby make
//! one share worth so much that every later depositor's contribution rounds
//! down to zero shares and is absorbed. Locking a floor of shares makes the
//! share price bounded from the start, so the rounding never has that much
//! room.

use crate::error::{DexError, Result};
use crate::fees::{FEE_DENOMINATOR, FeeSchedule};
use crate::math::{isqrt_u128, mul_div_ceil, mul_div_floor};
use crate::types::{Direction, PRICE_SCALE};

/// Shares permanently withheld from the first depositor and owned by nobody.
///
/// A thousand: large enough that the share price cannot start at an absurd
/// value, small enough that the deposit it costs is negligible for any pool
/// worth creating. The first deposit must therefore be large enough that the
/// geometric mean of its two sides exceeds this.
pub const MINIMUM_LIQUIDITY: u64 = 1_000;

/// Reserves, issued shares, and the fee split of one constant-product pool.
///
/// Carries no asset identifiers. Which assets a pool trades is a fact about the
/// chain record that holds it; this type is the curve, and keeping it that way
/// is what lets the arithmetic be checked without a notion of an asset at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Pool {
    /// Units of the base asset held.
    pub reserve_base: u64,
    /// Units of the quote asset held.
    pub reserve_quote: u64,
    /// Total shares issued, including the permanently locked
    /// [`MINIMUM_LIQUIDITY`].
    pub total_shares: u64,
    /// How swap fees are split.
    pub fees: FeeSchedule,
}

/// The result of a swap: what moved, and the pool that results.
///
/// The pool is returned rather than mutated in place. A caller that computes a
/// swap and then rejects it — because the trader's slippage bound was missed,
/// or because a later leg of a route failed — must be unable to leave a
/// half-applied pool behind, and the way to make that unable rather than merely
/// unlikely is to never hand out a mutable pool at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SwapOutcome {
    /// Total charged to the trader, fees included.
    pub amount_in: u64,
    /// Delivered to the trader.
    pub amount_out: u64,
    /// Retained in the reserves for the providers.
    pub lp_fee: u64,
    /// Diverted to the protocol fee sink. Never enters the reserves.
    pub protocol_fee: u64,
    /// The pool after the swap.
    pub pool: Pool,
}

/// The result of a deposit or a redemption.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LiquidityDelta {
    /// Base units moved.
    pub base: u64,
    /// Quote units moved.
    pub quote: u64,
    /// Shares minted, or burned on a redemption.
    pub shares: u64,
    /// The pool afterwards.
    pub pool: Pool,
}

impl Pool {
    /// An empty pool with the given fee split.
    #[must_use]
    pub const fn empty(fees: FeeSchedule) -> Self {
        Self {
            reserve_base: 0,
            reserve_quote: 0,
            total_shares: 0,
            fees,
        }
    }

    /// The constant-product invariant's current value.
    #[must_use]
    pub const fn k(&self) -> u128 {
        (self.reserve_base as u128) * (self.reserve_quote as u128)
    }

    /// Whether the pool holds liquidity on both sides.
    #[must_use]
    pub const fn is_funded(&self) -> bool {
        self.reserve_base > 0 && self.reserve_quote > 0 && self.total_shares > 0
    }

    /// Marginal price: quote units per [`PRICE_SCALE`] base units.
    ///
    /// The price of an infinitesimal trade, not of any trade a caller can
    /// actually make. Every real trade executes worse than this, by an amount
    /// that grows with its size — which is the whole content of the curve, and
    /// the reason a quote taken from this function and used as an execution
    /// price would be wrong in the trader's disfavour every time.
    ///
    /// # Errors
    ///
    /// Returns [`DexError::EmptyPool`] if the pool is unfunded, or
    /// [`DexError::Overflow`] if the scaled price exceeds `u64`.
    pub fn spot_price(&self) -> Result<u64> {
        if !self.is_funded() {
            return Err(DexError::EmptyPool);
        }
        mul_div_floor(
            u128::from(self.reserve_quote),
            PRICE_SCALE,
            u128::from(self.reserve_base),
        )
        .ok_or(DexError::Overflow)
    }

    /// The reserves in `(in, out)` order for `direction`.
    const fn oriented(&self, direction: Direction) -> (u64, u64) {
        match direction {
            Direction::BaseToQuote => (self.reserve_base, self.reserve_quote),
            Direction::QuoteToBase => (self.reserve_quote, self.reserve_base),
        }
    }

    /// Rebuilds a pool from oriented reserves.
    const fn with_oriented(&self, direction: Direction, reserve_in: u64, reserve_out: u64) -> Self {
        let (reserve_base, reserve_quote) = match direction {
            Direction::BaseToQuote => (reserve_in, reserve_out),
            Direction::QuoteToBase => (reserve_out, reserve_in),
        };
        Self {
            reserve_base,
            reserve_quote,
            total_shares: self.total_shares,
            fees: self.fees,
        }
    }

    /// Swaps exactly `amount_in` units into the pool.
    ///
    /// The protocol's cut is taken off the input before the curve sees it; the
    /// providers' cut stays inside. See [`crate::fees`] for why the two are
    /// treated differently.
    ///
    /// # Errors
    ///
    /// - [`DexError::ZeroAmount`] for a zero input.
    /// - [`DexError::EmptyPool`] if the pool is unfunded.
    /// - [`DexError::FeeTooHigh`] if the stored fee schedule is out of bounds.
    /// - [`DexError::ZeroOutput`] if the input is so small that the output
    ///   rounds to nothing — rejected rather than silently taking the input.
    /// - [`DexError::Overflow`] if the arithmetic leaves `u64`.
    /// - [`DexError::InvariantViolation`] if the result would lower `k`, which
    ///   is unreachable and therefore a bug rather than a condition.
    pub fn swap_exact_in(&self, direction: Direction, amount_in: u64) -> Result<SwapOutcome> {
        if amount_in == 0 {
            return Err(DexError::ZeroAmount);
        }
        if !self.is_funded() {
            return Err(DexError::EmptyPool);
        }
        if !self.fees.is_valid() {
            return Err(DexError::FeeTooHigh);
        }

        let (reserve_in, reserve_out) = self.oriented(direction);

        // Floor: an inexact protocol cut leaves the remainder with the pool,
        // never with the treasury. The treasury is the party that can afford to
        // lose a unit; the invariant is not.
        let protocol_fee = mul_div_floor(
            u128::from(amount_in),
            u128::from(self.fees.protocol_bps),
            u128::from(FEE_DENOMINATOR),
        )
        .ok_or(DexError::Overflow)?;
        let pool_in = amount_in
            .checked_sub(protocol_fee)
            .ok_or(DexError::Overflow)?;
        if pool_in == 0 {
            return Err(DexError::ZeroOutput);
        }

        // Ceiling: an inexact LP fee rounds toward the reserves.
        let lp_fee = mul_div_ceil(
            u128::from(pool_in),
            u128::from(self.fees.lp_bps),
            u128::from(FEE_DENOMINATOR),
        )
        .ok_or(DexError::Overflow)?;
        let effective_in = pool_in.checked_sub(lp_fee).ok_or(DexError::Overflow)?;
        if effective_in == 0 {
            return Err(DexError::ZeroOutput);
        }

        // The curve. Only `effective_in` prices the trade, but the whole of
        // `pool_in` enters the reserve — that gap is the fee, and it is exactly
        // what makes `k` grow.
        let denominator = u128::from(reserve_in)
            .checked_add(u128::from(effective_in))
            .ok_or(DexError::Overflow)?;
        let amount_out = mul_div_floor(
            u128::from(reserve_out),
            u128::from(effective_in),
            denominator,
        )
        .ok_or(DexError::Overflow)?;
        if amount_out == 0 {
            return Err(DexError::ZeroOutput);
        }

        let new_reserve_in = reserve_in.checked_add(pool_in).ok_or(DexError::Overflow)?;
        let new_reserve_out = reserve_out
            .checked_sub(amount_out)
            .ok_or(DexError::InsufficientLiquidity)?;

        let pool = self.with_oriented(direction, new_reserve_in, new_reserve_out);
        if pool.k() < self.k() {
            return Err(DexError::InvariantViolation);
        }

        Ok(SwapOutcome {
            amount_in,
            amount_out,
            lp_fee,
            protocol_fee,
            pool,
        })
    }

    /// Swaps for exactly `amount_out` units, charging whatever input that
    /// costs.
    ///
    /// Any excess produced by inverting the rounding stays in the reserves, so
    /// the trader receives exactly what was asked for and the pool keeps the
    /// remainder. That is the same rule as everywhere else here, applied to the
    /// one operation where the remainder is visible.
    ///
    /// # Errors
    ///
    /// As [`Pool::swap_exact_in`], plus [`DexError::InsufficientLiquidity`] if
    /// `amount_out` is at least the whole output reserve — a constant-product
    /// pool cannot be emptied at any price.
    pub fn swap_exact_out(&self, direction: Direction, amount_out: u64) -> Result<SwapOutcome> {
        if amount_out == 0 {
            return Err(DexError::ZeroAmount);
        }
        if !self.is_funded() {
            return Err(DexError::EmptyPool);
        }
        if !self.fees.is_valid() {
            return Err(DexError::FeeTooHigh);
        }

        let (reserve_in, reserve_out) = self.oriented(direction);
        if amount_out >= reserve_out {
            return Err(DexError::InsufficientLiquidity);
        }

        // Invert the curve, then invert each fee, ceiling at every step so the
        // trader never underpays.
        let remaining_out = reserve_out - amount_out;
        let effective_in = mul_div_ceil(
            u128::from(reserve_in),
            u128::from(amount_out),
            u128::from(remaining_out),
        )
        .ok_or(DexError::Overflow)?;

        let lp_divisor = FEE_DENOMINATOR
            .checked_sub(self.fees.lp_bps)
            .ok_or(DexError::FeeTooHigh)?;
        let pool_in = mul_div_ceil(
            u128::from(effective_in),
            u128::from(FEE_DENOMINATOR),
            u128::from(lp_divisor),
        )
        .ok_or(DexError::Overflow)?;

        let protocol_divisor = FEE_DENOMINATOR
            .checked_sub(self.fees.protocol_bps)
            .ok_or(DexError::FeeTooHigh)?;
        let mut amount_in = mul_div_ceil(
            u128::from(pool_in),
            u128::from(FEE_DENOMINATOR),
            u128::from(protocol_divisor),
        )
        .ok_or(DexError::Overflow)?;

        // Inverting three ceilings can land a unit short of what the forward
        // path actually delivers, because the forward path floors the output.
        // Rather than reason about exactly when, ask the forward path and step
        // up until it agrees. Each step adds a whole unit of input to a
        // monotonically non-decreasing function, so it terminates; the bound is
        // a named constant so that a future change to the rounding rules fails
        // loudly here instead of spinning.
        const MAX_CORRECTIONS: u32 = 4;
        for _ in 0..MAX_CORRECTIONS {
            let forward = self.swap_exact_in(direction, amount_in)?;
            if forward.amount_out >= amount_out {
                // Deliver exactly what was requested; the surplus the forward
                // path would have paid stays in the reserve it came from.
                let surplus = forward.amount_out - amount_out;
                let pool = match direction {
                    Direction::BaseToQuote => Pool {
                        reserve_quote: forward
                            .pool
                            .reserve_quote
                            .checked_add(surplus)
                            .ok_or(DexError::Overflow)?,
                        ..forward.pool
                    },
                    Direction::QuoteToBase => Pool {
                        reserve_base: forward
                            .pool
                            .reserve_base
                            .checked_add(surplus)
                            .ok_or(DexError::Overflow)?,
                        ..forward.pool
                    },
                };
                if pool.k() < self.k() {
                    return Err(DexError::InvariantViolation);
                }
                return Ok(SwapOutcome {
                    amount_in,
                    amount_out,
                    lp_fee: forward.lp_fee,
                    protocol_fee: forward.protocol_fee,
                    pool,
                });
            }
            amount_in = amount_in.checked_add(1).ok_or(DexError::Overflow)?;
        }

        Err(DexError::InvariantViolation)
    }

    /// Deposits liquidity, minting shares.
    ///
    /// `base_desired` and `quote_desired` are ceilings, not commitments: the
    /// deposit is taken at the pool's current ratio, so at most one of the two
    /// is consumed in full. Taking both in full would move the price, which is
    /// a trade — and letting a deposit be a trade is how a depositor
    /// accidentally pays the spread on their own imbalance.
    ///
    /// The first deposit into an empty pool sets the ratio and therefore *is*
    /// the price. Both amounts are consumed in full, and [`MINIMUM_LIQUIDITY`]
    /// shares are withheld permanently.
    ///
    /// # Errors
    ///
    /// - [`DexError::ZeroAmount`] if either desired amount is zero.
    /// - [`DexError::InsufficientInitialLiquidity`] if a first deposit is too
    ///   small to mint a share once the locked minimum is withheld.
    /// - [`DexError::ZeroOutput`] if a later deposit is too small to mint one
    ///   share, which would otherwise take the assets and give nothing back.
    /// - [`DexError::Overflow`] if the arithmetic leaves `u64`.
    pub fn add_liquidity(&self, base_desired: u64, quote_desired: u64) -> Result<LiquidityDelta> {
        if base_desired == 0 || quote_desired == 0 {
            return Err(DexError::ZeroAmount);
        }

        if !self.is_funded() {
            return self.seed(base_desired, quote_desired);
        }

        // Ceiling on the amount required, so an inexact ratio is paid by the
        // depositor rather than by the pool.
        let quote_for_base = mul_div_ceil(
            u128::from(base_desired),
            u128::from(self.reserve_quote),
            u128::from(self.reserve_base),
        )
        .ok_or(DexError::Overflow)?;

        let (base, quote) = if quote_for_base <= quote_desired {
            (base_desired, quote_for_base)
        } else {
            let base_for_quote = mul_div_ceil(
                u128::from(quote_desired),
                u128::from(self.reserve_base),
                u128::from(self.reserve_quote),
            )
            .ok_or(DexError::Overflow)?;
            (base_for_quote, quote_desired)
        };

        if base == 0 || quote == 0 {
            return Err(DexError::ZeroOutput);
        }

        // Floor on both sides, then the smaller: the depositor is credited for
        // the side they under-supplied, never for the side they over-supplied.
        let by_base = mul_div_floor(
            u128::from(base),
            u128::from(self.total_shares),
            u128::from(self.reserve_base),
        )
        .ok_or(DexError::Overflow)?;
        let by_quote = mul_div_floor(
            u128::from(quote),
            u128::from(self.total_shares),
            u128::from(self.reserve_quote),
        )
        .ok_or(DexError::Overflow)?;
        let shares = by_base.min(by_quote);
        if shares == 0 {
            return Err(DexError::ZeroOutput);
        }

        let pool = Pool {
            reserve_base: self
                .reserve_base
                .checked_add(base)
                .ok_or(DexError::Overflow)?,
            reserve_quote: self
                .reserve_quote
                .checked_add(quote)
                .ok_or(DexError::Overflow)?,
            total_shares: self
                .total_shares
                .checked_add(shares)
                .ok_or(DexError::Overflow)?,
            fees: self.fees,
        };

        // A deposit must never lower the value of an existing share. Rounding
        // the mint down guarantees it; this states the guarantee rather than
        // leaving every reader to re-derive it.
        if !share_price_preserved(self, &pool) {
            return Err(DexError::InvariantViolation);
        }

        Ok(LiquidityDelta {
            base,
            quote,
            shares,
            pool,
        })
    }

    /// The first deposit, which sets the pool's ratio.
    fn seed(&self, base: u64, quote: u64) -> Result<LiquidityDelta> {
        // A partially funded pool cannot exist: reserves and shares are only
        // ever written together. Reaching here with any of the three non-zero
        // means a corrupt record, and seeding on top of it would mint against
        // value that is already there.
        if self.reserve_base != 0 || self.reserve_quote != 0 || self.total_shares != 0 {
            return Err(DexError::InvariantViolation);
        }

        let total = isqrt_u128(u128::from(base) * u128::from(quote)).ok_or(DexError::Overflow)?;
        let shares = total
            .checked_sub(MINIMUM_LIQUIDITY)
            .ok_or(DexError::InsufficientInitialLiquidity)?;
        if shares == 0 {
            return Err(DexError::InsufficientInitialLiquidity);
        }

        Ok(LiquidityDelta {
            base,
            quote,
            shares,
            pool: Pool {
                reserve_base: base,
                reserve_quote: quote,
                // The locked minimum is counted in the supply but issued to
                // nobody, so it can never be redeemed.
                total_shares: total,
                fees: self.fees,
            },
        })
    }

    /// Redeems `shares`, returning a proportional slice of both reserves.
    ///
    /// # Errors
    ///
    /// - [`DexError::ZeroAmount`] for a zero redemption.
    /// - [`DexError::EmptyPool`] if the pool is unfunded.
    /// - [`DexError::InsufficientShares`] if `shares` exceeds what is
    ///   redeemable — the supply less the permanently locked minimum.
    /// - [`DexError::ZeroOutput`] if the redemption would round to nothing on
    ///   either side, which would burn shares for no assets.
    pub fn remove_liquidity(&self, shares: u64) -> Result<LiquidityDelta> {
        if shares == 0 {
            return Err(DexError::ZeroAmount);
        }
        if !self.is_funded() {
            return Err(DexError::EmptyPool);
        }

        // The locked minimum is not redeemable by anyone, so it bounds the
        // redemption rather than merely sitting in the supply.
        let redeemable = self
            .total_shares
            .checked_sub(MINIMUM_LIQUIDITY)
            .ok_or(DexError::InsufficientShares)?;
        if shares > redeemable {
            return Err(DexError::InsufficientShares);
        }

        let base = mul_div_floor(
            u128::from(self.reserve_base),
            u128::from(shares),
            u128::from(self.total_shares),
        )
        .ok_or(DexError::Overflow)?;
        let quote = mul_div_floor(
            u128::from(self.reserve_quote),
            u128::from(shares),
            u128::from(self.total_shares),
        )
        .ok_or(DexError::Overflow)?;

        if base == 0 || quote == 0 {
            return Err(DexError::ZeroOutput);
        }

        let pool = Pool {
            reserve_base: self.reserve_base - base,
            reserve_quote: self.reserve_quote - quote,
            total_shares: self.total_shares - shares,
            fees: self.fees,
        };

        if !share_price_preserved(self, &pool) {
            return Err(DexError::InvariantViolation);
        }

        Ok(LiquidityDelta {
            base,
            quote,
            shares,
            pool,
        })
    }

    /// Adds assets to the reserves without minting shares.
    ///
    /// The batch clearing in [`crate::batch`] settles every trader at one price
    /// and hands the pool whatever the rounding left over. That remainder
    /// belongs to the providers — it came out of trades against their capital —
    /// and the way to give it to them is to raise the share price rather than
    /// to mint anybody new shares.
    ///
    /// # Errors
    ///
    /// Returns [`DexError::Overflow`] if a reserve would exceed `u64`.
    pub fn donate(&self, base: u64, quote: u64) -> Result<Self> {
        Ok(Self {
            reserve_base: self
                .reserve_base
                .checked_add(base)
                .ok_or(DexError::Overflow)?,
            reserve_quote: self
                .reserve_quote
                .checked_add(quote)
                .ok_or(DexError::Overflow)?,
            total_shares: self.total_shares,
            fees: self.fees,
        })
    }
}

/// Whether `after` values each share at least as highly as `before` did.
///
/// Cross-multiplied rather than divided, so it is exact. Comparing two floored
/// quotients would report a fall in share price that is really a rounding
/// artefact, and a false alarm on an invariant is how an invariant gets
/// switched off.
#[must_use]
fn share_price_preserved(before: &Pool, after: &Pool) -> bool {
    // Nothing to compare against on either edge: an unfunded pool has no share
    // price, and a fully redeemed one leaves only the locked minimum's reserves
    // behind, which nobody can claim.
    if before.total_shares == 0 || after.total_shares == 0 {
        return true;
    }

    let base_ok = u128::from(after.reserve_base) * u128::from(before.total_shares)
        >= u128::from(before.reserve_base) * u128::from(after.total_shares);
    let quote_ok = u128::from(after.reserve_quote) * u128::from(before.total_shares)
        >= u128::from(before.reserve_quote) * u128::from(after.total_shares);
    base_ok && quote_ok
}
