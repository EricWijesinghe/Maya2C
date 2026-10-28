//! DEX front end: quote, protect, review.
//!
//! What separates a front end from a drainer is three checks before the
//! wallet prompt, all of them here:
//!
//! 1. **Is the token the one the user thinks?** A symbol is not an identity;
//!    the look-alike guard compares contracts.
//! 2. **What will the trade really cost?** The quote comes from the pool's own
//!    curve (`swap_exact_in`), never from the spot price, and carries a
//!    minimum-out floor derived from the user's slippage tolerance.
//! 3. **Does the transaction do what the page claims?** The clear-signing
//!    review compares the effects the page claims with the simulated ones.

use maya_clear_sign::{Address, Context, Effect, Warning, review};
use maya_design_system::guard::{Token, TokenVerdict, token};
use maya_dex::Amount;
use maya_dex::amm::Pool;
use maya_dex::error::DexError;
use maya_dex::types::{Direction, PRICE_SCALE};

/// Basis points in one whole.
pub const BPS: u64 = 10_000;
/// Price impact above which the page makes the user confirm twice.
pub const HIGH_IMPACT_BPS: u64 = 300;

/// A quote ready to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Quote {
    /// Exact input.
    pub amount_in: Amount,
    /// Output the curve gives now.
    pub expected_out: Amount,
    /// Floor written into the transaction: it reverts below this.
    pub min_out: Amount,
    /// Execution price against the marginal price, in basis points.
    pub impact_bps: u64,
    /// Whether the impact needs a second confirmation.
    pub high_impact: bool,
}

/// Why the page refuses to build a transaction.
#[derive(Debug, PartialEq, Eq)]
pub enum FrontError {
    /// The token impersonates a verified one.
    LookAlike(String),
    /// The pool refused the trade.
    Pool(DexError),
}

/// Quotes selling `amount_in` of the side named by `direction`.
///
/// # Errors
///
/// [`FrontError::LookAlike`] if `selling` imitates a verified token;
/// [`FrontError::Pool`] if the pool cannot fill the trade.
pub fn quote(
    pool: &Pool,
    direction: Direction,
    amount_in: Amount,
    slippage_bps: u64,
    selling: Token<'_>,
    verified: &[Token<'_>],
) -> Result<Quote, FrontError> {
    if let TokenVerdict::Impersonates(real) = token(selling, verified) {
        return Err(FrontError::LookAlike(real.to_string()));
    }
    let out = pool
        .swap_exact_in(direction, amount_in)
        .map_err(FrontError::Pool)?
        .amount_out;
    let spot = pool.spot_price().map_err(FrontError::Pool)?;
    let ideal = match direction {
        Direction::BaseToQuote => amount_in.mul_div(spot, PRICE_SCALE),
        Direction::QuoteToBase => amount_in.mul_div(PRICE_SCALE, spot.max(Amount::ONE)),
    }
    .unwrap_or(Amount::MAX);
    let bps = Amount::from_u64(BPS);
    // Impact at most 100%: the shortfall is never above `ideal`.
    let impact = ideal
        .saturating_sub(out)
        .mul_div(bps, ideal)
        .map_or(BPS, |v| v.saturating_to_u64().min(BPS));
    let impact_bps = impact;
    let floor = out
        .mul_div(Amount::from_u64(BPS.saturating_sub(slippage_bps)), bps)
        .unwrap_or(Amount::ZERO);
    Ok(Quote {
        amount_in,
        expected_out: out,
        min_out: floor,
        impact_bps,
        high_impact: impact_bps > HIGH_IMPACT_BPS,
    })
}

/// The wallet-side review of the swap the page submits: what the page claims
/// against what simulation shows.
#[must_use]
pub fn review_swap(
    claimed: &[Effect],
    simulated: &[Effect],
    pool: Address,
    balances: Vec<(Address, Amount)>,
) -> Vec<Warning> {
    let ctx = Context {
        verified: [pool].into_iter().collect(),
        contacts: [pool].into_iter().collect(),
        balances,
    };
    review(claimed, simulated, &ctx)
}
