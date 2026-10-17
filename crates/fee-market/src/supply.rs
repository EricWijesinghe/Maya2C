//! The supply bound, and what fees do to supply.
//!
//! # A cap on a supply nothing increases
//!
//! There is no emission on this chain: no coinbase, no block reward, nothing
//! that mints. Supply is fixed at genesis, and the only thing that changes any
//! supply figure is value landing at the unspendable fee sink — `total` is
//! conserved and `circulating` falls (`src/rpc/market.rs`).
//!
//! So [`MAX_SUPPLY`] bounds nothing today. It exists so that the day emission
//! is added, the bound is already here and already tested, and an emission bug
//! meets a refusal rather than a quiet overshoot.
//!
//! # The cap is the width of a balance (ADR-046)
//!
//! 2²⁵⁶ − 1 base units: the largest value a 256-bit balance can hold, so no
//! larger cap could be represented by any account at all. It replaced
//! 21,000,000 × 10⁸, which sat far inside `u64`. At 18 decimals `u64::MAX` is
//! about 18 whole coins, so the old width could not carry the decision.
//!
//! A cap this size bounds nothing in practice, by design. What limits real
//! supply is the emission schedule, which ADR-046 decouples from the cap. The
//! check stays for the same reason it always existed: a minting bug meets a
//! refusal (here, an overflow in [`Amount::checked_add`]) instead of a quiet
//! wrap. Once a chain carrying value checks it, changing it is a hard fork.

use maya_amount::Amount;

/// The most base units that may ever exist: 2²⁵⁶ − 1.
pub const MAX_SUPPLY: Amount = Amount::MAX;

/// A supply figure outside the bound.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SupplyError {
    /// `total` exceeds [`MAX_SUPPLY`].
    AboveCap {
        /// The offending total.
        total: Amount,
    },
    /// More circulating than exists — a bookkeeping error, never a state.
    CirculatingExceedsTotal {
        /// Circulating supply.
        circulating: Amount,
        /// Total supply.
        total: Amount,
    },
    /// A burn larger than what is circulating.
    BurnExceedsCirculating {
        /// The burn.
        burned: Amount,
        /// What was circulating.
        circulating: Amount,
    },
}

/// Total and circulating supply, in base units.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Supply {
    /// Everything that exists, including what sits at the fee sink.
    pub total: Amount,
    /// Everything that can move: total minus the fee sink.
    pub circulating: Amount,
}

impl Supply {
    /// Builds a supply figure, refusing an impossible one.
    ///
    /// # Errors
    ///
    /// [`SupplyError::AboveCap`] or [`SupplyError::CirculatingExceedsTotal`].
    pub fn new(total: Amount, circulating: Amount) -> Result<Self, SupplyError> {
        check_supply(total)?;
        if circulating > total {
            return Err(SupplyError::CirculatingExceedsTotal { circulating, total });
        }
        Ok(Self { total, circulating })
    }

    /// Supply after `burned` units reach the fee sink.
    ///
    /// Treasury credits and tips move value between circulating accounts and
    /// change neither figure. Only the burn moves anything, and it moves
    /// `circulating` alone: the units still exist, at an address no key can
    /// spend from.
    ///
    /// # Errors
    ///
    /// [`SupplyError::BurnExceedsCirculating`] — which a correct fee
    /// computation cannot produce, since every burned unit was paid by a
    /// circulating account.
    pub fn after_burn(self, burned: Amount) -> Result<Self, SupplyError> {
        let circulating =
            self.circulating
                .checked_sub(burned)
                .ok_or(SupplyError::BurnExceedsCirculating {
                    burned,
                    circulating: self.circulating,
                })?;
        Ok(Self {
            total: self.total,
            circulating,
        })
    }
}

/// Refuses a total above [`MAX_SUPPLY`].
///
/// With the cap at [`Amount::MAX`] no `Amount` exceeds it, so this always
/// succeeds; it is kept so that a smaller cap, if one is ever chosen before
/// genesis, needs no change at any caller.
///
/// # Errors
///
/// [`SupplyError::AboveCap`].
pub fn check_supply(total: Amount) -> Result<(), SupplyError> {
    if total > MAX_SUPPLY {
        return Err(SupplyError::AboveCap { total });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn units(n: u64) -> Amount {
        Amount::from(n)
    }

    #[test]
    fn the_cap_is_two_to_the_256_minus_one_and_admits_itself() {
        assert_eq!(MAX_SUPPLY, Amount::MAX);
        assert_eq!(check_supply(MAX_SUPPLY), Ok(()));
        // Nothing above the cap can even be built: one more overflows.
        assert_eq!(MAX_SUPPLY.checked_add(units(1)), None);
    }

    #[test]
    fn a_supply_at_the_cap_is_valid_and_burns_like_any_other() {
        let s = Supply::new(MAX_SUPPLY, MAX_SUPPLY).expect("supply at the cap");
        let after = s.after_burn(units(1)).expect("burn");
        assert_eq!(after.total, MAX_SUPPLY);
        assert_eq!(
            after.circulating,
            MAX_SUPPLY.checked_sub(units(1)).expect("sub")
        );
    }

    #[test]
    fn a_burn_lowers_circulating_and_leaves_total_alone() {
        let before = Supply::new(units(1_000), units(900)).expect("supply");
        let after = before.after_burn(units(40)).expect("burn");
        assert_eq!(
            after,
            Supply {
                total: units(1_000),
                circulating: units(860)
            }
        );
    }

    #[test]
    fn a_burn_larger_than_circulating_is_refused() {
        let s = Supply::new(units(1_000), units(10)).expect("supply");
        assert_eq!(
            s.after_burn(units(11)),
            Err(SupplyError::BurnExceedsCirculating {
                burned: units(11),
                circulating: units(10)
            })
        );
    }

    #[test]
    fn more_circulating_than_exists_is_refused() {
        assert!(matches!(
            Supply::new(units(5), units(6)),
            Err(SupplyError::CirculatingExceedsTotal { .. })
        ));
    }
}
