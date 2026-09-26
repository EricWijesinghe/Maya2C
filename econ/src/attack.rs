//! Cost of attack (Master Prompt 18 §2).
//!
//! For each attack on a BFT chain: the share of stake it needs, the stake
//! that is at a given staking ratio and supply, and its fiat value at a given
//! price. "Value at risk" is what the attacker must hold, and for attacks
//! that produce slashable evidence, what they lose.

/// An attack and the stake it needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Attack {
    /// Stop finality: withhold votes with more than one third.
    Halt,
    /// Censor: exclude transactions indefinitely. Needs more than one third
    /// to block their certification, more than two thirds to finalize
    /// without the censored proposers.
    Censor,
    /// Finalize two conflicting blocks. Needs more than one third
    /// *equivocating*, and every one of them is slashable by the evidence.
    ConflictingFinality,
    /// Rewrite history for light clients that sync from an old checkpoint,
    /// with keys that have since unbonded — costs nothing at the time of the
    /// attack; weak subjectivity is the defence, not stake.
    LongRange,
}

impl Attack {
    /// Every attack, in report order.
    pub const ALL: [Self; 4] = [
        Self::Halt,
        Self::Censor,
        Self::ConflictingFinality,
        Self::LongRange,
    ];

    /// Name for tables.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Halt => "halt the chain",
            Self::Censor => "censor transactions",
            Self::ConflictingFinality => "finalize conflicting blocks",
            Self::LongRange => "long-range attack on light clients",
        }
    }

    /// Stake needed, ppm of active stake (strictly more than this).
    pub const fn stake_ppm(self) -> u64 {
        match self {
            Self::Halt | Self::ConflictingFinality => 333_334,
            Self::Censor => 666_667,
            Self::LongRange => 0,
        }
    }

    /// Whether the attack leaves slashable on-chain evidence.
    pub const fn slashable(self) -> bool {
        matches!(self, Self::ConflictingFinality)
    }
}

/// Tokens (whole units) an attack needs at a staking ratio and supply.
pub fn tokens_needed(attack: Attack, staking_ppm: u64, supply_tokens: u64) -> u64 {
    let staked = u128::from(supply_tokens) * u128::from(staking_ppm) / 1_000_000;
    // Bounded by supply_tokens, which is a u64.
    u64::try_from(staked * u128::from(attack.stake_ppm()) / 1_000_000).unwrap_or(u64::MAX)
}

/// Weak-subjectivity period, in days, for an unbonding period in days: a node
/// offline longer than this must obtain a checkpoint out of band, because
/// keys that signed before it may have unbonded and be free to sign a fork
/// at no cost. Kept to half the unbonding period so a checkpoint is always
/// inside the window in which equivocation is still slashable.
pub const fn weak_subjectivity_days(unbonding_days: u64) -> u64 {
    unbonding_days / 2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thresholds_are_the_bft_bounds() {
        // 10M supply, half staked: 5M staked; > 1/3 is 1,666,670 tokens.
        assert_eq!(tokens_needed(Attack::Halt, 500_000, 10_000_000), 1_666_670);
        assert_eq!(
            tokens_needed(Attack::Censor, 500_000, 10_000_000),
            3_333_335
        );
        assert_eq!(tokens_needed(Attack::LongRange, 500_000, 10_000_000), 0);
        assert!(Attack::ConflictingFinality.slashable() && !Attack::Halt.slashable());
        assert_eq!(weak_subjectivity_days(21), 10);
    }
}
