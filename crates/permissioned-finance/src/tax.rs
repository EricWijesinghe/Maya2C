//! Per-jurisdiction capital-gains calculators, as pure integer functions.
//!
//! **Not tax advice.** These implement the textbook rule of each jurisdiction
//! on a simplified history (one asset, whole units, prices in cents, no fees,
//! no wash-sale or same-day rules). They exist so the rules are tested code
//! instead of prose; `docs/LEGAL_NOTICE.md` applies.
//!
//! | jurisdiction | cost basis | holding-period rule |
//! |---|---|---|
//! | `Us` | FIFO lots | gain split short (≤ 365 days) / long (> 365 days) |
//! | `Uk` | one average-cost pool (Section 104 style) | none |
//! | `De` | FIFO lots | gains on lots held > 365 days are exempt |

use std::collections::VecDeque;

/// A trade.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Trade {
    /// Day number.
    pub day: u32,
    /// True for a buy, false for a sell.
    pub buy: bool,
    /// Units.
    pub units: u64,
    /// Price per unit, cents.
    pub price: i64,
}

/// Which rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Jurisdiction {
    /// United States.
    Us,
    /// United Kingdom.
    Uk,
    /// Germany.
    De,
}

/// A year's result, cents.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Gains {
    /// Gains taxed at the short-term (or only) rate.
    pub taxable_short: i64,
    /// Gains taxed at the long-term rate (US only).
    pub taxable_long: i64,
    /// Gains the jurisdiction exempts (DE holding period).
    pub exempt: i64,
}

/// Why a history is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaxError {
    /// A sell of more units than held.
    Oversold {
        /// Index of the trade.
        at: usize,
    },
}

const YEAR: u32 = 365;

/// Realised gains over `history` (in day order) under `rules`.
///
/// # Errors
///
/// [`TaxError::Oversold`] for a sell exceeding holdings.
pub fn gains(history: &[Trade], rules: Jurisdiction) -> Result<Gains, TaxError> {
    match rules {
        Jurisdiction::Uk => average_cost(history),
        Jurisdiction::Us | Jurisdiction::De => fifo(history, rules),
    }
}

fn fifo(history: &[Trade], rules: Jurisdiction) -> Result<Gains, TaxError> {
    let mut lots: VecDeque<(u32, u64, i64)> = VecDeque::new();
    let mut g = Gains::default();
    for (at, t) in history.iter().enumerate() {
        if t.buy {
            lots.push_back((t.day, t.units, t.price));
            continue;
        }
        let mut left = t.units;
        while left > 0 {
            let Some(front) = lots.front_mut() else {
                return Err(TaxError::Oversold { at });
            };
            let take = front.1.min(left);
            let gain = (t.price - front.2) * i64::try_from(take).unwrap_or(i64::MAX);
            let long = t.day.saturating_sub(front.0) > YEAR;
            match (rules, long) {
                (Jurisdiction::De, true) => g.exempt += gain,
                (Jurisdiction::Us, true) => g.taxable_long += gain,
                _ => g.taxable_short += gain,
            }
            front.1 -= take;
            left -= take;
            if front.1 == 0 {
                lots.pop_front();
            }
        }
    }
    Ok(g)
}

fn average_cost(history: &[Trade]) -> Result<Gains, TaxError> {
    let (mut units, mut cost): (u64, i128) = (0, 0);
    let mut g = Gains::default();
    for (at, t) in history.iter().enumerate() {
        if t.buy {
            units += t.units;
            cost += i128::from(t.price) * i128::from(t.units);
            continue;
        }
        if t.units > units {
            return Err(TaxError::Oversold { at });
        }
        // Allowable cost: the pool's cost pro rata, rounded down.
        let allowable = cost * i128::from(t.units) / i128::from(units);
        let proceeds = i128::from(t.price) * i128::from(t.units);
        g.taxable_short += i64::try_from(proceeds - allowable).unwrap_or(i64::MAX);
        cost -= allowable;
        units -= t.units;
    }
    Ok(g)
}
