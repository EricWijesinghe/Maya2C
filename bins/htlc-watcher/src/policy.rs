//! Every decision the watcher makes, as pure functions.
//!
//! Nothing here reads a clock, a socket, or a file. The worker gathers an
//! [`Observation`] and does what [`decide`] returns, so the question "does this
//! lose money?" is answered by tests over plain values.

use thiserror::Error;

use maya_htlc_lattice::{Opening, claim_window_open};

use crate::chain::{LockState, LockView};
use crate::swap::{ChainSide, Leg, LockId, Outcome, Role, Swap};

/// Default confirmations before a settlement counts as final.
pub const DEFAULT_CONFIRMATIONS: u64 = 6;

/// Default blocks allowed between broadcasting a transaction and its inclusion.
pub const DEFAULT_SUBMISSION_BLOCKS: u64 = 3;

/// Safety margins, in blocks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Margins {
    /// Confirmations on the Maya chain.
    pub maya_confirmations: u64,
    /// Confirmations on the counterparty chain.
    pub counterparty_confirmations: u64,
    /// Blocks a broadcast may take to be included.
    pub submission_blocks: u64,
}

impl Default for Margins {
    fn default() -> Self {
        Self {
            maya_confirmations: DEFAULT_CONFIRMATIONS,
            counterparty_confirmations: DEFAULT_CONFIRMATIONS,
            submission_blocks: DEFAULT_SUBMISSION_BLOCKS,
        }
    }
}

impl Margins {
    /// Confirmations on one side.
    #[must_use]
    pub const fn confirmations(&self, side: ChainSide) -> u64 {
        match side {
            ChainSide::Maya => self.maya_confirmations,
            ChainSide::Counterparty => self.counterparty_confirmations,
        }
    }
}

/// An **upper bound** on how many blocks the long-expiry chain produces per
/// block of the short-expiry chain: `long_blocks / per_short_blocks`.
///
/// Heights on two chains do not convert exactly. An upper bound makes the
/// conversion err towards refusing a pairing, never towards accepting one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockRate {
    /// Long-chain blocks…
    pub long_blocks: u64,
    /// …per this many short-chain blocks.
    pub per_short_blocks: u64,
}

impl BlockRate {
    /// Both chains produce blocks at the same rate.
    pub const EQUAL: Self = Self {
        long_blocks: 1,
        per_short_blocks: 1,
    };
}

/// One chain's position in a pairing check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChainPoint {
    /// That chain's tip.
    pub tip: u64,
    /// The lock's expiry on that chain.
    pub expiry_height: u64,
    /// Confirmations the watcher requires there.
    pub confirmations: u64,
}

/// Why a pairing was refused.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum PairingError {
    /// The rate has a zero denominator.
    #[error("block rate has a zero denominator")]
    ZeroRate,
    /// The short leg's expiry is at or below its chain's tip.
    #[error("the short leg expires at or below its chain's tip")]
    ShortLegExpired,
    /// The long leg expires before the short leg's last reveal can be claimed.
    #[error("the long leg must expire above height {required}, but expires at {expiry}")]
    TooTight {
        /// The height the long leg's expiry must exceed.
        required: u64,
        /// The long leg's actual expiry.
        expiry: u64,
    },
    /// The margins overflow a height.
    #[error("the margins overflow a block height")]
    Overflow,
}

/// Checks that a reveal at the short leg's last admissible height leaves time
/// to claim the long leg, returning the spare blocks.
///
/// `tip_long + ceil((expiry_short − tip_short + conf_short) × rate)
///  + submission + conf_long < expiry_long`.
///
/// # Errors
///
/// A [`PairingError`] naming what is unsafe.
pub fn check_pairing(
    long: ChainPoint,
    short: ChainPoint,
    rate: BlockRate,
    submission_blocks: u64,
) -> Result<u64, PairingError> {
    if rate.per_short_blocks == 0 {
        return Err(PairingError::ZeroRate);
    }
    if short.expiry_height <= short.tip {
        return Err(PairingError::ShortLegExpired);
    }
    let short_blocks =
        u128::from(short.expiry_height - short.tip) + u128::from(short.confirmations);
    let converted =
        (short_blocks * u128::from(rate.long_blocks)).div_ceil(u128::from(rate.per_short_blocks));
    let required = u128::from(long.tip)
        + converted
        + u128::from(submission_blocks)
        + u128::from(long.confirmations);
    let required = u64::try_from(required).map_err(|_| PairingError::Overflow)?;
    if required >= long.expiry_height {
        return Err(PairingError::TooTight {
            required,
            expiry: long.expiry_height,
        });
    }
    Ok(long.expiry_height - required)
}

/// What both chains report about one swap.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Observation {
    /// Tip of the chain the inbound lock is on.
    pub inbound_tip: u64,
    /// Tip of the chain the outbound lock is on.
    pub outbound_tip: u64,
    /// The inbound lock, if the swap has one and its chain holds it.
    pub inbound: Option<LockView>,
    /// The outbound lock, if its chain holds it.
    pub outbound: Option<LockView>,
}

/// Something the watcher should report but cannot fix.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Alert {
    /// A journalled lock is not on its chain: not yet mined, or reorganised
    /// away.
    LockMissing(ChainSide),
    /// A lock's commitment is not the swap's.
    CommitmentMismatch(ChainSide),
    /// Initiator: the secret for this swap has not been supplied.
    SecretMissing,
    /// Initiator: too close to the inbound expiry to reveal safely. Nothing is
    /// broadcast.
    RevealWindowClosed,
    /// Responder: the opening is public and the inbound lock expired unclaimed.
    ClaimWindowClosed,
}

