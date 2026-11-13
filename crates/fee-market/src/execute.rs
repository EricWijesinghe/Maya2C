//! What a block's fees would do, as a pure function.
//!
//! [`apply_block_fees`] is the state transition's fee rule with the state left
//! out: given the parent's base fee and size and this block's transactions, it
//! returns the base fee, each transaction's charge and split, and the block's
//! totals. Written this way so the rule can be tested exhaustively, on its own,
//! before anything in `state/db.rs` calls it — which nothing yet does.
//!
//! # Paying the producer without a coinbase
//!
//! There is no coinbase and no block reward, so there is nowhere in a block
//! that names who produced it. `state/governance_exec.rs` already solved that
//! for work claims: the producer puts one claim in their own block naming a
//! beneficiary, and the executor allows one per block. [`FeeClaim`] is the same
//! shape, validated the same way, and it avoids a header change — the header is
//! already the subject of an open finding (its transaction list is not
//! committed), and two header changes in flight at once is how one gets rushed.
//!
//! A block with no claim pays its tips to nobody, so they are burned: value is
//! never left unassigned.

use alloc::vec::Vec;

use maya_amount::Amount;

use crate::base_fee::next_base_fee;
use crate::config::{ConfigError, FeeConfig};
use crate::split::{FeeSplit, split};

/// What the parent block contributes to this block's base fee.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParentFees {
    /// The parent's base fee, per byte.
    pub base_fee: Amount,
    /// The parent's serialized size, in bytes.
    pub size_bytes: u64,
}

/// One transaction's fee terms, as its sender set them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TxFee {
    /// Serialized size, in bytes — what the base fee is charged on.
    pub size_bytes: u64,
    /// The most the sender will pay in total, base fee and tip together.
    pub max_fee: Amount,
    /// The most the sender will pay the producer on top of the base fee.
    pub max_tip: Amount,
}

/// A producer naming who receives the block's tips.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FeeClaim {
    /// The account credited with the tips.
    pub beneficiary: [u8; 32],
}

/// One transaction's charge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Charge {
    /// What leaves the sender's balance.
    pub charged: Amount,
    /// Where it goes.
    pub split: FeeSplit,
}

/// The fees of one block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockFeeOutcome {
    /// Below the activation height: nothing is charged under these rules.
    Inactive,
    /// Fees were charged.
    Charged {
        /// This block's base fee, per byte.
        base_fee: Amount,
        /// One entry per transaction, in order.
        charges: Vec<Charge>,
        /// Total sent to the fee sink, including unclaimed tips.
        burned: Amount,
        /// Total credited to the treasury.
        treasury: Amount,
        /// Total credited to the beneficiary.
        tips: Amount,
        /// Who the tips went to, if anyone claimed them.
        beneficiary: Option<[u8; 32]>,
    },
}

/// Why a block's fees cannot be applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeeError {
    /// The configuration is outside the limits.
    Config(ConfigError),
    /// A transaction's `max_fee` does not cover the base fee on its size.
    ///
    /// An error, not a no-op, and deliberately unlike a losing trade
    /// (invariant 7): whether a transaction can pay the base fee is knowable
    /// when the block is built, from the parent alone, so a producer who
    /// includes an underpriced transaction has built an invalid block — not
    /// been surprised by one.
    Underpriced {
        /// The transaction's position in the block.
        index: usize,
        /// What the base fee on its size comes to.
        required: Amount,
        /// What the sender offered.
        offered: Amount,
    },
    /// More than one fee claim in a block.
    DuplicateClaim,
    /// A total overflowed 2²⁵⁶ − 1 — unreachable under [`crate::MAX_SUPPLY`], and
    /// refused rather than wrapped if it is ever reached.
    Overflow,
}

