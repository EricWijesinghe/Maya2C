//! A DAO: token-weighted votes decide treasury grants.
//!
//! One proposal pays one grant. The vote uses the governance crate's rules
//! (voting window, quorum, approval threshold, timelock, expiry), and an
//! executed proposal is the treasury's single approval stage, so the
//! treasury's per-epoch limit still applies: a captured vote cannot empty it
//! in one epoch.

use maya_governance::error::GovernanceError;
use maya_governance::proposal::{Proposal, ProposalState};
use maya_governance::tally::Choice;
use maya_treasury::spending::{SpendError, Status, Treasury};

/// The DAO's constitution.
#[derive(Clone, Copy, Debug)]
pub struct Rules {
    /// Blocks a vote stays open.
    pub voting_blocks: u64,
    /// Blocks between passing and executing, so dissenters can exit.
    pub timelock_blocks: u64,
    /// Share of eligible weight that must vote, in basis points.
    pub quorum_bps: u32,
    /// Share of decisive votes that must be "for", in basis points.
    pub approval_bps: u32,
    /// Total voting weight.
    pub eligible: u128,
}

/// Why a grant did not pay.
#[derive(Debug, PartialEq, Eq)]
pub enum DaoError {
    /// The vote's rules refused the step.
    Governance(GovernanceError),
    /// The vote failed.
    Rejected,
    /// The treasury refused the payment.
    Treasury(SpendError),
}

/// A grant proposal: the vote and the treasury spend it would release.
pub struct Grant {
    /// The vote.
    pub proposal: Proposal,
    spend: usize,
}

/// Opens a vote on paying `amount` to `recipient`.
///
/// # Errors
///
/// [`DaoError::Governance`] if the schedule is invalid.
pub fn propose(
    treasury: &mut Treasury,
    rules: &Rules,
    recipient: [u8; 32],
    amount: u64,
    memo: &str,
    height: u64,
) -> Result<Grant, DaoError> {
    let proposal = Proposal::open(height, rules.voting_blocks, rules.timelock_blocks)
        .map_err(DaoError::Governance)?;
    let spend = treasury.propose(recipient, amount, memo, height);
    Ok(Grant { proposal, spend })
}

/// Records one weighted vote.
///
/// # Errors
///
/// [`DaoError::Governance`] outside the voting window.
pub fn vote(grant: &mut Grant, choice: Choice, weight: u128, height: u64) -> Result<(), DaoError> {
    grant.proposal = grant
        .proposal
        .vote(choice, weight, height)
        .map_err(DaoError::Governance)?;
    Ok(())
}

/// Closes the vote, waits out the timelock (the caller passes the height),
/// executes, and releases the treasury spend.
///
/// # Errors
///
/// [`DaoError::Rejected`] if it failed; [`DaoError::Governance`] before the
/// timelock or after expiry; [`DaoError::Treasury`] over the epoch limit.
pub fn execute(
    treasury: &mut Treasury,
    grant: &mut Grant,
    rules: &Rules,
    height: u64,
) -> Result<(), DaoError> {
    let decided = grant
        .proposal
        .finalize(height, rules.eligible, rules.quorum_bps, rules.approval_bps)
        .map_err(DaoError::Governance)?;
    grant.proposal = decided;
    if decided.state == ProposalState::Rejected {
        return Err(DaoError::Rejected);
    }
    grant.proposal = decided.execute(height).map_err(DaoError::Governance)?;
    match treasury.approve(grant.spend, 0, height) {
        Ok(Status::Executed { .. }) => Ok(()),
        Ok(_) => Err(DaoError::Treasury(SpendError::OutOfOrder)),
        Err(e) => Err(DaoError::Treasury(e)),
    }
}