/// What to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Submit a claim.
    Claim {
        /// Which chain.
        side: ChainSide,
        /// Which lock.
        lock_id: LockId,
        /// The opening to publish.
        opening: Opening,
    },
    /// Submit a refund.
    Refund {
        /// Which chain.
        side: ChainSide,
        /// Which lock.
        lock_id: LockId,
    },
    /// Mark the swap finished.
    Finish(Outcome),
    /// Report a condition.
    Alert(Alert),
}

/// Decides one swap.
///
/// `secret` is the initiator's opening, if the operator supplied the secret.
#[must_use]
pub fn decide(
    swap: &Swap,
    observation: &Observation,
    secret: Option<&Opening>,
    margins: &Margins,
) -> Vec<Action> {
    let Some(outbound) = &observation.outbound else {
        return vec![Action::Alert(Alert::LockMissing(swap.outbound.side))];
    };
    if outbound.commitment_id != swap.commitment_id {
        return vec![Action::Alert(Alert::CommitmentMismatch(swap.outbound.side))];
    }
    let Some(leg) = swap.inbound else {
        return unpaired(swap, observation.outbound_tip, outbound, margins);
    };
    let Some(inbound) = &observation.inbound else {
        return vec![Action::Alert(Alert::LockMissing(leg.side))];
    };
    if inbound.commitment_id != swap.commitment_id {
        return vec![Action::Alert(Alert::CommitmentMismatch(leg.side))];
    }

    let inbound_buried = buried(
        inbound,
        observation.inbound_tip,
        margins.confirmations(leg.side),
    );
    let outbound_buried = buried(
        outbound,
        observation.outbound_tip,
        margins.confirmations(swap.outbound.side),
    );
    if inbound_buried && outbound_buried {
        return vec![Action::Finish(outcome(inbound, outbound))];
    }

    let mut actions = Vec::new();
    actions.extend(claim(
        swap,
        leg,
        observation.inbound_tip,
        inbound,
        outbound,
        secret,
        margins,
    ));
    actions.extend(refund(swap, observation.outbound_tip, outbound));
    actions
}

/// An initiator's swap nobody has answered: only the refund matters.
fn unpaired(swap: &Swap, tip: u64, outbound: &LockView, margins: &Margins) -> Vec<Action> {
    if buried(outbound, tip, margins.confirmations(swap.outbound.side)) {
        // Claimed without a responder means the secret left this watcher
        // some other way.
        let outcome = if matches!(outbound.state, LockState::Claimed { .. }) {
            Outcome::Lost
        } else {
            Outcome::Unwound
        };
        return vec![Action::Finish(outcome)];
    }
    refund(swap, tip, outbound).into_iter().collect()
}

/// The inbound claim, if one is due.
fn claim(
    swap: &Swap,
    leg: Leg,
    tip: u64,
    inbound: &LockView,
    outbound: &LockView,
    secret: Option<&Opening>,
    margins: &Margins,
) -> Option<Action> {
    if inbound.state != LockState::Locked {
        return None;
    }
    // A broadcast now lands in the next block at the earliest.
    let next = tip.saturating_add(1);
    let claim = |opening: &Opening| Action::Claim {
        side: leg.side,
        lock_id: leg.lock_id,
        opening: opening.clone(),
    };
    match swap.role {
        Role::Responder => {
            let LockState::Claimed { opening, .. } = &outbound.state else {
                return None;
            };
            Some(if claim_window_open(inbound.expiry_height, next) {
                claim(opening)
            } else {
                Action::Alert(Alert::ClaimWindowClosed)
            })
        }
        Role::Initiator => {
            let Some(opening) = secret else {
                return Some(Action::Alert(Alert::SecretMissing));
            };
            // Revealing is safe only if the claim can land *and* be buried
            // before the expiry. A claim that lands after it does nothing and
            // still publishes the opening, which costs both legs.
            let latest = next
                .saturating_add(margins.submission_blocks)
                .saturating_add(margins.confirmations(leg.side));
            Some(if claim_window_open(inbound.expiry_height, latest) {
                claim(opening)
            } else {
                Action::Alert(Alert::RevealWindowClosed)
            })
        }
    }
}

/// The outbound refund, if one is due.
fn refund(swap: &Swap, tip: u64, outbound: &LockView) -> Option<Action> {
    if outbound.state != LockState::Locked
        || claim_window_open(outbound.expiry_height, tip.saturating_add(1))
    {
        return None;
    }
    Some(Action::Refund {
        side: swap.outbound.side,
        lock_id: swap.outbound.lock_id,
    })
}

/// Whether a lock settled at least `confirmations` blocks deep.
fn buried(view: &LockView, tip: u64, confirmations: u64) -> bool {
    view.settled_height()
        .is_some_and(|height| tip.saturating_sub(height).saturating_add(1) >= confirmations)
}

/// How two settled legs ended, from this watcher's side.
fn outcome(inbound: &LockView, outbound: &LockView) -> Outcome {
    let inbound_claimed = matches!(inbound.state, LockState::Claimed { .. });
    let outbound_claimed = matches!(outbound.state, LockState::Claimed { .. });
    match (inbound_claimed, outbound_claimed) {
        (true, true) => Outcome::Swapped,
        (false, false) => Outcome::Unwound,
        (true, false) => Outcome::KeptBoth,
        (false, true) => Outcome::Lost,
    }
}