/// Applies the fee rule to one block.
///
/// `parent` is `None` exactly at the activation height, where the base fee is
/// the configuration's initial value.
///
/// # Errors
///
/// Any [`FeeError`]. Every error means the block is invalid under these rules.
pub fn apply_block_fees(
    config: &FeeConfig,
    height: u64,
    parent: Option<ParentFees>,
    txs: &[TxFee],
    claims: &[FeeClaim],
) -> Result<BlockFeeOutcome, FeeError> {
    if !config.is_active(height) {
        return Ok(BlockFeeOutcome::Inactive);
    }
    config.validate().map_err(FeeError::Config)?;
    if claims.len() > 1 {
        return Err(FeeError::DuplicateClaim);
    }

    let base_fee = resolve_base_fee(config, parent);

    let mut charges = Vec::with_capacity(txs.len());
    let (mut burned, mut treasury, mut tips) = (Amount::ZERO, Amount::ZERO, Amount::ZERO);
    for (index, tx) in txs.iter().enumerate() {
        // A price near 2^256 times any size can overflow; no sender can afford it,
        // so it saturates to a requirement nothing meets rather than wrapping.
        let required = base_fee
            .checked_mul(Amount::from_u64(tx.size_bytes))
            .unwrap_or(Amount::MAX);
        if tx.max_fee < required {
            return Err(FeeError::Underpriced {
                index,
                required,
                offered: tx.max_fee,
            });
        }
        let base_paid = required;
        let left_over = tx
            .max_fee
            .checked_sub(base_paid)
            .ok_or(FeeError::Overflow)?;
        let tip = tx.max_tip.min(left_over);
        let s = split(base_paid, tip, config.treasury_bps);

        burned = burned.checked_add(s.burned).ok_or(FeeError::Overflow)?;
        treasury = treasury.checked_add(s.treasury).ok_or(FeeError::Overflow)?;
        tips = tips.checked_add(s.tip).ok_or(FeeError::Overflow)?;
        charges.push(Charge {
            charged: base_paid.checked_add(tip).ok_or(FeeError::Overflow)?,
            split: s,
        });
    }

    let beneficiary = claims.first().map(|c| c.beneficiary);
    if beneficiary.is_none() {
        // Nobody to pay: the tips join the burn rather than vanishing.
        burned = burned.checked_add(tips).ok_or(FeeError::Overflow)?;
        tips = Amount::ZERO;
    }

    Ok(BlockFeeOutcome::Charged {
        base_fee,
        charges,
        burned,
        treasury,
        tips,
        beneficiary,
    })
}

