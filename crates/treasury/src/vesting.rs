//! Allocation vesting: a cliff, then linear release by block height.
//!
//! - Nothing is claimable before `start + cliff`.
//! - From there, `total × (h − start) / duration` has vested, capped at
//!   `total`, computed in `u128` so no allocation size overflows.
//! - **Termination** (an employee leaving) freezes vesting at the height it
//!   happens: what had vested stays claimable, the rest returns to the treasury.
//! - **Clawback** (a governance decision, only if the grant allows it)
//!   returns everything not yet *claimed*, vested or not. Whether a grant is
//!   clawback-eligible is fixed when it is created and cannot be added later.

use maya_amount::Amount;

/// A vesting grant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Grant {
    /// Total tokens granted.
    pub total: Amount,
    /// Height vesting starts.
    pub start: u64,
    /// Blocks before anything vests.
    pub cliff: u64,
    /// Blocks from `start` until everything has vested (≥ `cliff`).
    pub duration: u64,
    /// Whether governance may claw back unclaimed tokens.
    pub clawback_allowed: bool,
    /// Tokens already claimed.
    pub claimed: Amount,
    /// Height vesting was frozen by termination, if it was.
    pub terminated_at: Option<u64>,
    /// Whether the grant was clawed back.
    pub clawed_back: bool,
}

/// Why an operation was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VestingError {
    /// `duration < cliff` or `duration == 0`.
    BadSchedule,
    /// Claiming more than is vested and unclaimed.
    NotVested,
    /// The grant does not allow clawback.
    ClawbackNotAllowed,
    /// The grant was already terminated or clawed back.
    AlreadyClosed,
}

impl Grant {
    /// A new grant.
    ///
    /// # Errors
    ///
    /// [`VestingError::BadSchedule`] for an impossible schedule.
    pub fn new(
        total: Amount,
        start: u64,
        cliff: u64,
        duration: u64,
        clawback_allowed: bool,
    ) -> Result<Self, VestingError> {
        if duration == 0 || duration < cliff {
            return Err(VestingError::BadSchedule);
        }
        Ok(Self {
            total,
            start,
            cliff,
            duration,
            clawback_allowed,
            claimed: Amount::default(),
            terminated_at: None,
            clawed_back: false,
        })
    }

    /// Tokens vested at `height`.
    #[must_use]
    pub fn vested(&self, height: u64) -> Amount {
        if self.clawed_back {
            return self.claimed;
        }
        let h = self.terminated_at.map_or(height, |t| t.min(height));
        if h < self.start.saturating_add(self.cliff) {
            return Amount::default();
        }
        let elapsed = (h - self.start).min(self.duration);
        // The 512-bit product inside `mul_div` cannot overflow; elapsed <= duration
        // keeps the quotient <= total, so the fallback is never taken.
        self.total
            .mul_div(Amount::from_u64(elapsed), Amount::from_u64(self.duration))
            .unwrap_or(self.total)
    }

    /// Tokens claimable now.
    #[must_use]
    pub fn claimable(&self, height: u64) -> Amount {
        self.vested(height).saturating_sub(self.claimed)
    }

    /// Claims `amount`.
    ///
    /// # Errors
    ///
    /// [`VestingError::NotVested`] if `amount` exceeds what is claimable.
    pub fn claim(&mut self, amount: Amount, height: u64) -> Result<(), VestingError> {
        if amount > self.claimable(height) {
            return Err(VestingError::NotVested);
        }
        self.claimed = self.claimed.saturating_add(amount);
        Ok(())
    }

    /// Terminates at `height`: vesting freezes; returns what goes back to the treasury.
    ///
    /// # Errors
    ///
    /// [`VestingError::AlreadyClosed`] if already terminated or clawed back.
    pub fn terminate(&mut self, height: u64) -> Result<Amount, VestingError> {
        if self.terminated_at.is_some() || self.clawed_back {
            return Err(VestingError::AlreadyClosed);
        }
        self.terminated_at = Some(height);
        Ok(self.total.saturating_sub(self.vested(height)))
    }

    /// Claws back everything unclaimed; returns the amount returned.
    ///
    /// # Errors
    ///
    /// [`VestingError::ClawbackNotAllowed`] or [`VestingError::AlreadyClosed`].
    pub fn claw_back(&mut self) -> Result<Amount, VestingError> {
        if !self.clawback_allowed {
            return Err(VestingError::ClawbackNotAllowed);
        }
        if self.clawed_back {
            return Err(VestingError::AlreadyClosed);
        }
        self.clawed_back = true;
        Ok(self.total.saturating_sub(self.claimed))
    }
}
