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
    pub amount_in: u64,
    /// Output the curve gives now.
    pub expected_out: u64,
    /// Floor written into the transaction: it reverts below this.
    pub min_out: u64,
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
    amount_in: u64,
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
    let spot = u128::from(pool.spot_price().map_err(FrontError::Pool)?);
    let scale = PRICE_SCALE;
    let ideal = match direction {
        Direction::BaseToQuote => u128::from(amount_in) * spot / scale,
        Direction::QuoteToBase => u128::from(amount_in) * scale / spot.max(1),
    };
    let impact = (ideal.saturating_sub(u128::from(out)) * u128::from(BPS))
        .checked_div(ideal)
        .unwrap_or(0);
    let impact_bps = u64::try_from(impact).unwrap_or(BPS);
    let floor = u128::from(out) * u128::from(BPS.saturating_sub(slippage_bps)) / u128::from(BPS);
    Ok(Quote {
        amount_in,
        expected_out: out,
        min_out: u64::try_from(floor).unwrap_or(0),
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
    balances: Vec<(Address, u128)>,
) -> Vec<Warning> {
    let ctx = Context {
        verified: [pool].into_iter().collect(),
        contacts: [pool].into_iter().collect(),
        balances,
    };
    review(claimed, simulated, &ctx)
}