/// The base fee for a block: the configuration's initial value at activation,
/// otherwise the linear step applied to the parent. The neural gain that once
/// sat beside it at activation `u64::MAX` is in `maya-neural-fee`, outside the
/// node (ADR-029 amendment, 2026-11-14).
fn resolve_base_fee(config: &FeeConfig, parent: Option<ParentFees>) -> Amount {
    parent.map_or(config.initial_base_fee, |parent| {
        next_base_fee(
            parent.base_fee,
            parent.size_bytes,
            config.target_block_bytes,
            config.change_denominator,
            config.min_base_fee,
        )
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    const CONFIG: FeeConfig = FeeConfig::TESTING;
    const ALICE: FeeClaim = FeeClaim {
        beneficiary: [0xA1; 32],
    };

    fn a(v: u64) -> Amount {
        Amount::from_u64(v)
    }

    fn tx(size_bytes: u64, max_fee: u64, max_tip: u64) -> TxFee {
        TxFee {
            size_bytes,
            max_fee: a(max_fee),
            max_tip: a(max_tip),
        }
    }

    #[test]
    fn below_activation_nothing_is_charged() {
        let out = apply_block_fees(&FeeConfig::DISABLED, 1_000, None, &[tx(1, 0, 0)], &[]);
        assert_eq!(out, Ok(BlockFeeOutcome::Inactive));
    }

    #[test]
    fn the_activation_block_uses_the_initial_base_fee() {
        let Ok(BlockFeeOutcome::Charged { base_fee, .. }) =
            apply_block_fees(&CONFIG, 1, None, &[], &[])
        else {
            panic!("charged");
        };
        assert_eq!(base_fee, CONFIG.initial_base_fee);
    }

    #[test]
    fn a_transaction_pays_base_fee_times_size_plus_its_tip() {
        // Base fee 10/byte on 100 bytes = 1,000; tip 50 fits under max_fee.
        let Ok(BlockFeeOutcome::Charged {
            charges,
            burned,
            treasury,
            tips,
            ..
        }) = apply_block_fees(&CONFIG, 1, None, &[tx(100, 2_000, 50)], &[ALICE])
        else {
            panic!("charged");
        };
        assert_eq!(charges[0].charged, a(1_050));
        assert_eq!((burned, treasury, tips), (a(800), a(200), a(50)));
    }

    fn base_fee_of(outcome: Result<BlockFeeOutcome, FeeError>) -> Amount {
        match outcome {
            Ok(BlockFeeOutcome::Charged { base_fee, .. }) => base_fee,
            other => panic!("not charged: {other:?}"),
        }
    }

    const PARENT: ParentFees = ParentFees {
        base_fee: Amount::from_u64(800),
        size_bytes: 3 * 1024 * 1024 / 2,
    };

    #[test]
    fn a_parent_steps_the_base_fee_by_the_linear_rule_at_every_height() {
        // Half over target from 800: EIP-1559's step is 800 / 2 / 8 = 50.
        for height in [2, 1_000, u64::MAX - 1] {
            assert_eq!(
                base_fee_of(apply_block_fees(&CONFIG, height, Some(PARENT), &[], &[])),
                a(850)
            );
        }
    }

    #[test]
    fn the_tip_is_capped_by_what_max_fee_leaves_over() {
        let Ok(BlockFeeOutcome::Charged { charges, .. }) =
            apply_block_fees(&CONFIG, 1, None, &[tx(100, 1_020, 500)], &[ALICE])
        else {
            panic!("charged");
        };
        assert_eq!(charges[0].split.tip, a(20));
        assert_eq!(charges[0].charged, a(1_020));
    }

    #[test]
    fn an_underpriced_transaction_makes_the_block_invalid() {
        let out = apply_block_fees(&CONFIG, 1, None, &[tx(100, 999, 0)], &[]);
        assert_eq!(
            out,
            Err(FeeError::Underpriced {
                index: 0,
                required: a(1_000),
                offered: a(999)
            })
        );
    }

    #[test]
    fn unclaimed_tips_are_burned_not_lost() {
        let Ok(BlockFeeOutcome::Charged {
            burned,
            tips,
            beneficiary,
            ..
        }) = apply_block_fees(&CONFIG, 1, None, &[tx(100, 2_000, 50)], &[])
        else {
            panic!("charged");
        };
        assert_eq!((burned, tips, beneficiary), (a(850), Amount::ZERO, None));
    }

    #[test]
    fn two_claims_in_one_block_are_refused() {
        assert_eq!(
            apply_block_fees(&CONFIG, 1, None, &[], &[ALICE, ALICE]),
            Err(FeeError::DuplicateClaim)
        );
    }

    #[test]
    fn a_configuration_outside_the_limits_is_refused_when_active() {
        let bad = FeeConfig {
            treasury_bps: 9_000,
            ..CONFIG
        };
        assert!(matches!(
            apply_block_fees(&bad, 1, None, &[], &[]),
            Err(FeeError::Config(_))
        ));
    }

    #[test]
    fn what_a_block_charges_is_exactly_what_it_distributes() {
        let txs: Vec<TxFee> = (1..=50)
            .map(|i| tx(i * 97, i * 97 * 10 + i * 3, i * 7))
            .collect();
        let Ok(BlockFeeOutcome::Charged {
            charges,
            burned,
            treasury,
            tips,
            ..
        }) = apply_block_fees(&CONFIG, 1, None, &txs, &[ALICE])
        else {
            panic!("charged");
        };
        let charged = charges
            .iter()
            .try_fold(Amount::ZERO, |sum, c| sum.checked_add(c.charged))
            .expect("fits");
        let paid_out = burned
            .checked_add(treasury)
            .and_then(|x| x.checked_add(tips))
            .expect("fits");
        assert_eq!(charged, paid_out);
    }

    #[test]
    fn a_fee_past_u64_is_charged_and_split_exactly() {
        // ADR-046: max_fee is 256-bit; a sender may offer more than 2^64 base
        // units (about 0.0000184 coins at 24 decimals) and the excess is a tip.
        let big = Amount::from_u128(u128::from(u64::MAX) * 3);
        let fee = TxFee {
            size_bytes: 100,
            max_fee: big,
            max_tip: Amount::MAX,
        };
        let Ok(BlockFeeOutcome::Charged { charges, tips, .. }) =
            apply_block_fees(&CONFIG, 1, None, &[fee], &[ALICE])
        else {
            panic!("charged");
        };
        assert_eq!(charges[0].charged, big);
        assert_eq!(Some(tips), big.checked_sub(a(1_000)));
    }
}
