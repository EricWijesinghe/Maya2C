//! The timelock rule: which of claim and refund a lock admits at a height.
//!
//! ## Exactly one, at every height
//!
//! A claim is admitted while `height < expiry_height`; a refund once
//! `height ≥ expiry_height`. The two windows partition the heights with no gap
//! and no overlap, so a lock can never be claimed and refunded both, and never
//! be stuck with neither — `proofs.rs` checks both under Kani.
//!
//! Heights, never timestamps (invariant 9): a miner may write any `u64` into a
//! header's timestamp, so a deadline in seconds is a deadline the miner of the
//! deciding block chooses.
//!
//! ## Losing is not an error
//!
//! Every outcome that is not `Claimed` or `Refunded` is a reason the
//! transaction did nothing, and the chain turns each into a no-op rather than
//! an `Err` (invariant 7). A claim racing a refund at the expiry boundary is
//! the ordinary case, not an attack, and the loser must not void the block the
//! winner is in.

/// Where a lock stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LockStatus {
    /// Funds escrowed, awaiting a claim or the expiry.
    Locked,
    /// Paid to the recipient.
    Claimed,
    /// Returned to the sender.
    Refunded,
}

/// What a claim did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClaimOutcome {
    /// The opening verified inside the window; the recipient is paid.
    Claimed,
    /// No lock has this id. A no-op like the rest: the lock may be in a block
    /// the claimer saw and this chain reorganised away.
    UnknownLock,
    /// The lock was already claimed or refunded.
    AlreadySettled,
    /// The claim arrived at or after the expiry height.
    Expired,
    /// The opening does not open this lock's commitment.
    WrongOpening,
}

/// What a refund did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefundOutcome {
    /// The expiry passed with no claim; the sender is repaid.
    Refunded,
    /// No lock has this id.
    UnknownLock,
    /// The lock was already claimed or refunded.
    AlreadySettled,
    /// The expiry height has not been reached.
    NotExpired,
}

impl ClaimOutcome {
    /// Whether value moved.
    #[must_use]
    pub const fn settled(self) -> bool {
        matches!(self, Self::Claimed)
    }
}

impl RefundOutcome {
    /// Whether value moved.
    #[must_use]
    pub const fn settled(self) -> bool {
        matches!(self, Self::Refunded)
    }
}

/// Whether a claim included at `height` is inside the window.
#[must_use]
pub const fn claim_window_open(expiry_height: u64, height: u64) -> bool {
    height < expiry_height
}

/// Decides a claim.
///
/// `opens` is evaluated last and only if everything else admits the claim: a
/// verification is about two million additions, and a claim against a settled
/// or expired lock should not cost the chain one.
pub fn decide_claim(
    status: LockStatus,
    expiry_height: u64,
    height: u64,
    opens: impl FnOnce() -> bool,
) -> ClaimOutcome {
    match status {
        LockStatus::Claimed | LockStatus::Refunded => ClaimOutcome::AlreadySettled,
        LockStatus::Locked if !claim_window_open(expiry_height, height) => ClaimOutcome::Expired,
        LockStatus::Locked if !opens() => ClaimOutcome::WrongOpening,
        LockStatus::Locked => ClaimOutcome::Claimed,
    }
}

/// Decides a refund.
#[must_use]
pub const fn decide_refund(status: LockStatus, expiry_height: u64, height: u64) -> RefundOutcome {
    match status {
        LockStatus::Claimed | LockStatus::Refunded => RefundOutcome::AlreadySettled,
        LockStatus::Locked if claim_window_open(expiry_height, height) => RefundOutcome::NotExpired,
        LockStatus::Locked => RefundOutcome::Refunded,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn the_boundary_belongs_to_the_refund() {
        assert_eq!(
            decide_claim(LockStatus::Locked, 100, 99, || true),
            ClaimOutcome::Claimed
        );
        assert_eq!(
            decide_claim(LockStatus::Locked, 100, 100, || true),
            ClaimOutcome::Expired
        );
        assert_eq!(
            decide_refund(LockStatus::Locked, 100, 99),
            RefundOutcome::NotExpired
        );
        assert_eq!(
            decide_refund(LockStatus::Locked, 100, 100),
            RefundOutcome::Refunded
        );
    }

    #[test]
    fn an_expired_or_settled_claim_never_runs_the_verifier() {
        let panics = || -> bool { unreachable!("verifier ran") };
        assert_eq!(
            decide_claim(LockStatus::Locked, 10, 10, panics),
            ClaimOutcome::Expired
        );
        assert_eq!(
            decide_claim(LockStatus::Refunded, 10, 0, panics),
            ClaimOutcome::AlreadySettled
        );
    }

    #[test]
    fn a_settled_lock_settles_nothing_else() {
        for status in [LockStatus::Claimed, LockStatus::Refunded] {
            for height in [0, 99, 100, u64::MAX] {
                assert_eq!(
                    decide_claim(status, 100, height, || true),
                    ClaimOutcome::AlreadySettled
                );
                assert_eq!(
                    decide_refund(status, 100, height),
                    RefundOutcome::AlreadySettled
                );
            }
        }
    }
}
