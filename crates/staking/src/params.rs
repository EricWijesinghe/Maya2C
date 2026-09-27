//! Staking parameters. Consensus: every node must hold the same values, so
//! they come from genesis and change only through governance.

/// Basis points denominator.
pub const BPS: u64 = 10_000;

/// Staking parameters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Params {
    /// Least self bond a validator may register or remain active with.
    pub min_self_bond: u64,
    /// Least amount one delegation may add.
    pub min_delegation: u64,
    /// Largest active set. The DAG-BFT engine's committee is a `u16`, and
    /// every validator verifies every other's signatures each round, so this
    /// bounds per-round work, not only governance.
    pub max_validators: u16,
    /// Epochs between an unbond and the funds' release. The window in which
    /// evidence of an earlier offence can still reach the bond.
    pub unbonding_epochs: u64,
    /// Share of total stake slashed for signing two vertices in one slot.
    pub double_sign_slash_bps: u16,
    /// Share slashed for downtime.
    pub downtime_slash_bps: u16,
    /// A validator that authored fewer than this share of an epoch's rounds
    /// is down for the epoch.
    pub downtime_threshold_bps: u16,
    /// Epochs a downtime-jailed validator sits out.
    pub jail_epochs: u64,
    /// Highest commission a validator may set.
    pub max_commission_bps: u16,
}

impl Params {
    /// Devnet and test defaults. Not mainnet values: those come from the
    /// economics work (Master Prompt 18) and genesis.
    pub const DEVNET: Self = Self {
        min_self_bond: 1_000,
        min_delegation: 10,
        max_validators: 100,
        unbonding_epochs: 7,
        double_sign_slash_bps: 5_000,
        downtime_slash_bps: 100,
        downtime_threshold_bps: 5_000,
        jail_epochs: 2,
        max_commission_bps: 2_000,
    };

    /// Whether every basis-point field is at most 100%.
    #[must_use]
    pub const fn is_valid(&self) -> bool {
        (self.double_sign_slash_bps as u64) <= BPS
            && (self.downtime_slash_bps as u64) <= BPS
            && (self.downtime_threshold_bps as u64) <= BPS
            && (self.max_commission_bps as u64) <= BPS
            && self.max_validators > 0
            && self.min_self_bond > 0
    }
}

/// `amount × bps / 10 000`, rounded down, in `u128` so it cannot overflow.
#[must_use]
pub(crate) fn bps_of(amount: u64, bps: u16) -> u64 {
    let product = u128::from(amount) * u128::from(bps) / u128::from(BPS);
    // `bps ≤ BPS` for every caller (checked by `Params::is_valid`), so the
    // result is at most `amount`.
    u64::try_from(product).unwrap_or(amount)
}
