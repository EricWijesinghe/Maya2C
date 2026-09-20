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
//! # The number is a decision, not a derivation
//!
//! 21,000,000 × 10⁸ base units. Nothing in this repository fixes a decimal
//! convention or a genesis total — the committed `genesis.json` allocates
//! nothing — so there is no figure to derive it from. It is chosen to sit far
//! inside `u64` (≈ 0.011% of it), so no sum of two supplies can overflow. Once
//! a chain carrying value checks it, changing it is a hard fork.

/// The most base units that may ever exist.
pub const MAX_SUPPLY: u64 = 2_100_000_000_000_000;

/// A supply figure outside the bound.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SupplyError {
    /// `total` exceeds [`MAX_SUPPLY`].
    AboveCap {
        /// The offending total.
        total: u64,
    },
    /// More circulating than exists — a bookkeeping error, never a state.
    CirculatingExceedsTotal {
        /// Circulating supply.
        circulating: u64,
        /// Total supply.
        total: u64,
    },
    /// A burn larger than what is circulating.
    BurnExceedsCirculating {
        /// The burn.
        burned: u64,
        /// What was circulating.
        circulating: u64,
    },
}

/// Total and circulating supply, in base units.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Supply {
    /// Everything that exists, including what sits at the fee sink.
    pub total: u64,
    /// Everything that can move: total minus the fee sink.
    pub circulating: u64,
}

impl Supply {
    /// Builds a supply figure, refusing an impossible one.
    ///
    /// # Errors
    ///
    /// [`SupplyError::AboveCap`] or [`SupplyError::CirculatingExceedsTotal`].
    pub fn new(total: u64, circulating: u64) -> Result<Self, SupplyError> {
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
    pub fn after_burn(self, burned: u64) -> Result<Self, SupplyError> {
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
/// # Errors
///
/// [`SupplyError::AboveCap`].
pub fn check_supply(total: u64) -> Result<(), SupplyError> {
    if total > MAX_SUPPLY {
        return Err(SupplyError::AboveCap { total });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cap_admits_itself_and_refuses_one_more() {
        assert_eq!(check_supply(MAX_SUPPLY), Ok(()));
        assert_eq!(
            check_supply(MAX_SUPPLY + 1),
            Err(SupplyError::AboveCap {
                total: MAX_SUPPLY + 1
            })
        );
    }

    #[test]
    fn two_capped_supplies_cannot_overflow_a_u64_when_summed() {
        assert!(MAX_SUPPLY.checked_mul(2).is_some());
    }

    #[test]
    fn a_burn_lowers_circulating_and_leaves_total_alone() {
        let before = Supply::new(1_000, 900).expect("supply");
        let after = before.after_burn(40).expect("burn");
        assert_eq!(
            after,
            Supply {
                total: 1_000,
                circulating: 860
            }
        );
    }

    #[test]
    fn a_burn_larger_than_circulating_is_refused() {
        let s = Supply::new(1_000, 10).expect("supply");
        assert_eq!(
            s.after_burn(11),
            Err(SupplyError::BurnExceedsCirculating {
                burned: 11,
                circulating: 10
            })
        );
    }

    #[test]
    fn more_circulating_than_exists_is_refused() {
        assert!(matches!(
            Supply::new(5, 6),
            Err(SupplyError::CirculatingExceedsTotal { .. })
        ));
    }
}
