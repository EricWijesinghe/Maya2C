//! Sandwich extraction against the two execution models the chain has
//! (Master Prompt 18 §4): the continuous constant-product curve, where order
//! within a block is price, and the per-block batch auction, where every
//! trade in the batch clears at one price. Both are `maya_dex`'s code.

use maya_dex::{Amount, Direction, FeeSchedule, Pool, SwapIntent, clear_batch};

/// A simulated amount: every figure this model runs is a `u64`.
fn a(v: u64) -> Amount {
    Amount::from_u64(v)
}

/// A simulated amount as a signed figure; `None` past `i128`.
fn signed(v: Amount) -> Option<i128> {
    i128::try_from(v.to_u128()?).ok()
}

/// Quote units an attacker gains by sandwiching a victim who buys with
/// `victim_in` quote, front-running with `attack_in` quote. Negative means
/// the attempt loses money.
pub fn sandwich_continuous(pool: Pool, victim_in: u64, attack_in: u64) -> Option<i128> {
    let front = pool.swap_exact_in(Direction::QuoteToBase, a(attack_in)).ok()?;
    let victim = front
        .pool
        .swap_exact_in(Direction::QuoteToBase, a(victim_in))
        .ok()?;
    let back = victim
        .pool
        .swap_exact_in(Direction::BaseToQuote, front.amount_out)
        .ok()?;
    Some(signed(back.amount_out)? - i128::from(attack_in))
}

/// The same attempt inside one batch: the attacker's buy and sell and the
/// victim's buy clear at one uniform price.
pub fn sandwich_batch(pool: Pool, victim_in: u64, attack_in: u64) -> Option<i128> {
    // The attacker's sell must be sized before the price is known; they sell
    // what their buy would have bought on the curve, their best guess.
    let guess = pool
        .swap_exact_in(Direction::QuoteToBase, a(attack_in))
        .ok()?
        .amount_out;
    let intent = |id: u8, direction, amount_in| SwapIntent {
        id: [id; 32],
        trader: [id; 32],
        direction,
        amount_in,
        min_out: Amount::ZERO,
    };
    let out = clear_batch(
        &pool,
        &[
            intent(1, Direction::QuoteToBase, a(attack_in)),
            intent(2, Direction::QuoteToBase, a(victim_in)),
            intent(3, Direction::BaseToQuote, guess),
        ],
    )
    .ok()?;
    let got = |id: u8| {
        out.cleared
            .iter()
            .find(|t| t.id == [id; 32])
            .map(|t| t.amount_out)
    };
    let bought = got(1)?;
    let sold_for = got(3)?;
    // Net: quote received for the sell, minus quote paid for the buy, with
    // any base left over or short valued at the clearing price.
    let base_delta = signed(bought)? - signed(guess)?;
    let scale = signed(maya_dex::PRICE_SCALE)?;
    let base_value = base_delta * signed(out.price)? / scale;
    Some(signed(sold_for)? - i128::from(attack_in) + base_value)
}

/// A pool to run the comparison against.
pub fn reference_pool() -> Pool {
    Pool {
        reserve_base: a(10_000_000_000),
        reserve_quote: a(10_000_000_000),
        total_shares: a(10_000_000_000),
        fees: FeeSchedule::standard(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sandwich_pays_on_the_curve_and_not_in_a_batch() {
        let pool = reference_pool();
        let continuous = sandwich_continuous(pool, 500_000_000, 1_000_000_000).expect("runs");
        let batch = sandwich_batch(pool, 500_000_000, 1_000_000_000).expect("runs");
        assert!(continuous > 0, "continuous sandwich extracts {continuous}");
        assert!(batch <= 0, "batch sandwich extracts {batch}");
    }
}
